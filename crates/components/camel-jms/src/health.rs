use crate::component::{BridgeState, JmsBridgePool};
use crate::proto::{HealthRequest, bridge_service_client::BridgeServiceClient};
use async_trait::async_trait;
use camel_api::{AsyncHealthCheck, CheckResult};
use camel_component_api::CamelError;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

type HealthFuture = Pin<Box<dyn Future<Output = Result<bool, CamelError>> + Send>>;

/// Trait abstracting a single gRPC health probe call (mockable for tests).
trait BridgeHealthProbe: Send + Sync {
    fn probe(&self) -> HealthFuture;
}

/// Real probe that reaches into the bridge pool, extracts the channel from a
/// Ready slot, and issues a gRPC `HealthRequest` via `BridgeServiceClient`.
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

            let mut client = BridgeServiceClient::new(channel);
            let resp = client.health(HealthRequest {}).await.map_err(|e| {
                CamelError::ProcessorError(format!(
                    "JMS bridge health RPC failed for broker '{}': {}",
                    broker_name, e
                ))
            })?;

            Ok(resp.into_inner().healthy)
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
        match tokio::time::timeout(self.timeout, self.probe.probe()).await {
            Ok(Ok(true)) => CheckResult::healthy(self.name()),
            Ok(Ok(false)) => CheckResult::unhealthy(self.name(), "bridge reported unhealthy"),
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

    #[tokio::test]
    async fn jms_health_check_healthy_on_true() {
        let probe = Arc::new(MockHealthProbe::new(|| Box::pin(async { Ok(true) })));
        let check = JmsHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "jms");
        assert_eq!(result.status, HealthStatus::Healthy);
        assert!(result.message.is_none());
    }

    #[tokio::test]
    async fn jms_health_check_unhealthy_on_error() {
        let probe = Arc::new(MockHealthProbe::new(|| {
            Box::pin(async {
                Err(CamelError::ProcessorError(
                    "simulated bridge error".to_string(),
                ))
            })
        }));
        let check = JmsHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "jms");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("simulated bridge error"))
        );
    }

    #[tokio::test]
    async fn jms_health_check_unhealthy_on_timeout() {
        let probe = Arc::new(MockHealthProbe::new(|| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(true)
            })
        }));
        let check = JmsHealthCheck::with_probe_for_tests(probe, Duration::from_millis(5));

        let result = check.check().await;

        assert_eq!(result.name, "jms");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert_eq!(result.message.as_deref(), Some("health probe timed out"));
    }
}
