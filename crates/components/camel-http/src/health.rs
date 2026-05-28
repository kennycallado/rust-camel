use async_trait::async_trait;
use camel_api::{AsyncHealthCheck, CheckResult};
use camel_component_api::CamelError;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

type ProbeFuture = Pin<Box<dyn Future<Output = Result<(), CamelError>> + Send>>;

trait HttpHealthProbe: Send + Sync {
    fn probe(&self) -> ProbeFuture;
}

struct HttpListenerProbe {
    host: String,
    port: u16,
}

impl HttpListenerProbe {
    fn new(host: String, port: u16) -> Self {
        let probe_host = match host.as_str() {
            "0.0.0.0" => "127.0.0.1".to_string(),
            "::" | "[::]" => "::1".to_string(),
            _ => host,
        };
        Self {
            host: probe_host,
            port,
        }
    }
}

impl HttpHealthProbe for HttpListenerProbe {
    fn probe(&self) -> ProbeFuture {
        let addr = format!("{}:{}", self.host, self.port);
        Box::pin(async move {
            tokio::net::TcpStream::connect(&addr)
                .await
                .map(|_| ())
                .map_err(|e| {
                    CamelError::ProcessorError(format!(
                        "HTTP listener health check failed for '{}': {}",
                        addr, e
                    ))
                })
        })
    }
}

pub struct HttpHealthCheck {
    probe: Arc<dyn HttpHealthProbe>,
    timeout: Duration,
}

impl HttpHealthCheck {
    pub fn new(host: String, port: u16) -> Self {
        Self {
            probe: Arc::new(HttpListenerProbe::new(host, port)),
            timeout: Duration::from_secs(3),
        }
    }

    #[cfg(test)]
    fn with_probe_for_tests(probe: Arc<dyn HttpHealthProbe>, timeout: Duration) -> Self {
        Self { probe, timeout }
    }
}

#[async_trait]
impl AsyncHealthCheck for HttpHealthCheck {
    fn name(&self) -> &str {
        "http"
    }

    async fn check(&self) -> CheckResult {
        match tokio::time::timeout(self.timeout, self.probe.probe()).await {
            Ok(Ok(())) => CheckResult::healthy(self.name()),
            Ok(Err(err)) => CheckResult::unhealthy(self.name(), &err.to_string()),
            Err(_) => CheckResult::unhealthy(self.name(), "HTTP listener probe timed out"),
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

    impl HttpHealthProbe for MockProbe {
        fn probe(&self) -> ProbeFuture {
            (self.responder)()
        }
    }

    #[tokio::test]
    async fn http_health_check_healthy_when_probe_succeeds() {
        let probe = Arc::new(MockProbe::new(|| Box::pin(async { Ok(()) })));
        let check = HttpHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "http");
        assert_eq!(result.status, HealthStatus::Healthy);
        assert!(result.message.is_none());
    }

    #[tokio::test]
    async fn http_health_check_unhealthy_when_probe_fails() {
        let probe = Arc::new(MockProbe::new(|| {
            Box::pin(async { Err(CamelError::ProcessorError("listener not bound".to_string())) })
        }));
        let check = HttpHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "http");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("listener not bound"))
        );
    }

    #[tokio::test]
    async fn http_health_check_unhealthy_when_probe_times_out() {
        let probe = Arc::new(MockProbe::new(|| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(())
            })
        }));
        let check = HttpHealthCheck::with_probe_for_tests(probe, Duration::from_millis(5));

        let result = check.check().await;

        assert_eq!(result.name, "http");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert_eq!(
            result.message.as_deref(),
            Some("HTTP listener probe timed out")
        );
    }
}
