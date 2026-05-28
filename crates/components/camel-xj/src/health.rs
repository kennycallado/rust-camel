use async_trait::async_trait;
use camel_api::{AsyncHealthCheck, CheckResult};
use camel_component_api::CamelError;
use camel_xslt::BridgeState;
use camel_xslt::proto::{HealthCheckRequest, health_client::HealthClient};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

type HealthFuture = Pin<Box<dyn Future<Output = Result<bool, CamelError>> + Send>>;

/// Trait abstracting a single gRPC health probe call (mockable for tests).
trait XjBridgeHealthProbe: Send + Sync {
    fn probe(&self) -> HealthFuture;
}

/// Real probe that peeks at the bridge state and, if Ready, issues a gRPC
/// `HealthCheckRequest` via `HealthClient`.
struct XjBridgeProbe {
    state_rx: Arc<watch::Receiver<BridgeState>>,
}

impl XjBridgeProbe {
    fn new(state_rx: Arc<watch::Receiver<BridgeState>>) -> Self {
        Self { state_rx }
    }
}

impl XjBridgeHealthProbe for XjBridgeProbe {
    fn probe(&self) -> HealthFuture {
        let state_rx = Arc::clone(&self.state_rx);

        Box::pin(async move {
            let channel = match &*state_rx.borrow() {
                BridgeState::Ready { channel } => channel.clone(),
                other => {
                    return Err(CamelError::ProcessorError(format!(
                        "xj bridge not ready: {other:?}"
                    )));
                }
            };

            let mut client = HealthClient::new(channel);
            let resp = client
                .check(HealthCheckRequest {})
                .await
                .map_err(|e| CamelError::ProcessorError(format!("xj health RPC failed: {e}")))?;

            Ok(resp.into_inner().status == "SERVING")
        })
    }
}

/// Async health check for the XJ component that probes the xml-bridge
/// sidecar via gRPC `HealthCheckRequest`.
pub struct XjHealthCheck {
    probe: Arc<dyn XjBridgeHealthProbe>,
    timeout: Duration,
}

impl XjHealthCheck {
    pub fn new(state_rx: Arc<watch::Receiver<BridgeState>>) -> Self {
        Self {
            probe: Arc::new(XjBridgeProbe::new(state_rx)),
            timeout: Duration::from_secs(2),
        }
    }

    #[cfg(test)]
    fn with_probe_for_tests(probe: Arc<dyn XjBridgeHealthProbe>, timeout: Duration) -> Self {
        Self { probe, timeout }
    }
}

#[async_trait]
impl AsyncHealthCheck for XjHealthCheck {
    fn name(&self) -> &str {
        "xj"
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

    impl XjBridgeHealthProbe for MockHealthProbe {
        fn probe(&self) -> HealthFuture {
            (self.responder)()
        }
    }

    #[tokio::test]
    async fn xj_health_check_healthy_on_true() {
        let probe = Arc::new(MockHealthProbe::new(|| Box::pin(async { Ok(true) })));
        let check = XjHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "xj");
        assert_eq!(result.status, HealthStatus::Healthy);
        assert!(result.message.is_none());
    }

    #[tokio::test]
    async fn xj_health_check_unhealthy_on_error() {
        let probe = Arc::new(MockHealthProbe::new(|| {
            Box::pin(async {
                Err(CamelError::ProcessorError(
                    "simulated bridge error".to_string(),
                ))
            })
        }));
        let check = XjHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "xj");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("simulated bridge error"))
        );
    }

    #[tokio::test]
    async fn xj_health_check_unhealthy_on_timeout() {
        let probe = Arc::new(MockHealthProbe::new(|| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(true)
            })
        }));
        let check = XjHealthCheck::with_probe_for_tests(probe, Duration::from_millis(5));

        let result = check.check().await;

        assert_eq!(result.name, "xj");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert_eq!(result.message.as_deref(), Some("health probe timed out"));
    }
}
