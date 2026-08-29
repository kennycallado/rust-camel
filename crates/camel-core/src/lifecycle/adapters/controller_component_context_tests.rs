use super::*;

/// Minimal double recording component-operation calls.
struct OpsRecorder(std::sync::Mutex<Vec<String>>);

impl MetricsCollector for OpsRecorder {
    fn increment_errors(&self, _route_id: &str, _error_type: &str) {}
    fn record_exchange_duration(&self, _route_id: &str, _seconds: std::time::Duration) {}
    fn increment_exchanges(&self, _route_id: &str) {}
    fn set_queue_depth(&self, _queue: &str, _depth: usize) {}
    fn record_circuit_breaker_change(&self, _route: &str, _from: &str, _to: &str) {}
    fn record_component_operation(&self, component: &str, operation: &str, outcome: &str) {
        self.0
            .lock()
            .expect("ops lock") // allow-unwrap
            .push(format!("{component}:{operation}:{outcome}"));
    }
}

fn build_ctx(enabled: bool) -> Arc<ControllerComponentContext> {
    Arc::new(ControllerComponentContext::new(
        Arc::new(std::sync::Mutex::new(Registry::new())),
        Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        Arc::new(OpsRecorder(std::sync::Mutex::new(Vec::new()))),
        Arc::new(camel_api::NoopPlatformService::default()),
        Arc::new(crate::health_registry::HealthCheckRegistry::new(
            std::time::Duration::from_secs(30),
        )),
        Some("r".to_string()),
        enabled,
    ))
}

/// Task 4.1 review finding: the production controller path must thread
/// the `[observability.metrics].components` lever snapshot through
/// `ControllerComponentContext` — endpoints reach `component_metrics()`
/// through this impl, not through `CamelContext`.
#[test]
fn controller_path_component_metrics_reflects_lever() {
    let on = build_ctx(true);
    assert!(
        on.component_metrics_enabled(),
        "enabled snapshot must surface on the ComponentContext seam"
    );
    let recorder = Arc::new(OpsRecorder(std::sync::Mutex::new(Vec::new())));
    let facade =
        camel_api::ComponentMetrics::new(Arc::clone(&recorder) as Arc<dyn MetricsCollector>, true);
    facade.observe("wasm", "invoke", true);
    let calls = recorder.0.lock().expect("ops lock").clone();
    assert_eq!(
        calls,
        vec!["wasm:invoke:failure"],
        "lever-on facade must record the component operation"
    );

    let off = build_ctx(false);
    assert!(
        !off.component_metrics_enabled(),
        "disabled snapshot must stay off (opt-in default)"
    );
}
