use async_trait::async_trait;
use camel_api::{AsyncHealthCheck, CheckResult};
use camel_component_api::CamelError;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use crate::ContainerConfig;

type ProbeFuture = Pin<Box<dyn Future<Output = Result<(), CamelError>> + Send>>;

/// Trait for probing Docker container health.
trait ContainerHealthProbe: Send + Sync {
    fn probe(&self) -> ProbeFuture;
}

/// Real probe that creates a Docker client and calls `Docker::ping()`.
struct DockerPingProbe {
    config: ContainerConfig,
}

impl DockerPingProbe {
    fn new(config: &ContainerConfig) -> Self {
        Self {
            config: config.clone(),
        }
    }
}

impl ContainerHealthProbe for DockerPingProbe {
    fn probe(&self) -> ProbeFuture {
        let config = self.config.clone();
        Box::pin(async move {
            let docker = config.connect_docker_client()?;
            docker
                .ping()
                .await
                .map(|_| ())
                .map_err(|e| CamelError::ProcessorError(format!("Docker ping failed: {}", e)))
        })
    }
}

/// Async health check that probes the Docker daemon via `Docker::ping()`.
pub struct ContainerHealthCheck {
    probe: Arc<dyn ContainerHealthProbe>,
    timeout: Duration,
}

impl ContainerHealthCheck {
    /// Creates a new health check using the given endpoint config.
    /// The Docker client is created lazily during the probe.
    pub fn new(config: &ContainerConfig) -> Self {
        Self {
            probe: Arc::new(DockerPingProbe::new(config)),
            timeout: Duration::from_secs(2),
        }
    }

    #[cfg(test)]
    fn with_probe_for_tests(probe: Arc<dyn ContainerHealthProbe>, timeout: Duration) -> Self {
        Self { probe, timeout }
    }
}

#[async_trait]
impl AsyncHealthCheck for ContainerHealthCheck {
    fn name(&self) -> &str {
        "container"
    }

    async fn check(&self) -> CheckResult {
        match tokio::time::timeout(self.timeout, self.probe.probe()).await {
            Ok(Ok(())) => CheckResult::healthy(self.name()),
            Ok(Err(err)) => CheckResult::unhealthy(self.name(), &err.to_string()),
            Err(_) => CheckResult::unhealthy(self.name(), "Docker ping timed out"),
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

    impl ContainerHealthProbe for MockProbe {
        fn probe(&self) -> ProbeFuture {
            (self.responder)()
        }
    }

    #[tokio::test]
    async fn container_health_check_healthy_on_ping_ok() {
        let probe = Arc::new(MockProbe::new(|| Box::pin(async { Ok(()) })));
        let check = ContainerHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "container");
        assert_eq!(result.status, HealthStatus::Healthy);
        assert!(result.message.is_none());
    }

    #[tokio::test]
    async fn container_health_check_unhealthy_on_error() {
        let probe = Arc::new(MockProbe::new(|| {
            Box::pin(async {
                Err(CamelError::ProcessorError(
                    "simulated docker error".to_string(),
                ))
            })
        }));
        let check = ContainerHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "container");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("simulated docker error"))
        );
    }

    #[tokio::test]
    async fn container_health_check_unhealthy_on_timeout() {
        let probe = Arc::new(MockProbe::new(|| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(())
            })
        }));
        let check = ContainerHealthCheck::with_probe_for_tests(probe, Duration::from_millis(5));

        let result = check.check().await;

        assert_eq!(result.name, "container");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert_eq!(result.message.as_deref(), Some("Docker ping timed out"));
    }
}
