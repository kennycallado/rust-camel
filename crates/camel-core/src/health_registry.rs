use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use camel_api::{AsyncHealthCheck, CheckResult, HealthReport, HealthStatus, ServiceHealth};
use chrono::Utc;
use futures::FutureExt;
use futures::future::join_all;
use tokio::time::{Duration, timeout};
use tokio_util::sync::CancellationToken;

enum HealthEntry {
    Live { check: Arc<dyn AsyncHealthCheck> },
    ForcedUnhealthy { name: String, reason: String },
}

pub struct HealthCheckRegistry {
    entries: RwLock<HashMap<String, Vec<HealthEntry>>>,
    default_timeout: Duration,
    cancel_token: CancellationToken,
}

impl HealthCheckRegistry {
    pub fn new(default_timeout: Duration) -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            default_timeout,
            cancel_token: CancellationToken::new(),
        }
    }

    pub fn register_for_route(&self, route_id: &str, check: Arc<dyn AsyncHealthCheck>) {
        let mut entries = self.entries.write().expect("health registry lock poisoned"); // allow-unwrap
        let route_entries = entries.entry(route_id.to_string()).or_default();
        if route_entries
            .iter()
            .any(|entry| matches!(entry, HealthEntry::ForcedUnhealthy { .. }))
        {
            *route_entries = vec![HealthEntry::Live { check }];
        } else {
            route_entries.push(HealthEntry::Live { check });
        }
    }

    pub fn unregister_for_route(&self, route_id: &str) {
        let mut entries = self.entries.write().expect("health registry lock poisoned"); // allow-unwrap
        entries.remove(route_id);
    }

    pub fn force_unhealthy_for_route(&self, route_id: &str, name: &str, reason: impl Into<String>) {
        let mut entries = self.entries.write().expect("health registry lock poisoned"); // allow-unwrap
        entries.insert(
            route_id.to_string(),
            vec![HealthEntry::ForcedUnhealthy {
                name: name.to_string(),
                reason: reason.into(),
            }],
        );
    }

    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel_token.clone()
    }

    pub async fn check_all(&self) -> HealthReport {
        if self.cancel_token.is_cancelled() {
            return HealthReport {
                status: HealthStatus::Unhealthy,
                services: vec![ServiceHealth {
                    name: "registry".to_string(),
                    status: camel_api::ServiceStatus::Failed,
                    message: Some("shutdown in progress".to_string()),
                }],
                timestamp: Utc::now(),
            };
        }

        let entries: Vec<HealthEntry> = {
            let guard = self.entries.read().expect("health registry lock poisoned"); // allow-unwrap
            guard
                .values()
                .flat_map(|route_entries| route_entries.iter())
                .map(|entry| match entry {
                    HealthEntry::Live { check } => HealthEntry::Live {
                        check: Arc::clone(check),
                    },
                    HealthEntry::ForcedUnhealthy { name, reason } => HealthEntry::ForcedUnhealthy {
                        name: name.clone(),
                        reason: reason.clone(),
                    },
                })
                .collect()
        };

        if entries.is_empty() {
            return HealthReport::default();
        }

        let futures: Vec<_> = entries
            .into_iter()
            .map(|entry| {
                let dur = self.default_timeout;
                async move {
                    match entry {
                        HealthEntry::Live { check } => {
                            let check_name = check.name().to_string();
                            std::panic::AssertUnwindSafe(async {
                                match timeout(dur, check.check()).await {
                                    Ok(result) => result,
                                    Err(_) => CheckResult::unhealthy(&check_name, "timed out"),
                                }
                            })
                            .catch_unwind()
                            .await
                            .unwrap_or_else(|_| {
                                CheckResult::unhealthy(&check_name, "checker panicked")
                            })
                        }
                        HealthEntry::ForcedUnhealthy { name, reason } => {
                            CheckResult::unhealthy(&name, &reason)
                        }
                    }
                }
            })
            .collect();

        let results = join_all(futures).await;

        let mut worst = HealthStatus::Healthy;
        let mut services = Vec::with_capacity(results.len());

        for result in results {
            if result.status == HealthStatus::Unhealthy {
                worst = HealthStatus::Unhealthy;
            } else if result.status == HealthStatus::Degraded && worst != HealthStatus::Unhealthy {
                worst = HealthStatus::Degraded;
            }
            let status = match result.status {
                HealthStatus::Healthy => camel_api::ServiceStatus::Started,
                HealthStatus::Degraded => camel_api::ServiceStatus::Started,
                HealthStatus::Unhealthy => camel_api::ServiceStatus::Failed,
            };
            services.push(ServiceHealth {
                name: result.name,
                status,
                message: result.message.or_else(|| {
                    if result.status == HealthStatus::Degraded {
                        Some("degraded".to_string())
                    } else {
                        None
                    }
                }),
            });
        }

        HealthReport {
            status: worst,
            services,
            timestamp: Utc::now(),
        }
    }
}

