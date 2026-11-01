//! `ComponentMetrics` — component-scoped facade for the uniform
//! component-operations family.
//!
//! Each handle fixes its component identity at construction; call sites
//! only supply the operation and outcome. The facade owns two concerns so
//! individual components do not:
//!
//! - **Lever gating:** the effective per-component switch
//!   ([`ComponentMetricsLevers::enabled_for`]) suppresses only the
//!   `camel_component_operations_total` family.
//! - **Unconditional error forwarding:** failures always increment the
//!   non-disableable error family (`camel_errors_total`) as
//!   `increment_errors(component, "e:{component}:{operation}")` — never
//!   lever-gated (metrics-configuration Req 2).
//!
//! [`ComponentMetricsLevers`] is the effective per-component switch
//! snapshot. It arrives pre-resolved because `camel-api` cannot depend on
//! `camel-core`, where `MetricsLeversConfig` lives: the construction site
//! (camel-core, via the `RuntimeObservability` blanket impl) snapshots the
//! current levers into the handle at build time.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::metrics::MetricsCollector;

/// Facade over a [`MetricsCollector`] for uniform component-operation
/// emission. Construct via `RuntimeObservability::component_metrics(component)`
/// or directly in tests.
///
/// Cheap to clone: the collector sits behind an `Arc`, so one facade can
/// be shared across objects built from the same connection (e.g. a
/// repository and its payload store).
#[derive(Clone)]
pub struct ComponentMetrics {
    collector: Arc<dyn MetricsCollector>,
    component: Arc<str>,
    enabled: bool,
}

impl ComponentMetrics {
    /// Builds a facade over `collector` with the component identity fixed
    /// to `component`; `enabled` is the effective per-component switch
    /// ([`ComponentMetricsLevers::enabled_for`]) snapshot taken at
    /// construction time.
    pub fn new(
        collector: Arc<dyn MetricsCollector>,
        component: impl Into<Arc<str>>,
        enabled: bool,
    ) -> Self {
        Self {
            collector,
            component: component.into(),
            enabled,
        }
    }

    /// Returns the component identity fixed at construction.
    pub fn component(&self) -> &str {
        &self.component
    }

    /// Observes one component operation for the fixed identity. `failed`
    /// selects the closed-set outcome label ("failure"/"success") and,
    /// when true, unconditionally forwards to the error family with the
    /// `e:{component}:{operation}` label — error-family emission is never
    /// lever-gated.
    pub fn observe(&self, operation: &str, failed: bool) {
        if failed {
            // allow-open-label rc-otxh (facade builds the e:{component}:{operation} label per ADR-0012; names bounded at observe() call sites)
            self.collector.increment_errors(
                &self.component,
                &format!("e:{}:{operation}", self.component),
            );
        }
        if self.enabled {
            // allow-open-label rc-gm6s (component/operation: identity is fixed at construction and operation is a caller-bounded literal; outcome is a two-literal if/else)
            self.collector.record_component_operation(
                &self.component,
                operation,
                if failed { "failure" } else { "success" },
            );
        }
    }
}

/// Effective per-component switch snapshot for the uniform
/// component-operations family.
///
/// `enabled` is the master switch and `components` is the coarse default;
/// an override wins over `components` in both directions but can never
/// resurrect the family under the master switch:
///
/// ```text
/// enabled_for(C) = enabled && overrides.get(C).unwrap_or(components)
/// ```
///
/// Cloning is cheap: the overrides map is shared behind an `Arc`.
#[derive(Clone, Debug)]
pub struct ComponentMetricsLevers {
    enabled: bool,
    components: bool,
    overrides: Arc<BTreeMap<String, bool>>,
}

impl ComponentMetricsLevers {
    /// Builds a snapshot from the resolved master switch, the component
    /// default, and the per-component overrides.
    pub fn new(enabled: bool, components: bool, overrides: BTreeMap<String, bool>) -> Self {
        Self {
            enabled,
            components,
            overrides: Arc::new(overrides),
        }
    }

    /// Builds a snapshot with no overrides: `components=true` enables the
    /// family for every component, `components=false` disables it
    /// everywhere (including the master switch).
    pub fn uniform(components: bool) -> Self {
        Self {
            enabled: components,
            components,
            overrides: Arc::new(BTreeMap::new()),
        }
    }

    /// Resolves the effective switch for `component`.
    pub fn enabled_for(&self, component: &str) -> bool {
        self.enabled
            && self
                .overrides
                .get(component)
                .copied()
                .unwrap_or(self.components)
    }
}

