use async_trait::async_trait;
use camel_api::{AsyncHealthCheck, CamelError, CheckResult};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

type ProbeFuture = Pin<Box<dyn Future<Output = Result<(), CamelError>> + Send>>;

/// Trait for probing the WASM engine state.
trait WasmEngineProbe: Send + Sync {
    fn probe(&self) -> ProbeFuture;
}

/// Real probe that checks an `AtomicBool` flag indicating whether the WASM module is loaded.
struct WasmEngineStateProbe {
    loaded: Arc<AtomicBool>,
}

impl WasmEngineStateProbe {
    fn new(loaded: Arc<AtomicBool>) -> Self {
        Self { loaded }
    }
}

impl WasmEngineProbe for WasmEngineStateProbe {
    fn probe(&self) -> ProbeFuture {
        let loaded = Arc::clone(&self.loaded);
        Box::pin(async move {
            if loaded.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err(CamelError::ComponentNotFound(
                    "WASM engine module not loaded".to_string(),
                ))
            }
        })
    }
}

/// Health check for the WASM component that probes the engine state flag.
pub struct WasmHealthCheck {
    probe: Arc<dyn WasmEngineProbe>,
    timeout: Duration,
}

impl WasmHealthCheck {
    pub fn new(loaded: Arc<AtomicBool>) -> Self {
        let timeout = Duration::from_secs(5);
        Self {
            probe: Arc::new(WasmEngineStateProbe::new(loaded)),
            timeout,
        }
    }

    #[cfg(test)]
    fn with_probe_for_tests(probe: Arc<dyn WasmEngineProbe>, timeout: Duration) -> Self {
        Self { probe, timeout }
    }
}

#[async_trait]
impl AsyncHealthCheck for WasmHealthCheck {
    fn name(&self) -> &str {
        "wasm"
    }

    async fn check(&self) -> CheckResult {
        match tokio::time::timeout(self.timeout, self.probe.probe()).await {
            Ok(Ok(())) => CheckResult::healthy(self.name()),
            Ok(Err(err)) => CheckResult::unhealthy(self.name(), &err.to_string()),
            Err(_) => CheckResult::unhealthy(self.name(), "engine state probe timed out"),
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

    impl WasmEngineProbe for MockProbe {
        fn probe(&self) -> ProbeFuture {
            (self.responder)()
        }
    }

    #[tokio::test]
    async fn wasm_health_check_healthy_when_engine_loaded() {
        let probe = Arc::new(MockProbe::new(|| Box::pin(async { Ok(()) })));
        let check = WasmHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "wasm");
        assert_eq!(result.status, HealthStatus::Healthy);
        assert!(result.message.is_none());
    }

    #[tokio::test]
    async fn wasm_health_check_unhealthy_when_engine_not_loaded() {
        let probe = Arc::new(MockProbe::new(|| {
            Box::pin(async {
                Err(CamelError::ComponentNotFound(
                    "WASM engine module not loaded".to_string(),
                ))
            })
        }));
        let check = WasmHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "wasm");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("WASM engine module not loaded"))
        );
    }

    #[tokio::test]
    async fn wasm_health_check_unhealthy_on_timeout() {
        let probe = Arc::new(MockProbe::new(|| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(())
            })
        }));
        let check = WasmHealthCheck::with_probe_for_tests(probe, Duration::from_millis(5));

        let result = check.check().await;

        assert_eq!(result.name, "wasm");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert_eq!(
            result.message.as_deref(),
            Some("engine state probe timed out")
        );
    }
}
