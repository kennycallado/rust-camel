//! Recording [`MetricsCollector`] test double for the component-operations
//! and error families.
//!
//! Shared by the cache and idempotent repository test modules so both can
//! assert on exact `ComponentMetrics` emission without a live collector.
//! The component-operations lever is owned by [`camel_api::ComponentMetrics`];
//! this double records whatever the facade forwards, so a lever-off facade
//! produces error entries only.

use camel_api::metrics::MetricsCollector;
use std::sync::Mutex;
use std::time::Duration;

/// Captures `increment_errors` and `record_component_operation` calls.
pub(crate) struct RecordingMetrics {
    ops: Mutex<Vec<(String, String, String)>>,
    errors: Mutex<Vec<(String, String)>>,
}

impl RecordingMetrics {
    /// Create an empty recorder.
    pub(crate) fn new() -> Self {
        Self {
            ops: Mutex::new(Vec::new()),
            errors: Mutex::new(Vec::new()),
        }
    }

    /// Recorded component operations as `"component:operation:outcome"`.
    pub(crate) fn ops(&self) -> Vec<String> {
        self.ops
            .lock()
            .expect("ops lock")
            .iter()
            .map(|(component, operation, outcome)| format!("{component}:{operation}:{outcome}"))
            .collect()
    }

    /// Recorded error-family entries as `(route_id, error_type)` clones.
    pub(crate) fn errors(&self) -> Vec<(String, String)> {
        self.errors.lock().expect("errors lock").clone()
    }
}

impl Default for RecordingMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsCollector for RecordingMetrics {
    fn record_exchange_duration(&self, _route_id: &str, _duration: Duration) {}

    fn increment_errors(&self, route_id: &str, error_type: &str) {
        self.errors
            .lock()
            .expect("errors lock")
            .push((route_id.to_string(), error_type.to_string()));
    }

    fn increment_exchanges(&self, _route_id: &str) {}

    fn set_queue_depth(&self, _queue: &str, _depth: usize) {}

    fn record_circuit_breaker_change(&self, _route_id: &str, _from: &str, _to: &str) {}

    fn record_component_operation(&self, component: &str, operation: &str, outcome: &str) {
        self.ops.lock().expect("ops lock").push((
            component.to_string(),
            operation.to_string(),
            outcome.to_string(),
        ));
    }
}
