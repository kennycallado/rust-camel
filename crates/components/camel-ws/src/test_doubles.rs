//! Shared test doubles for the camel-ws crate (compiled only under `cargo test`).

use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Recording metrics collector: captures retry-attempt and error-count calls
/// so tests can assert the retry policy actually attempted N times and
/// dispatch failures were counted (pattern: camel-component-api
/// network_retry_tests::RecordingMetrics).
pub(crate) struct RecordingMetrics {
    pub(crate) attempts: Mutex<Vec<(String, String)>>,
    pub(crate) errors: Arc<Mutex<Vec<(String, String)>>>,
    pub(crate) component_ops: Mutex<Vec<(String, String, String)>>,
}

impl RecordingMetrics {
    pub(crate) fn new() -> Self {
        Self {
            attempts: Mutex::new(Vec::new()),
            errors: Arc::new(Mutex::new(Vec::new())),
            component_ops: Mutex::new(Vec::new()),
        }
    }

    /// Build a collector that records errors into a caller-owned `Arc`, so the
    /// test can assert on the recorded errors after the consumer runs.
    pub(crate) fn with_errors(errors: Arc<Mutex<Vec<(String, String)>>>) -> Self {
        Self {
            attempts: Mutex::new(Vec::new()),
            errors,
            component_ops: Mutex::new(Vec::new()),
        }
    }
}

impl camel_api::MetricsCollector for RecordingMetrics {
    fn record_exchange_duration(&self, _route_id: &str, _duration: Duration) {}
    fn increment_errors(&self, route_id: &str, error_type: &str) {
        self.errors
            .lock()
            .expect("recording collector lock")
            .push((route_id.to_string(), error_type.to_string()));
    }
    fn increment_exchanges(&self, _route_id: &str) {}
    fn set_queue_depth(&self, _queue: &str, _depth: usize) {}
    fn record_circuit_breaker_change(&self, _route_id: &str, _from: &str, _to: &str) {}
    fn increment_retry_attempt(&self, scheme: &str, operation: &str) {
        self.attempts
            .lock()
            .unwrap()
            .push((scheme.to_string(), operation.to_string()));
    }
    fn record_component_operation(&self, component: &str, operation: &str, outcome: &str) {
        self.component_ops
            .lock()
            .expect("recording collector lock")
            .push((
                component.to_string(),
                operation.to_string(),
                outcome.to_string(),
            ));
    }
}

/// `RuntimeObservability` test double over [`RecordingMetrics`]. Overrides
/// `component_metrics()` with the components lever ON: the trait default
/// passes lever=false, which SUPPRESSES the component-operation success
/// series — without this override the facade emits nothing observable.
pub(crate) struct CountingRuntime {
    pub(crate) metrics: Arc<RecordingMetrics>,
}

impl camel_component_api::RuntimeObservability for CountingRuntime {
    fn metrics(&self) -> Arc<dyn camel_api::MetricsCollector> {
        let collector: Arc<dyn camel_api::MetricsCollector> = self.metrics.clone();
        collector
    }
    fn health(&self) -> Arc<dyn camel_component_api::HealthCheckRegistry> {
        Arc::new(camel_component_api::NoOpHealthCheckRegistry)
    }
    fn component_metrics(&self) -> camel_api::ComponentMetrics {
        let collector: Arc<dyn camel_api::MetricsCollector> = self.metrics.clone();
        camel_api::ComponentMetrics::new(collector, true)
    }
}
