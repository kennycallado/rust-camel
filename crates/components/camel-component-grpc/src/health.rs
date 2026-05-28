use async_trait::async_trait;
use camel_api::{AsyncHealthCheck, CheckResult};
use camel_component_api::CamelError;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

type ProbeFuture = Pin<Box<dyn Future<Output = Result<(), CamelError>> + Send>>;

/// Trait abstracting a single connectivity probe (mockable for tests).
trait GrpcHealthProbe: Send + Sync {
    fn probe(&self) -> ProbeFuture;
}

/// Real probe that performs a TCP connect to the gRPC target host:port.
struct GrpcChannelProbe {
    host: String,
    port: u16,
}

impl GrpcChannelProbe {
    fn new(host: String, port: u16) -> Self {
        Self { host, port }
    }
}

impl GrpcHealthProbe for GrpcChannelProbe {
    fn probe(&self) -> ProbeFuture {
        let addr = format!("{}:{}", self.host, self.port);
        Box::pin(async move {
            tokio::net::TcpStream::connect(&addr)
                .await
                .map(|_| ())
                .map_err(|e| {
                    CamelError::ProcessorError(format!(
                        "gRPC channel health check failed for '{}': {}",
                        addr, e
                    ))
                })
        })
    }
}

/// Async health check for the gRPC component that probes TCP connectivity
/// to the configured target host:port.
pub struct GrpcHealthCheck {
    probe: Arc<dyn GrpcHealthProbe>,
    timeout: Duration,
}

impl GrpcHealthCheck {
    pub fn new(host: String, port: u16) -> Self {
        Self {
            probe: Arc::new(GrpcChannelProbe::new(host, port)),
            timeout: Duration::from_secs(3),
        }
    }

    #[cfg(test)]
    fn with_probe_for_tests(probe: Arc<dyn GrpcHealthProbe>, timeout: Duration) -> Self {
        Self { probe, timeout }
    }
}

#[async_trait]
impl AsyncHealthCheck for GrpcHealthCheck {
    fn name(&self) -> &str {
        "grpc"
    }

    async fn check(&self) -> CheckResult {
        match tokio::time::timeout(self.timeout, self.probe.probe()).await {
            Ok(Ok(())) => CheckResult::healthy(self.name()),
            Ok(Err(err)) => CheckResult::unhealthy(self.name(), &err.to_string()),
            Err(_) => CheckResult::unhealthy(self.name(), "gRPC channel probe timed out"),
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

    impl GrpcHealthProbe for MockProbe {
        fn probe(&self) -> ProbeFuture {
            (self.responder)()
        }
    }

    #[tokio::test]
    async fn grpc_health_check_healthy_when_probe_succeeds() {
        let probe = Arc::new(MockProbe::new(|| Box::pin(async { Ok(()) })));
        let check = GrpcHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "grpc");
        assert_eq!(result.status, HealthStatus::Healthy);
        assert!(result.message.is_none());
    }

    #[tokio::test]
    async fn grpc_health_check_unhealthy_when_probe_fails() {
        let probe = Arc::new(MockProbe::new(|| {
            Box::pin(async { Err(CamelError::ProcessorError("connection refused".to_string())) })
        }));
        let check = GrpcHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "grpc");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("connection refused"))
        );
    }

    #[tokio::test]
    async fn grpc_health_check_unhealthy_when_probe_times_out() {
        let probe = Arc::new(MockProbe::new(|| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(())
            })
        }));
        let check = GrpcHealthCheck::with_probe_for_tests(probe, Duration::from_millis(5));

        let result = check.check().await;

        assert_eq!(result.name, "grpc");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert_eq!(
            result.message.as_deref(),
            Some("gRPC channel probe timed out")
        );
    }
}
