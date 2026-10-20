use crate::component::{BridgeState, JmsBridgePool, bridge_service_client};
use crate::proto::HealthRequest;
use async_trait::async_trait;
use camel_api::{AsyncHealthCheck, CheckResult};
use camel_component_api::CamelError;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// One bridge health probe result: bridge liveness plus broker connectivity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BridgeProbeResult {
    pub healthy: bool,
    pub broker_connected: bool,
    pub message: String,
}

type HealthFuture = Pin<Box<dyn Future<Output = Result<BridgeProbeResult, CamelError>> + Send>>;

/// Trait abstracting a single gRPC health probe call (mockable for tests).
trait BridgeHealthProbe: Send + Sync {
    fn probe(&self) -> HealthFuture;

    /// True while no bridge slot exists for the broker yet (producer-only
    /// routes create it lazily). Readiness reports this as Degraded (200).
    fn not_started(&self) -> bool {
        false
    }
}

/// Real probe that reaches into the bridge pool, extracts the channel from a
/// Ready slot, and issues a gRPC `HealthRequest` via `bridge_service_client`.
struct JmsBridgeHealthProbe {
    pool: Arc<JmsBridgePool>,
    broker_name: String,
}

impl JmsBridgeHealthProbe {
    fn new(pool: Arc<JmsBridgePool>, broker_name: String) -> Self {
        Self { pool, broker_name }
    }
}

impl BridgeHealthProbe for JmsBridgeHealthProbe {
    fn not_started(&self) -> bool {
        !self.pool.slots.contains_key(&self.broker_name)
    }

    fn probe(&self) -> HealthFuture {
        let pool = Arc::clone(&self.pool);
        let broker_name = self.broker_name.clone();

        Box::pin(async move {
            let slot = pool.slots.get(&broker_name).ok_or_else(|| {
                CamelError::ProcessorError(format!(
                    "No bridge slot found for broker '{}'",
                    broker_name
                ))
            })?;

            let channel = match &*slot.state_rx.borrow() {
                BridgeState::Ready { channel } => channel.clone(),
                other => {
                    return Err(CamelError::ProcessorError(format!(
                        "Bridge not ready for broker '{}': {:?}",
                        broker_name, other
                    )));
                }
            };

            let mut client = bridge_service_client(channel);
            let resp = client.health(HealthRequest {}).await.map_err(|e| {
                CamelError::ProcessorError(format!(
                    "JMS bridge health RPC failed for broker '{}': {}",
                    broker_name, e
                ))
            })?;

            let resp = resp.into_inner();
            Ok(BridgeProbeResult {
                healthy: resp.healthy,
                broker_connected: resp.broker_connected,
                message: resp.message,
            })
        })
    }
}

/// Async health check for the JMS component that probes the bridge sidecar
/// via gRPC `HealthRequest`.
pub struct JmsHealthCheck {
    probe: Arc<dyn BridgeHealthProbe>,
    timeout: Duration,
}

impl JmsHealthCheck {
    pub fn new(pool: Arc<JmsBridgePool>, broker_name: String) -> Self {
        Self {
            probe: Arc::new(JmsBridgeHealthProbe::new(pool, broker_name)),
            timeout: Duration::from_secs(2),
        }
    }

    #[cfg(test)]
    fn with_probe_for_tests(probe: Arc<dyn BridgeHealthProbe>, timeout: Duration) -> Self {
        Self { probe, timeout }
    }
}

#[async_trait]
impl AsyncHealthCheck for JmsHealthCheck {
    fn name(&self) -> &str {
        "jms"
    }