impl Default for ComponentMetricsLevers {
    fn default() -> Self {
        Self::new(false, false, BTreeMap::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::MetricsCollector;
    use crate::metrics::MetricsHandle;
    use crate::metrics::NoOpMetrics;
    use std::sync::Arc;
    use std::sync::Mutex;
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

    impl RecordingComponentMetrics {
        fn errors(&self) -> Vec<(String, String)> {
            self.errors.lock().expect("errors lock").clone()
        }

        fn ops(&self) -> Vec<(String, String, String)> {
            self.ops.lock().expect("ops lock").clone()
        }
    }

    fn recording() -> (Arc<RecordingComponentMetrics>, Arc<dyn MetricsCollector>) {
        let collector = Arc::new(RecordingComponentMetrics::new());
        let erased = Arc::clone(&collector) as Arc<dyn MetricsCollector>;
        (collector, erased)
    }

    #[test]
    fn scoped_handle_records_fixed_component() {
        let (collector, erased) = recording();
        let metrics = ComponentMetrics::new(erased, "redis", true);

        assert_eq!(metrics.component(), "redis");
        metrics.observe("command", false);
        metrics.observe("command", false);

        assert_eq!(
            collector.ops(),
            vec![
                (
                    "redis".to_string(),
                    "command".to_string(),
                    "success".to_string()
                );
                2
            ],
            "both observations must carry the fixed component identity"
        );
        assert!(
            collector.errors().is_empty(),
            "successful observations must not touch the error family"
        );
    }

    #[test]
    fn lever_off_gates_operations_not_errors() {
        let (collector, erased) = recording();
        let metrics = ComponentMetrics::new(erased, "redis", false);

        metrics.observe("command", true);

        assert_eq!(
            collector.errors(),
            vec![("redis".to_string(), "e:redis:command".to_string())],
            "failure must hit the error family with the lever off"
        );
        assert!(
            collector.ops().is_empty(),
            "component ops must be suppressed with the lever off"
        );
    }

    #[test]
    fn lever_on_records_both_outcomes() {
        let (collector, erased) = recording();
        let metrics = ComponentMetrics::new(erased, "redis", true);

        metrics.observe("command", false);
        metrics.observe("command", true);

        assert_eq!(
            collector.ops(),
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
            "lever on must record both outcomes in order"
        );
        assert_eq!(
            collector.errors(),
            vec![("redis".to_string(), "e:redis:command".to_string())],
            "failure must still increment errors with the lever on"
        );
    }

    #[test]
    fn levers_enabled_for_precedence() {
        let levers =
            ComponentMetricsLevers::new(true, false, BTreeMap::from([("http".to_string(), true)]));
        assert!(
            levers.enabled_for("http"),
            "override must win over components=false"
        );
        assert!(
            !levers.enabled_for("kafka"),
            "absent override inherits components=false"
        );

        let levers =
            ComponentMetricsLevers::new(false, true, BTreeMap::from([("http".to_string(), true)]));
        assert!(
            !levers.enabled_for("http"),
            "master switch off must beat an enabled override"
        );
        assert!(
            !levers.enabled_for("kafka"),
            "master switch off must suppress everything"
        );
    }

    #[test]
    fn levers_uniform_off() {
        let levers = ComponentMetricsLevers::uniform(false);
        assert!(
            !levers.enabled_for("http"),
            "uniform(false) must disable the family for every component"
        );
    }

    #[test]
    fn levers_uniform_on() {
        let levers = ComponentMetricsLevers::uniform(true);
        assert!(levers.enabled_for("http"), "uniform(true) must enable http");
        assert!(
            ComponentMetricsLevers::uniform(true).enabled_for("unlisted"),
            "uniform(true) must inherit components=true for unlisted components"
        );
    }

    #[test]
    fn composite_collector_receives_once() {
        let (first, first_erased) = recording();
        let (second, second_erased) = recording();
        let cell = MetricsHandle::new();
        cell.register(first_erased);
        cell.register(second_erased);
        let metrics =
            ComponentMetrics::new(Arc::new(cell) as Arc<dyn MetricsCollector>, "http", true);

        metrics.observe("consume", true);

        for collector in [&first, &second] {
            assert_eq!(
                collector.ops(),
                vec![(
                    "http".to_string(),
                    "consume".to_string(),
                    "failure".to_string()
                )],
                "each collector must receive exactly one scoped operation"
            );
            assert_eq!(
                collector.errors(),
                vec![("http".to_string(), "e:http:consume".to_string())],
                "each collector must receive exactly one scoped error"
            );
        }
    }

    #[test]
    fn late_binding_reaches_collector_registered_after_first_observe() {
        let cell = Arc::new(MetricsHandle::new());
        let metrics =
            ComponentMetrics::new(Arc::clone(&cell) as Arc<dyn MetricsCollector>, "http", true);

        metrics.observe("consume", false);

        let (collector, erased) = recording();
        cell.register(erased);
        metrics.observe("consume", false);

        assert_eq!(
            collector.ops(),
            vec![(
                "http".to_string(),
                "consume".to_string(),
                "success".to_string()
            )],
            "only the post-registration observation must reach the collector"
        );
        assert!(
            collector.errors().is_empty(),
            "successful observations must not touch the error family"
        );
    }

    #[test]
    fn noop_collector_is_safe() {
        let metrics = ComponentMetrics::new(
            Arc::new(NoOpMetrics) as Arc<dyn MetricsCollector>,
            "redis",
            true,
        );

        metrics.observe("command", false);
        metrics.observe("command", true);
    }
}