impl camel_component_api::HealthCheckRegistry for HealthCheckRegistry {
    fn force_unhealthy_for_route(&self, route_id: &str, name: &str, reason: &str) {
        // Rust's method resolution prefers inherent methods over trait methods
        // when both are in scope with the same name. This calls the inherent
        // method (defined on HealthCheckRegistry directly above), NOT this
        // trait method we're defining here — so there is no recursion.
        self.force_unhealthy_for_route(route_id, name, reason.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockCheck {
        check_name: String,
        result: CheckResult,
    }

    #[async_trait]
    impl AsyncHealthCheck for MockCheck {
        fn name(&self) -> &str {
            &self.check_name
        }

        async fn check(&self) -> CheckResult {
            self.result.clone()
        }
    }

    fn healthy_check(name: &str) -> Arc<dyn AsyncHealthCheck> {
        Arc::new(MockCheck {
            check_name: name.to_string(),
            result: CheckResult::healthy(name),
        })
    }

    fn unhealthy_check(name: &str) -> Arc<dyn AsyncHealthCheck> {
        Arc::new(MockCheck {
            check_name: name.to_string(),
            result: CheckResult::unhealthy(name, "fail"),
        })
    }

    fn degraded_check(name: &str) -> Arc<dyn AsyncHealthCheck> {
        Arc::new(MockCheck {
            check_name: name.to_string(),
            result: CheckResult::degraded(name, "slow"),
        })
    }

    #[test]
    fn register_and_unregister_are_sync() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.register_for_route("route-1", healthy_check("redis"));
        registry.unregister_for_route("route-1");
    }

    #[tokio::test]
    async fn empty_registry_returns_healthy() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Healthy);
        assert!(report.services.is_empty());
    }

    #[tokio::test]
    async fn single_healthy_check() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.register_for_route("route-1", healthy_check("redis"));
        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Healthy);
        assert_eq!(report.services.len(), 1);
        assert!(report.services[0].message.is_none());
    }

    #[tokio::test]
    async fn one_unhealthy_makes_aggregate_unhealthy() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.register_for_route("route-1", healthy_check("redis"));
        registry.register_for_route("route-2", unhealthy_check("kafka"));
        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn one_degraded_makes_aggregate_degraded() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.register_for_route("route-1", healthy_check("redis"));
        registry.register_for_route("route-2", degraded_check("sql"));
        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Degraded);
    }

    #[tokio::test]
    async fn unhealthy_takes_precedence_over_degraded() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.register_for_route("route-1", degraded_check("sql"));
        registry.register_for_route("route-2", unhealthy_check("kafka"));
        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn multiple_checks_per_route_all_reported() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.register_for_route("route-1", healthy_check("redis"));
        registry.register_for_route("route-1", unhealthy_check("sql"));
        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Unhealthy);
        assert_eq!(report.services.len(), 2);
    }

    #[tokio::test]
    async fn unregister_removes_check() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.register_for_route("route-1", unhealthy_check("kafka"));
        registry.unregister_for_route("route-1");
        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Healthy);
        assert!(report.services.is_empty());
    }

    #[tokio::test]
    async fn cancelled_token_returns_unhealthy() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.cancel_token().cancel();
        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn message_preserved_in_report() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.register_for_route("route-1", unhealthy_check("kafka"));
        let report = registry.check_all().await;
        assert_eq!(report.services[0].message.as_deref(), Some("fail"));
    }

    struct SlowCheck;

    #[async_trait]
    impl AsyncHealthCheck for SlowCheck {
        fn name(&self) -> &str {
            "slow"
        }

        async fn check(&self) -> CheckResult {
            tokio::time::sleep(Duration::from_secs(10)).await;
            CheckResult::healthy("slow")
        }
    }

    #[tokio::test]
    async fn timeout_returns_unhealthy() {
        let registry = HealthCheckRegistry::new(Duration::from_millis(50));
        registry.register_for_route("route-1", Arc::new(SlowCheck));
        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Unhealthy);
    }

    struct PanickingCheck;

    #[async_trait]
    impl AsyncHealthCheck for PanickingCheck {
        fn name(&self) -> &str {
            "panicker"
        }

        async fn check(&self) -> CheckResult {
            panic!("intentional panic");
        }
    }

    #[tokio::test]
    async fn panic_caught_and_reported_as_unhealthy() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.register_for_route("route-1", Arc::new(PanickingCheck));
        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Unhealthy);
        assert!(
            report.services[0]
                .message
                .as_deref()
                .unwrap()
                .contains("panicked")
        );
    }

    #[tokio::test]
    async fn register_during_check_all_does_not_deadlock() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.register_for_route("route-1", healthy_check("redis"));
        let registry = Arc::new(registry);
        let reg = Arc::clone(&registry);
        let h = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            reg.register_for_route("route-2", healthy_check("late-sql"));
        });
        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Healthy);
        h.await.unwrap();
    }

    struct CountingCheck {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl AsyncHealthCheck for CountingCheck {
        fn name(&self) -> &str {
            "counting"
        }

        async fn check(&self) -> CheckResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            CheckResult::healthy("counting")
        }
    }

    #[tokio::test]
    async fn force_unhealthy_returns_unhealthy_without_io() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        let calls = Arc::new(AtomicUsize::new(0));
        registry.register_for_route(
            "route-1",
            Arc::new(CountingCheck {
                calls: Arc::clone(&calls),
            }),
        );
        registry.force_unhealthy_for_route("route-1", "forced", "route failed");

        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Unhealthy);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(report.services[0].name, "forced");
        assert_eq!(report.services[0].message.as_deref(), Some("route failed"));
    }

    #[tokio::test]
    async fn force_unhealthy_replaces_all_checks_for_route() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.register_for_route("route-1", healthy_check("redis"));
        registry.register_for_route("route-1", unhealthy_check("sql"));

        registry.force_unhealthy_for_route("route-1", "forced", "route failed");

        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Unhealthy);
        assert_eq!(report.services.len(), 1);
        assert_eq!(report.services[0].name, "forced");
        assert_eq!(report.services[0].message.as_deref(), Some("route failed"));
    }

    #[tokio::test]
    async fn force_unhealthy_then_register_replaces_with_live() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.force_unhealthy_for_route("route-1", "forced", "route failed");
        registry.register_for_route("route-1", healthy_check("redis"));

        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Healthy);
        assert_eq!(report.services.len(), 1);
        assert_eq!(report.services[0].name, "redis");
    }

    #[tokio::test]
    async fn forced_unhealthy_persists_until_replaced() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.force_unhealthy_for_route("route-1", "forced", "route failed");

        let report1 = registry.check_all().await;
        let report2 = registry.check_all().await;

        assert_eq!(report1.status, HealthStatus::Unhealthy);
        assert_eq!(report2.status, HealthStatus::Unhealthy);
        assert_eq!(report1.services[0].name, "forced");
        assert_eq!(report2.services[0].name, "forced");
    }

    // ---------------------------------------------------------
    // Regression: trait delegation must not recurse infinitely.
    // ---------------------------------------------------------

    #[tokio::test]
    async fn health_registry_trait_delegation_does_not_recurse() {
        // GIVEN: the concrete HealthCheckRegistry with a registered probe.
        let registry = HealthCheckRegistry::new(std::time::Duration::from_secs(5));
        // Register a no-op live check so the route has at least one entry
        // (force_unhealthy_for_route replaces all entries for the route).
        struct NoopCheck;
        #[async_trait]
        impl camel_api::AsyncHealthCheck for NoopCheck {
            fn name(&self) -> &str {
                "noop"
            }
            async fn check(&self) -> camel_api::CheckResult {
                camel_api::CheckResult::healthy("noop")
            }
        }
        registry.register_for_route("test-route", std::sync::Arc::new(NoopCheck));

        // WHEN: the trait method is invoked (UFCS to be explicit about which
        // method we're calling). This MUST resolve to the inherent impl, NOT
        // recurse into the trait method body that calls it.
        camel_component_api::HealthCheckRegistry::force_unhealthy_for_route(
            &registry,
            "test-route",
            "probe",
            "test reason",
        );

        // THEN: call completed without stack overflow AND the route is now
        // Unhealthy with the forced entry. The `force_unhealthy_for_route`
        // inherent method replaces all existing entries with a single
        // ForcedUnhealthy entry; if the trait method had recursed instead,
        // we'd have stack-overflowed before reaching this assertion.
        let report = registry.check_all().await;
        assert_eq!(report.status, camel_api::HealthStatus::Unhealthy);
        assert_eq!(report.services.len(), 1);
        assert_eq!(report.services[0].name, "probe");
        assert_eq!(report.services[0].message.as_deref(), Some("test reason"));
    }
}