    async fn check(&self) -> CheckResult {
        if self.probe.not_started() {
            return CheckResult::degraded(self.name(), "bridge not started (starts on first use)");
        }
        match tokio::time::timeout(self.timeout, self.probe.probe()).await {
            Ok(Ok(result)) if !result.healthy => {
                let message = if result.message.is_empty() {
                    "bridge reported unhealthy"
                } else {
                    &result.message
                };
                CheckResult::unhealthy(self.name(), message)
            }
            Ok(Ok(result)) if !result.broker_connected => {
                let message = if result.message.is_empty() {
                    "broker status unverified"
                } else {
                    &result.message
                };
                CheckResult::degraded(self.name(), message)
            }
            Ok(Ok(_)) => CheckResult::healthy(self.name()),
            Ok(Err(err)) => CheckResult::unhealthy(self.name(), &err.to_string()),
            Err(_) => CheckResult::unhealthy(self.name(), "health probe timed out"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::HealthStatus;

    struct MockHealthProbe {
        responder: Arc<dyn Fn() -> HealthFuture + Send + Sync>,
    }

    impl MockHealthProbe {
        fn new<F>(f: F) -> Self
        where
            F: Fn() -> HealthFuture + Send + Sync + 'static,
        {
            Self {
                responder: Arc::new(f),
            }
        }
    }

    impl BridgeHealthProbe for MockHealthProbe {
        fn probe(&self) -> HealthFuture {
            (self.responder)()
        }
    }

    struct NotStartedProbe;

    impl BridgeHealthProbe for NotStartedProbe {
        fn probe(&self) -> HealthFuture {
            Box::pin(async { Err(CamelError::ProcessorError("must not probe".to_string())) })
        }

        fn not_started(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn jms_health_check_degraded_when_bridge_not_started() {
        let check = JmsHealthCheck::with_probe_for_tests(
            Arc::new(NotStartedProbe),
            Duration::from_millis(50),
        );

        let result = check.check().await;

        assert_eq!(result.status, HealthStatus::Degraded);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("not started"))
        );
    }

    fn probe_result(healthy: bool, broker_connected: bool, message: &str) -> HealthFuture {
        let message = message.to_string();
        Box::pin(async move {
            Ok(BridgeProbeResult {
                healthy,
                broker_connected,
                message,
            })
        })
    }

    #[tokio::test]
    async fn jms_health_check_healthy_when_verified_up() {
        let probe = Arc::new(MockHealthProbe::new(|| probe_result(true, true, "ok")));
        let check = JmsHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "jms");
        assert_eq!(result.status, HealthStatus::Healthy);
        assert!(result.message.is_none());
    }

    #[tokio::test]
    async fn jms_health_check_unhealthy_when_verified_down() {
        let probe = Arc::new(MockHealthProbe::new(|| {
            probe_result(false, false, "broker unreachable")
        }));
        let check = JmsHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "jms");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert_eq!(result.message.as_deref(), Some("broker unreachable"));
    }

    #[tokio::test]
    async fn jms_health_check_degraded_for_unknown_or_skipped() {
        let probe = Arc::new(MockHealthProbe::new(|| {
            probe_result(true, false, "broker probe skipped; connectivity unverified")
        }));
        let check = JmsHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        for _ in 0..2 {
            let result = check.check().await;

            assert_eq!(result.name, "jms");
            assert_eq!(result.status, HealthStatus::Degraded);
            assert_eq!(
                result.message.as_deref(),
                Some("broker probe skipped; connectivity unverified")
            );
        }
    }

    #[tokio::test]
    async fn jms_health_check_degraded_for_stale() {
        let probe = Arc::new(MockHealthProbe::new(|| {
            probe_result(true, false, "broker status stale; age 10001 ms")
        }));
        let check = JmsHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "jms");
        assert_eq!(result.status, HealthStatus::Degraded);
        assert_eq!(
            result.message.as_deref(),
            Some("broker status stale; age 10001 ms")
        );
    }

    #[tokio::test]
    async fn jms_health_check_unhealthy_on_error_and_timeout() {
        let error_probe = Arc::new(MockHealthProbe::new(|| {
            Box::pin(async {
                Err(CamelError::ProcessorError(
                    "simulated bridge error".to_string(),
                ))
            })
        }));
        let error_check =
            JmsHealthCheck::with_probe_for_tests(error_probe, Duration::from_millis(50));

        let result = error_check.check().await;

        assert_eq!(result.name, "jms");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("simulated bridge error"))
        );

        let sleeping_probe = Arc::new(MockHealthProbe::new(|| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(BridgeProbeResult {
                    healthy: true,
                    broker_connected: true,
                    message: String::new(),
                })
            })
        }));
        let timeout_check =
            JmsHealthCheck::with_probe_for_tests(sleeping_probe, Duration::from_millis(5));

        let result = timeout_check.check().await;

        assert_eq!(result.name, "jms");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert_eq!(result.message.as_deref(), Some("health probe timed out"));
    }
}
