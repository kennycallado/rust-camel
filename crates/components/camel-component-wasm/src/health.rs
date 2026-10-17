use async_trait::async_trait;
use camel_api::{AsyncHealthCheck, CamelError, CheckResult};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

type ProbeFuture = Pin<Box<dyn Future<Output = Result<(), CamelError>> + Send>>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum LoadPhase {
    #[default]
    NotLoaded,
    Loaded,
    Failed(String),
}

/// Load state of one `wasm:` endpoint's module, shared between the
/// endpoint's health check and its producer / source consumer. Modules load
/// lazily (producer: first exchange) or at route start (source consumer), so
/// the state starts `NotLoaded`.
#[derive(Clone, Debug, Default)]
pub(crate) struct WasmLoadState(Arc<Mutex<LoadPhase>>);

impl WasmLoadState {
    fn set(&self, phase: LoadPhase) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = phase;
    }

    fn get(&self) -> LoadPhase {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub(crate) fn set_loaded(&self) {
        self.set(LoadPhase::Loaded);
    }

    pub(crate) fn set_failed(&self, reason: impl Into<String>) {
        self.set(LoadPhase::Failed(reason.into()));
    }
}

/// Trait for probing the WASM engine state.
trait WasmEngineProbe: Send + Sync {
    fn probe(&self) -> ProbeFuture;

    /// True while the module has not been loaded yet. Readiness reports this
    /// as Degraded (200) so an unused route does not hold the pod out of
    /// rotation.
    fn not_started(&self) -> bool {
        false
    }
}

/// Real probe that reads the endpoint's module load state.
struct WasmEngineStateProbe {
    state: WasmLoadState,
}

impl WasmEngineStateProbe {
    fn new(state: WasmLoadState) -> Self {
        Self { state }
    }
}

impl WasmEngineProbe for WasmEngineStateProbe {
    fn not_started(&self) -> bool {
        self.state.get() == LoadPhase::NotLoaded
    }

    fn probe(&self) -> ProbeFuture {
        let phase = self.state.get();
        Box::pin(async move {
            match phase {
                LoadPhase::Loaded => Ok(()),
                LoadPhase::NotLoaded => Err(CamelError::ComponentNotFound(
                    "WASM module not loaded".to_string(),
                )),
                LoadPhase::Failed(reason) => Err(CamelError::ProcessorError(format!(
                    "WASM module failed to load: {reason}"
                ))),
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
    pub(crate) fn new(state: WasmLoadState) -> Self {
        let timeout = Duration::from_secs(5);
        Self {
            probe: Arc::new(WasmEngineStateProbe::new(state)),
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
        if self.probe.not_started() {
            return CheckResult::degraded(self.name(), "module not loaded (loads on first use)");
        }
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
    async fn wasm_health_check_follows_load_state() {
        let state = WasmLoadState::default();
        let check = WasmHealthCheck::new(state.clone());

        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Degraded);

        state.set_loaded();
        assert_eq!(check.check().await.status, HealthStatus::Healthy);

        state.set_failed("bad magic");
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("bad magic"))
        );
    }

    #[tokio::test]
    async fn wasm_load_state_is_per_endpoint() {
        let a = WasmLoadState::default();
        let b = WasmLoadState::default();
        a.set_loaded();
        assert_eq!(
            WasmHealthCheck::new(a).check().await.status,
            HealthStatus::Healthy
        );
        assert_eq!(
            WasmHealthCheck::new(b).check().await.status,
            HealthStatus::Degraded
        );
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
