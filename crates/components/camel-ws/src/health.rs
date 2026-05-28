use async_trait::async_trait;
use camel_api::{AsyncHealthCheck, CheckResult};
use camel_component_api::CamelError;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

type ProbeFuture = Pin<Box<dyn Future<Output = Result<(), CamelError>> + Send>>;

trait WsHealthProbe: Send + Sync {
    fn probe(&self) -> ProbeFuture;
}

struct TcpListenerProbe {
    host: String,
    port: u16,
}

impl TcpListenerProbe {
    fn new(host: String, port: u16) -> Self {
        let probe_host = match host.as_str() {
            "0.0.0.0" | "localhost" => "127.0.0.1".to_string(),
            "::" | "[::]" => "::1".to_string(),
            _ => host,
        };
        Self {
            host: probe_host,
            port,
        }
    }
}

impl WsHealthProbe for TcpListenerProbe {
    fn probe(&self) -> ProbeFuture {
        let addr = format!("{}:{}", self.host, self.port);
        Box::pin(async move {
            tokio::net::TcpStream::connect(&addr)
                .await
                .map(|_| ())
                .map_err(|e| {
                    CamelError::ProcessorError(format!(
                        "WebSocket listener health check failed for '{}': {}",
                        addr, e
                    ))
                })
        })
    }
}

pub struct WsHealthCheck {
    probe: Box<dyn WsHealthProbe>,
    timeout: Duration,
}

impl WsHealthCheck {
    pub fn new(host: String, port: u16) -> Self {
        Self {
            probe: Box::new(TcpListenerProbe::new(host, port)),
            timeout: Duration::from_secs(3),
        }
    }

    #[cfg(test)]
    fn with_probe_for_tests(probe: Box<dyn WsHealthProbe>, timeout: Duration) -> Self {
        Self { probe, timeout }
    }
}

#[async_trait]
impl AsyncHealthCheck for WsHealthCheck {
    fn name(&self) -> &str {
        "ws"
    }

    async fn check(&self) -> CheckResult {
        match tokio::time::timeout(self.timeout, self.probe.probe()).await {
            Ok(Ok(())) => CheckResult::healthy(self.name()),
            Ok(Err(err)) => CheckResult::unhealthy(self.name(), &err.to_string()),
            Err(_) => CheckResult::unhealthy(self.name(), "WebSocket listener probe timed out"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::HealthStatus;

    struct MockProbe {
        responder: std::sync::Arc<dyn Fn() -> ProbeFuture + Send + Sync>,
    }

    impl MockProbe {
        fn new<F>(f: F) -> Self
        where
            F: Fn() -> ProbeFuture + Send + Sync + 'static,
        {
            Self {
                responder: std::sync::Arc::new(f),
            }
        }
    }

    impl WsHealthProbe for MockProbe {
        fn probe(&self) -> ProbeFuture {
            (self.responder)()
        }
    }

    #[tokio::test]
    async fn ws_health_check_healthy_when_probe_succeeds() {
        let probe = Box::new(MockProbe::new(|| Box::pin(async { Ok(()) })));
        let check = WsHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));
        let result = check.check().await;
        assert_eq!(result.name, "ws");
        assert_eq!(result.status, HealthStatus::Healthy);
        assert!(result.message.is_none());
    }

    #[tokio::test]
    async fn ws_health_check_unhealthy_when_probe_fails() {
        let probe = Box::new(MockProbe::new(|| {
            Box::pin(async { Err(CamelError::ProcessorError("simulated ws error".to_string())) })
        }));
        let check = WsHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));
        let result = check.check().await;
        assert_eq!(result.name, "ws");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("simulated ws error"))
        );
    }

    #[tokio::test]
    async fn ws_health_check_unhealthy_when_probe_times_out() {
        let probe = Box::new(MockProbe::new(|| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(())
            })
        }));
        let check = WsHealthCheck::with_probe_for_tests(probe, Duration::from_millis(5));
        let result = check.check().await;
        assert_eq!(result.name, "ws");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert_eq!(
            result.message.as_deref(),
            Some("WebSocket listener probe timed out")
        );
    }
}
