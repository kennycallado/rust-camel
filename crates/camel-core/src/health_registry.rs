use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use camel_api::{AsyncHealthCheck, CheckResult, HealthReport, HealthStatus, ServiceHealth};
use chrono::Utc;
use futures::FutureExt;
use futures::future::join_all;
use tokio::time::{Duration, timeout};
use tokio_util::sync::CancellationToken;

struct ForcedEntry {
    name: String,
    reason: String,
}

struct RouteHealth {
    active: bool,
    live: Vec<Arc<dyn AsyncHealthCheck>>,
    forced: Option<ForcedEntry>,
}

pub struct HealthCheckRegistry {
    entries: RwLock<HashMap<String, RouteHealth>>,
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
        let mut entries = self.entries.write();
        let route_health = entries
            .entry(route_id.to_string())
            .or_insert_with(|| RouteHealth {
                active: false,
                live: Vec::new(),
                forced: None,
            });
        let check_name = check.name();
        if let Some(existing) = route_health
            .live
            .iter()
            .position(|c| c.name() == check_name)
        {
            route_health.live[existing] = check;
        } else {
            route_health.live.push(check);
        }
        route_health.forced = None;
    }

    pub fn mark_route_started(&self, route_id: &str) {
        let mut entries = self.entries.write();
        if let Some(route_health) = entries.get_mut(route_id) {
            route_health.active = true;
        }
    }

    pub fn mark_route_stopped(&self, route_id: &str) {
        let mut entries = self.entries.write();
        if let Some(route_health) = entries.get_mut(route_id) {
            route_health.active = false;
        }
    }

    pub fn unregister_for_route(&self, route_id: &str) {
        let mut entries = self.entries.write();
        entries.remove(route_id);
    }

    pub fn force_unhealthy_for_route(&self, route_id: &str, name: &str, reason: impl Into<String>) {
        let mut entries = self.entries.write();
        let route_health = entries
            .entry(route_id.to_string())
            .or_insert_with(|| RouteHealth {
                active: false,
                live: Vec::new(),
                forced: None,
            });
        route_health.active = true;
        route_health.forced = Some(ForcedEntry {
            name: name.to_string(),
            reason: reason.into(),
        });
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

        let checks: Vec<CheckTask> = {
            let guard = self.entries.read();
            guard
                .values()
                .filter(|rh| rh.active)
                .flat_map(|rh| {
                    if let Some(ref forced) = rh.forced {
                        vec![CheckTask::Forced {
                            name: forced.name.clone(),
                            reason: forced.reason.clone(),
                        }]
                    } else {
                        rh.live
                            .iter()
                            .map(|c| CheckTask::Live {
                                check: Arc::clone(c),
                            })
                            .collect()
                    }
                })
                .collect()
        };

        if checks.is_empty() {
            return HealthReport::default();
        }

        let futures: Vec<_> = checks
            .into_iter()
            .map(|task| {
                let dur = self.default_timeout;
                async move {
                    match task {
                        CheckTask::Live { check } => {
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
                        CheckTask::Forced { name, reason } => {
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

enum CheckTask {
    Live { check: Arc<dyn AsyncHealthCheck> },
    Forced { name: String, reason: String },
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
        registry.mark_route_started("route-1");
        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Healthy);
        assert_eq!(report.services.len(), 1);
        assert!(report.services[0].message.is_none());
    }

    #[tokio::test]
    async fn one_unhealthy_makes_aggregate_unhealthy() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.register_for_route("route-1", healthy_check("redis"));
        registry.mark_route_started("route-1");
        registry.register_for_route("route-2", unhealthy_check("kafka"));
        registry.mark_route_started("route-2");
        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn one_degraded_makes_aggregate_degraded() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.register_for_route("route-1", healthy_check("redis"));
        registry.mark_route_started("route-1");
        registry.register_for_route("route-2", degraded_check("sql"));
        registry.mark_route_started("route-2");
        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Degraded);
    }

    #[tokio::test]
    async fn unhealthy_takes_precedence_over_degraded() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.register_for_route("route-1", degraded_check("sql"));
        registry.mark_route_started("route-1");
        registry.register_for_route("route-2", unhealthy_check("kafka"));
        registry.mark_route_started("route-2");
        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn multiple_checks_per_route_all_reported() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.register_for_route("route-1", healthy_check("redis"));
        registry.register_for_route("route-1", unhealthy_check("sql"));
        registry.mark_route_started("route-1");
        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Unhealthy);
        assert_eq!(report.services.len(), 2);
    }

    #[tokio::test]
    async fn unregister_removes_check() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.register_for_route("route-1", unhealthy_check("kafka"));
        registry.mark_route_started("route-1");
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
        registry.mark_route_started("route-1");
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
        registry.mark_route_started("route-1");
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
        registry.mark_route_started("route-1");
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
        registry.mark_route_started("route-1");
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
        let registry = HealthCheckRegistry::new(std::time::Duration::from_secs(5));
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

        camel_component_api::HealthCheckRegistry::force_unhealthy_for_route(
            &registry,
            "test-route",
            "probe",
            "test reason",
        );

        let report = registry.check_all().await;
        assert_eq!(report.status, camel_api::HealthStatus::Unhealthy);
        assert_eq!(report.services.len(), 1);
        assert_eq!(report.services[0].name, "probe");
        assert_eq!(report.services[0].message.as_deref(), Some("test reason"));
    }

    // ---------------------------------------------------------
    // Route-health state gating tests
    // ---------------------------------------------------------

    #[tokio::test]
    async fn inactive_route_not_checked() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.register_for_route("route-1", unhealthy_check("redis"));
        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Healthy);
        assert!(report.services.is_empty());
    }

    #[tokio::test]
    async fn mark_route_started_makes_check_active() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.register_for_route("route-1", healthy_check("redis"));
        registry.mark_route_started("route-1");
        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Healthy);
        assert_eq!(report.services.len(), 1);
        assert_eq!(report.services[0].name, "redis");
    }

    #[tokio::test]
    async fn mark_route_stopped_makes_check_inactive() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.register_for_route("route-1", unhealthy_check("redis"));
        registry.mark_route_started("route-1");
        registry.mark_route_stopped("route-1");
        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Healthy);
        assert!(report.services.is_empty());
    }

    #[tokio::test]
    async fn stop_does_not_delete_probes() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.register_for_route("route-1", healthy_check("redis"));
        registry.mark_route_started("route-1");
        registry.mark_route_stopped("route-1");
        registry.mark_route_started("route-1");
        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Healthy);
        assert_eq!(report.services.len(), 1);
        assert_eq!(report.services[0].name, "redis");
    }

    #[tokio::test]
    async fn forced_unhealthy_marks_route_active() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.register_for_route("route-1", healthy_check("redis"));
        // route not marked started — still inactive
        registry.force_unhealthy_for_route("route-1", "forced", "crashed");
        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Unhealthy);
        assert_eq!(report.services.len(), 1);
        assert_eq!(report.services[0].name, "forced");
        assert_eq!(report.services[0].message.as_deref(), Some("crashed"));
    }

    #[tokio::test]
    async fn restart_does_not_duplicate_probes() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        let calls = Arc::new(AtomicUsize::new(0));
        let check1: Arc<dyn AsyncHealthCheck> = Arc::new(CountingCheck {
            calls: Arc::clone(&calls),
        });
        registry.register_for_route("route-1", check1);
        registry.mark_route_started("route-1");
        registry.mark_route_stopped("route-1");

        let check2: Arc<dyn AsyncHealthCheck> = Arc::new(CountingCheck {
            calls: Arc::clone(&calls),
        });
        registry.register_for_route("route-1", check2);
        registry.mark_route_started("route-1");

        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Healthy);
        assert_eq!(report.services.len(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn remove_route_unregisters_completely() {
        let registry = HealthCheckRegistry::new(Duration::from_secs(5));
        registry.register_for_route("route-1", healthy_check("redis"));
        registry.mark_route_started("route-1");
        registry.unregister_for_route("route-1");
        let report = registry.check_all().await;
        assert_eq!(report.status, HealthStatus::Healthy);
        assert!(report.services.is_empty());
    }

    // ---------------------------------------------------------
    // Regression R4-H2: concurrent readers + writer must not
    // poison / panic / leave the registry stuck NotReady.
    // ---------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_readers_and_writer_never_poison() {
        let registry = Arc::new(HealthCheckRegistry::new(Duration::from_secs(5)));
        registry.register_for_route("route-1", healthy_check("redis"));
        registry.mark_route_started("route-1");

        let mut handles = Vec::new();
        for i in 0..8 {
            let reg = Arc::clone(&registry);
            handles.push(tokio::spawn(async move {
                for _ in 0..50 {
                    reg.register_for_route("route-1", healthy_check(&format!("chk-{i}")));
                    reg.unregister_for_route("route-1");
                    reg.register_for_route("route-1", healthy_check(&format!("chk-{i}")));
                }
            }));
        }
        for _ in 0..8 {
            let reg = Arc::clone(&registry);
            handles.push(tokio::spawn(async move {
                for _ in 0..50 {
                    let report = reg.check_all().await;
                    let _ = report.status;
                }
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        registry.register_for_route("route-final", healthy_check("final"));
        registry.mark_route_started("route-final");
        let report = registry.check_all().await;
        assert_ne!(report.services.len(), 0);
    }
}
