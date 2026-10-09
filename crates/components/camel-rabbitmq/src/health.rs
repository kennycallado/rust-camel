//! RabbitMQ health check: a bounded connection probe over the broker's
//! shared [`RabbitConnectionManager`] (kafka `health.rs` shape).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use camel_api::{AsyncHealthCheck, CheckResult};
use camel_component_api::CamelError;

use crate::connection::RabbitConnectionManager;

type ProbeFuture = Pin<Box<dyn Future<Output = Result<(), CamelError>> + Send>>;

/// Testable probe seam: yields `Ok` once the manager holds a live
/// connection, `Err` on exhaustion.
trait RabbitProbe: Send + Sync {
    fn probe(&self) -> ProbeFuture;
}

/// Real probe: bounded wait for a live connection.
struct RabbitManagerProbe {
    manager: Arc<RabbitConnectionManager>,
    timeout: Duration,
}

impl RabbitManagerProbe {
    fn new(manager: Arc<RabbitConnectionManager>, timeout: Duration) -> Self {
        Self { manager, timeout }
    }
}

impl RabbitProbe for RabbitManagerProbe {
    fn probe(&self) -> ProbeFuture {
        let manager = Arc::clone(&self.manager);
        let timeout = self.timeout;
        Box::pin(async move { manager.connection_within(timeout).await.map(|_| ()) })
    }
}

/// `AsyncHealthCheck` over the `rabbitmq` scheme's shared connection manager.
pub struct RabbitHealthCheck {
    probe: Arc<dyn RabbitProbe>,
    timeout: Duration,
}

impl RabbitHealthCheck {
    /// Build the real check over `manager` with a 5 s probe bound.
    pub fn new(manager: Arc<RabbitConnectionManager>) -> Self {
        let timeout = Duration::from_secs(5);
        Self {
            probe: Arc::new(RabbitManagerProbe::new(manager, timeout)),
            timeout,
        }
    }

    #[cfg(test)]
    fn with_probe_for_tests(probe: Arc<dyn RabbitProbe>, timeout: Duration) -> Self {
        Self { probe, timeout }
    }
}

#[async_trait]
impl AsyncHealthCheck for RabbitHealthCheck {
    fn name(&self) -> &str {
        "rabbitmq"
    }

    async fn check(&self) -> CheckResult {
        match tokio::time::timeout(self.timeout, self.probe.probe()).await {
            Ok(Ok(())) => CheckResult::healthy(self.name()),
            Ok(Err(err)) => CheckResult::unhealthy(self.name(), &err.to_string()),
            Err(_) => CheckResult::unhealthy(self.name(), "connection probe timed out"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::HealthStatus;

    struct MockProbe {
        responder: Arc<dyn Fn() -> ProbeFuture + Send + Sync>,
    }

    impl MockProbe {
        fn new<F>(f: F) -> Self
        where
            F: Fn() -> ProbeFuture + Send + Sync + 'static,
        {
            Self {
                responder: Arc::new(f),
            }
        }
    }

    impl RabbitProbe for MockProbe {
        fn probe(&self) -> ProbeFuture {
            (self.responder)()
        }
    }

    #[tokio::test]
    async fn health_unhealthy_on_probe_error() {
        let probe = Arc::new(MockProbe::new(|| {
            Box::pin(async {
                Err(CamelError::ProcessorError(
                    "simulated rabbitmq probe error".to_string(),
                ))
            })
        }));
        let check = RabbitHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "rabbitmq");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("simulated rabbitmq probe error"))
        );
    }

    /// `start_paused` drives the 50 ms check bound on the virtual clock, so
    /// the 10 s pending probe is cut instantly (no wall-clock sleep).
    #[tokio::test(start_paused = true)]
    async fn health_probe_timeout_bounds_wait() {
        let probe = Arc::new(MockProbe::new(|| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_secs(10)).await;
                Ok(())
            })
        }));
        let check = RabbitHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "rabbitmq");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert_eq!(
            result.message.as_deref(),
            Some("connection probe timed out")
        );
    }
}
