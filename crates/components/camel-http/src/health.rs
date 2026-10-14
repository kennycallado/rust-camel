use async_trait::async_trait;
use camel_api::{AsyncHealthCheck, CheckResult};
use camel_component_api::CamelError;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// Marks an `HttpEndpoint` as a listener (a consumer was created from it).
///
/// `HttpComponent::create_endpoint` cannot tell whether the endpoint will be
/// a consumer (`from`) or a producer (`to`), so it registers the check for
/// both. The gate keeps the check inert until `create_consumer` opens it:
/// a producer's URI names a remote target, and probing that target makes
/// `/readyz` depend on a downstream service (rc-hcica).
#[derive(Clone, Default)]
pub(crate) struct ListenerGate(Arc<AtomicBool>);

impl ListenerGate {
    pub(crate) fn open(&self) {
        self.0.store(true, Ordering::Release);
    }

    fn is_open(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

pub struct HttpHealthCheck {
    probe: Arc<dyn HttpHealthProbe>,
    timeout: Duration,
    /// `None`: always probe (public `new`). `Some`: probe only once opened.
    gate: Option<ListenerGate>,
}

impl HttpHealthCheck {
    pub fn new(host: String, port: u16) -> Self {
        Self {
            probe: Arc::new(HttpListenerProbe::new(host, port)),
            timeout: Duration::from_secs(3),
            gate: None,
        }
    }

    /// Listener check that stays healthy until `gate` is opened.
    pub(crate) fn gated(host: String, port: u16, gate: ListenerGate) -> Self {
        Self {
            gate: Some(gate),
            ..Self::new(host, port)
        }
    }

    #[cfg(test)]
    fn with_probe_for_tests(probe: Arc<dyn HttpHealthProbe>, timeout: Duration) -> Self {
        Self {
            probe,
            timeout,
            gate: None,
        }
    }
}

#[async_trait]
impl AsyncHealthCheck for HttpHealthCheck {
    fn name(&self) -> &str {
        "http"
    }

    async fn check(&self) -> CheckResult {
        if self.gate.as_ref().is_some_and(|g| !g.is_open()) {
            // Producer-only endpoint: no listener of ours to probe.
            return CheckResult::healthy(self.name());
        }
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

    // Timeout-semantics tests run under `start_paused = true` (rc-wsx2y):
    // the virtual clock removes wall-clock dependence — the probe's 50ms
    // sleep and the 5ms check timeout resolve instantly via auto-advance.
    // Caveat: this proves the timeout SEMANTICS on tokio's paused clock; the
    // real-time path is not exercised here (rc-8cw1 forensics: the one-time
    // red there was OOM attribution, not an assertion race).

    #[tokio::test(start_paused = true)]
    async fn http_health_check_healthy_when_probe_succeeds() {
        let probe = Arc::new(MockProbe::new(|| Box::pin(async { Ok(()) })));
        let check = HttpHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "http");
        assert_eq!(result.status, HealthStatus::Healthy);
        assert!(result.message.is_none());
    }

    #[tokio::test(start_paused = true)]
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

    fn failing_probe() -> Arc<MockProbe> {
        Arc::new(MockProbe::new(|| {
            Box::pin(async { Err(CamelError::ProcessorError("remote down".to_string())) })
        }))
    }

    /// rc-hcica: a producer endpoint never opens its gate, so a dead remote
    /// must not turn readiness unhealthy.
    #[tokio::test(start_paused = true)]
    async fn gated_check_is_healthy_until_a_consumer_opens_the_gate() {
        let gate = ListenerGate::default();
        let mut check =
            HttpHealthCheck::with_probe_for_tests(failing_probe(), Duration::from_millis(50));
        check.gate = Some(gate.clone());

        assert_eq!(check.check().await.status, HealthStatus::Healthy);

        gate.open();
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("remote down"))
        );
    }

    #[tokio::test(start_paused = true)]
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
