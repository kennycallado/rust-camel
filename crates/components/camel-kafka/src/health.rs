use crate::config::ResolvedKafkaEndpointConfig;
use crate::producer::KafkaProducer;
use async_trait::async_trait;
use camel_api::{AsyncHealthCheck, CheckResult};
use camel_component_api::CamelError;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

type ProbeFuture = Pin<Box<dyn Future<Output = Result<(), CamelError>> + Send>>;

trait KafkaMetadataProbe: Send + Sync {
    fn probe(&self) -> ProbeFuture;
}

struct KafkaClientMetadataProbe {
    config: ResolvedKafkaEndpointConfig,
    timeout: Duration,
}

impl KafkaClientMetadataProbe {
    fn new(config: ResolvedKafkaEndpointConfig, timeout: Duration) -> Self {
        Self { config, timeout }
    }
}

impl KafkaMetadataProbe for KafkaClientMetadataProbe {
    fn probe(&self) -> ProbeFuture {
        let config = self.config.clone();
        let timeout = self.timeout;
        Box::pin(async move { KafkaProducer::metadata_probe(&config, timeout) })
    }
}

pub struct KafkaHealthCheck {
    probe: Arc<dyn KafkaMetadataProbe>,
    timeout: Duration,
}

impl KafkaHealthCheck {
    pub fn new(config: ResolvedKafkaEndpointConfig) -> Self {
        let timeout = Duration::from_secs(5);
        Self {
            probe: Arc::new(KafkaClientMetadataProbe::new(config, timeout)),
            timeout,
        }
    }

    #[cfg(test)]
    fn with_probe_for_tests(probe: Arc<dyn KafkaMetadataProbe>, timeout: Duration) -> Self {
        Self { probe, timeout }
    }
}

#[async_trait]
impl AsyncHealthCheck for KafkaHealthCheck {
    fn name(&self) -> &str {
        "kafka"
    }

    async fn check(&self) -> CheckResult {
        match tokio::time::timeout(self.timeout, self.probe.probe()).await {
            Ok(Ok(())) => CheckResult::healthy(self.name()),
            Ok(Err(err)) => CheckResult::unhealthy(self.name(), &err.to_string()),
            Err(_) => CheckResult::unhealthy(self.name(), "metadata probe timed out"),
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

    impl KafkaMetadataProbe for MockProbe {
        fn probe(&self) -> ProbeFuture {
            (self.responder)()
        }
    }

    #[tokio::test]
    async fn kafka_health_check_healthy_when_metadata_probe_succeeds() {
        let probe = Arc::new(MockProbe::new(|| Box::pin(async { Ok(()) })));
        let check = KafkaHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "kafka");
        assert_eq!(result.status, HealthStatus::Healthy);
        assert!(result.message.is_none());
    }

    #[tokio::test]
    async fn kafka_health_check_unhealthy_when_metadata_probe_fails() {
        let probe = Arc::new(MockProbe::new(|| {
            Box::pin(async {
                Err(CamelError::ProcessorError(
                    "simulated kafka metadata error".to_string(),
                ))
            })
        }));
        let check = KafkaHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "kafka");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("simulated kafka metadata error"))
        );
    }

    #[tokio::test]
    async fn kafka_health_check_unhealthy_when_metadata_probe_times_out() {
        let probe = Arc::new(MockProbe::new(|| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(())
            })
        }));
        let check = KafkaHealthCheck::with_probe_for_tests(probe, Duration::from_millis(5));

        let result = check.check().await;

        assert_eq!(result.name, "kafka");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert_eq!(result.message.as_deref(), Some("metadata probe timed out"));
    }
}
