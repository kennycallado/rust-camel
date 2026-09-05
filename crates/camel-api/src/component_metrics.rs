//! `ComponentMetrics` — lever-gated facade for the uniform
//! component-operations family (dashboard-observability Task 4.1).
//!
//! Components call [`ComponentMetrics::observe`] at their principal
//! operation boundary. The facade owns two concerns so individual
//! components do not:
//!
//! - **Lever gating:** the `[observability.metrics].components` lever
//!   (default off, metrics-configuration Req 3) suppresses only the
//!   `camel_component_operations_total` family.
//! - **Unconditional error forwarding:** failures always increment the
//!   non-disableable error family (`camel_errors_total`) as
//!   `increment_errors(component, "e:{component}:{operation}")` — never
//!   lever-gated (metrics-configuration Req 2).
//!
//! The lever arrives as a plain `bool` because `camel-api` cannot depend
//! on `camel-core`, where `MetricsLeversConfig` lives: the construction
//! site (camel-core, via the `RuntimeObservability` blanket impl)
//! snapshots the current levers into the facade at build time.

use std::sync::Arc;

use crate::metrics::MetricsCollector;

/// Facade over a [`MetricsCollector`] for uniform component-operation
/// emission. Construct via `RuntimeObservability::component_metrics()`
/// or directly in tests.
pub struct ComponentMetrics {
    collector: Arc<dyn MetricsCollector>,
    components_enabled: bool,
}

impl ComponentMetrics {
    /// Builds a facade over `collector`; `components_enabled` is the
    /// snapshot of the `[observability.metrics].components` lever taken
    /// at construction time.
    pub fn new(collector: Arc<dyn MetricsCollector>, components_enabled: bool) -> Self {
        Self {
            collector,
            components_enabled,
        }
    }

    /// Observes one component operation. `failed` selects the closed-set
    /// outcome label ("failure"/"success") and, when true, unconditionally
    /// forwards to the error family with the `e:{component}:{operation}`
    /// label — error-family emission is never lever-gated.
    pub fn observe(&self, component: &str, operation: &str, failed: bool) {
        if failed {
            // allow-open-label rc-otxh (facade builds the e:{component}:{operation} label per ADR-0012; names bounded at observe() call sites)
            self.collector
                .increment_errors(component, &format!("e:{component}:{operation}"));
        }
        if self.components_enabled {
            // allow-open-label rc-gm6s (component/operation: caller-bounded literals through the facade; outcome is a two-literal if/else)
            self.collector.record_component_operation(
                component,
                operation,
                if failed { "failure" } else { "success" },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::MetricsCollector;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// Recording double capturing error-family and component-op emissions.
    struct RecordingComponentMetrics {
        errors: Mutex<Vec<(String, String)>>,
        ops: Mutex<Vec<(String, String, String)>>,
    }

    impl RecordingComponentMetrics {
        fn new() -> Self {
            Self {
                errors: Mutex::new(Vec::new()),
                ops: Mutex::new(Vec::new()),
            }
        }
    }

    impl MetricsCollector for RecordingComponentMetrics {
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

    /// Task 4.1: the components lever gates ONLY the component-operations
    /// family; error-family forwarding is unconditional.
    #[test]
    fn facade_gates_components_not_errors() {
        // Lever OFF: a failed observe forwards to the error family and
        // records no component-op.
        let off_collector = Arc::new(RecordingComponentMetrics::new());
        let off = ComponentMetrics::new(
            Arc::clone(&off_collector) as Arc<dyn MetricsCollector>,
            false,
        );
        off.observe("redis", "command", true);
        assert_eq!(
            off_collector.errors.lock().expect("errors lock").clone(),
            vec![("redis".to_string(), "e:redis:command".to_string())],
            "failure must hit the error family with the lever off"
        );
        assert!(
            off_collector.ops.lock().expect("ops lock").is_empty(),
            "component ops must be suppressed with the lever off"
        );

        // Lever ON: both outcomes recorded; the failure ALSO increments
        // errors (error family is never lever-gated).
        let on_collector = Arc::new(RecordingComponentMetrics::new());
        let on =
            ComponentMetrics::new(Arc::clone(&on_collector) as Arc<dyn MetricsCollector>, true);
        on.observe("redis", "command", false);
        on.observe("redis", "command", true);
        assert_eq!(
            on_collector.ops.lock().expect("ops lock").clone(),
            vec![
                (
                    "redis".to_string(),
                    "command".to_string(),
                    "success".to_string()
                ),
                (
                    "redis".to_string(),
                    "command".to_string(),
                    "failure".to_string()
                ),
            ],
            "lever on must record both outcomes"
        );
        assert_eq!(
            on_collector.errors.lock().expect("errors lock").clone(),
            vec![("redis".to_string(), "e:redis:command".to_string())],
            "failure must still increment errors with the lever on"
        );
    }
}
