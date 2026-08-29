use std::collections::HashMap;
use std::sync::Arc;

use camel_api::CamelError;
use camel_api::component_metadata::ComponentMetadata;
use camel_component_api::Component;

/// Registry that stores components by their URI scheme.
///
/// Also harvests and indexes [`ComponentMetadata`] for each registered
/// component, so the metadata can be queried through a
/// [`ComponentMetadataCatalog`](camel_api::component_metadata::ComponentMetadataCatalog)
/// without re-invoking the component.
pub struct Registry {
    components: HashMap<String, Arc<dyn Component>>,
    metadata: HashMap<String, ComponentMetadata>,
}

impl Registry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            components: HashMap::new(),
            metadata: HashMap::new(),
        }
    }

    /// Register a component. Replaces any existing component with the same scheme.
    ///
    /// Harvests the component's [`ComponentMetadata`] and indexes it by scheme
    /// in parallel with the component insertion. Validates that the metadata's
    /// scheme matches the component's scheme, normalizing on mismatch with a
    /// warning log.
    pub fn register(&mut self, component: Arc<dyn Component>) {
        let scheme = component.scheme().to_string();
        let mut metadata = component.metadata();
        if let Err(e) = metadata.validate_scheme(&scheme) {
            tracing::warn!(scheme = %scheme, error = %e, "metadata scheme mismatch, normalizing");
            metadata.scheme = scheme.clone();
        }
        self.metadata.insert(scheme.clone(), metadata);
        self.components.insert(scheme, component);
    }

    /// Look up a component by scheme.
    pub fn get(&self, scheme: &str) -> Option<Arc<dyn Component>> {
        self.components.get(scheme).cloned()
    }

    /// Look up a component by scheme, returning an error if not found.
    pub fn get_or_err(&self, scheme: &str) -> Result<Arc<dyn Component>, CamelError> {
        self.get(scheme)
            .ok_or_else(|| CamelError::ComponentNotFound(scheme.to_string()))
    }

    /// Look up harvested metadata for a component by scheme.
    pub fn get_metadata(&self, scheme: &str) -> Option<ComponentMetadata> {
        self.metadata.get(scheme).cloned()
    }

    /// Return metadata for every registered component.
    pub fn all_metadata(&self) -> Vec<ComponentMetadata> {
        self.metadata.values().cloned().collect()
    }

    /// Return the schemes of every registered component's metadata.
    pub fn metadata_schemes(&self) -> Vec<String> {
        self.metadata.keys().cloned().collect()
    }

    /// Returns the number of registered components.
    pub fn len(&self) -> usize {
        self.components.len()
    }

    /// Returns true if no components are registered.
    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

/// Adapter that lets `Registry` participate as a `ComponentContext`.
///
/// Wraps the shared `Arc<Mutex<Registry>>` and delegates `resolve_component`
/// to `Registry::get`. The metrics collector is threaded in from the
/// composition root (camel-cli) at construction; it is the ADR-0066
/// late-bound handle, not a backend snapshot, so late registrations flow
/// through [`camel_component_api::ComponentContext::metrics`] without
/// re-snapshotting. The
/// components-lever snapshot gates only the component-operations family —
/// the error family is never lever-gated. When no collector is wired
/// (e.g. compile-time security scan, standalone examples without a
/// live context), construction resolves
/// [`camel_api::NoOpMetrics`].
pub struct RegistryComponentContext {
    registry: Arc<std::sync::Mutex<Registry>>,
    metrics: Arc<dyn camel_api::MetricsCollector>,
    components_enabled: bool,
}

impl RegistryComponentContext {
    /// Builds the context, resolving the collector once: `metrics` when
    /// wired, `NoOpMetrics` otherwise.
    pub fn new(
        registry: Arc<std::sync::Mutex<Registry>>,
        metrics: Option<Arc<dyn camel_api::MetricsCollector>>,
        components_enabled: bool,
    ) -> Self {
        Self {
            registry,
            metrics: metrics.unwrap_or_else(|| Arc::new(camel_api::NoOpMetrics)),
            components_enabled,
        }
    }
}

impl camel_component_api::ComponentContext for RegistryComponentContext {
    fn resolve_component(&self, scheme: &str) -> Option<Arc<dyn camel_component_api::Component>> {
        self.registry.lock().ok()?.get(scheme)
    }

    fn resolve_language(&self, _name: &str) -> Option<Arc<dyn camel_language_api::Language>> {
        None
    }

    fn metrics(&self) -> Arc<dyn camel_api::MetricsCollector> {
        self.metrics.clone()
    }

    fn component_metrics_enabled(&self) -> bool {
        self.components_enabled
    }

    fn platform_service(&self) -> Arc<dyn camel_api::PlatformService> {
        Arc::new(camel_api::NoopPlatformService::default())
    }

    fn register_route_health_check(
        &self,
        _route_id: &str,
        _check: Arc<dyn camel_api::AsyncHealthCheck>,
    ) {
    }

    fn unregister_route_health_check(&self, _route_id: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use camel_api::MetricsCollector;
    use camel_api::component_metadata::ComponentMetadata;
    use camel_component_api::{ComponentContext, RuntimeObservability};
    use camel_component_log::LogComponent;
    use camel_component_timer::TimerComponent;

    /// Recording state owned by the [`RecordingMetrics`] double. Owned
    /// `String`s throughout — the facade passes formatted labels.
    struct RecordingState {
        errors: Vec<(String, String)>,
        component_ops: Vec<(String, String, String)>,
        counters: Vec<(String, f64)>,
    }

    /// Local recording double capturing the families the registry context
    /// can emit: error pairs, component-op triples, generic counters. All
    /// other trait methods are empty.
    struct RecordingMetrics {
        state: Arc<std::sync::Mutex<RecordingState>>,
    }

    impl RecordingMetrics {
        fn new() -> Self {
            Self {
                state: Arc::new(std::sync::Mutex::new(RecordingState {
                    errors: Vec::new(),
                    component_ops: Vec::new(),
                    counters: Vec::new(),
                })),
            }
        }

        fn recorded_errors(&self) -> Vec<(String, String)> {
            self.state
                .lock()
                .expect("recording state lock")
                .errors
                .clone()
        }

        fn recorded_component_operations(&self) -> Vec<(String, String, String)> {
            self.state
                .lock()
                .expect("recording state lock")
                .component_ops
                .clone()
        }

        fn recorded_counters(&self) -> Vec<(String, f64)> {
            self.state
                .lock()
                .expect("recording state lock")
                .counters
                .clone()
        }
    }

    impl MetricsCollector for RecordingMetrics {
        fn record_exchange_duration(&self, _route_id: &str, _duration: Duration) {}

        fn increment_errors(&self, route_id: &str, error_type: &str) {
            self.state
                .lock()
                .expect("recording state lock")
                .errors
                .push((route_id.to_string(), error_type.to_string()));
        }

        fn increment_exchanges(&self, _route_id: &str) {}

        fn set_queue_depth(&self, _queue: &str, _depth: usize) {}

        fn record_circuit_breaker_change(&self, _route_id: &str, _from: &str, _to: &str) {}

        fn record_counter(&self, name: &str, value: f64, _labels: &[(&str, &str)]) {
            self.state
                .lock()
                .expect("recording state lock")
                .counters
                .push((name.to_string(), value));
        }

        fn record_component_operation(&self, component: &str, operation: &str, outcome: &str) {
            self.state
                .lock()
                .expect("recording state lock")
                .component_ops
                .push((
                    component.to_string(),
                    operation.to_string(),
                    outcome.to_string(),
                ));
        }
    }

    #[test]
    fn registry_starts_empty() {
        let registry = Registry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(registry.get("timer").is_none());
    }

    #[test]
    fn registry_registers_and_gets_components() {
        let mut registry = Registry::new();
        registry.register(Arc::new(TimerComponent::new()));
        registry.register(Arc::new(LogComponent::new()));

        assert_eq!(registry.len(), 2);
        assert!(registry.get("timer").is_some());
        assert!(registry.get("log").is_some());
        assert!(!registry.is_empty());
    }

    #[test]
    fn registry_get_or_err_reports_missing_component() {
        let mut registry = Registry::new();
        registry.register(Arc::new(TimerComponent::new()));

        let err = match registry.get_or_err("missing") {
            Ok(_) => panic!("must fail"),
            Err(err) => err,
        };
        assert!(matches!(err, CamelError::ComponentNotFound(_)));
    }

    #[test]
    fn registry_replaces_component_with_same_scheme() {
        let mut registry = Registry::new();
        registry.register(Arc::new(TimerComponent::new()));
        registry.register(Arc::new(TimerComponent::new()));

        assert_eq!(registry.len(), 1);
        assert!(registry.get("timer").is_some());
        assert_eq!(registry.all_metadata().len(), 1);
    }

    #[test]
    fn registry_harvests_metadata_on_register() {
        let mut registry = Registry::new();
        registry.register(Arc::new(TimerComponent::new()));

        let meta = registry.get_metadata("timer");
        assert!(meta.is_some());
        let meta = meta.unwrap(); // allow-unwrap
        assert_eq!(meta.scheme, "timer");
        assert_eq!(meta.schema_version, ComponentMetadata::SCHEMA_VERSION);
    }

    #[test]
    fn registry_all_metadata_returns_all_schemes() {
        let mut registry = Registry::new();
        registry.register(Arc::new(TimerComponent::new()));
        registry.register(Arc::new(LogComponent::new()));

        let all = registry.all_metadata();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn registry_metadata_schemes_lists_all_keys() {
        let mut registry = Registry::new();
        registry.register(Arc::new(TimerComponent::new()));
        registry.register(Arc::new(LogComponent::new()));

        let mut schemes = registry.metadata_schemes();
        schemes.sort();
        assert_eq!(schemes, vec!["log".to_string(), "timer".to_string()]);
    }

    #[test]
    fn metrics_returns_wired_collector_and_is_stable() {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let collector = Arc::new(RecordingMetrics::new());
        let wired_dyn: Arc<dyn MetricsCollector> = collector.clone();
        let ctx = RegistryComponentContext::new(registry, Some(collector), false);

        let first = ComponentContext::metrics(&ctx);
        let second = ComponentContext::metrics(&ctx);
        assert!(Arc::ptr_eq(&first, &wired_dyn));
        assert!(Arc::ptr_eq(&second, &wired_dyn));
    }

    #[test]
    fn component_metrics_enabled_reflects_constructor_lever() {
        let on = RegistryComponentContext::new(
            Arc::new(std::sync::Mutex::new(Registry::new())),
            None,
            true,
        );
        let off = RegistryComponentContext::new(
            Arc::new(std::sync::Mutex::new(Registry::new())),
            None,
            false,
        );

        assert!(ComponentContext::component_metrics_enabled(&on));
        assert!(!ComponentContext::component_metrics_enabled(&off));
    }

    #[test]
    fn facade_error_family_reaches_wired_collector_with_lever_off() {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let collector = Arc::new(RecordingMetrics::new());
        let ctx = RegistryComponentContext::new(registry, Some(collector.clone()), false);

        let facade = RuntimeObservability::component_metrics(&ctx);
        facade.observe("wasm", "invoke", true);

        assert_eq!(
            collector.recorded_errors(),
            vec![("wasm".to_string(), "e:wasm:invoke".to_string())]
        );
    }

    #[test]
    fn facade_component_family_gated_by_lever() {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let collector = Arc::new(RecordingMetrics::new());
        let ctx_on = RegistryComponentContext::new(registry.clone(), Some(collector.clone()), true);
        let ctx_off = RegistryComponentContext::new(registry, Some(collector.clone()), false);

        RuntimeObservability::component_metrics(&ctx_on).observe("wasm", "invoke", false);
        RuntimeObservability::component_metrics(&ctx_off).observe("wasm", "invoke", false);

        assert_eq!(
            collector.recorded_component_operations(),
            vec![(
                "wasm".to_string(),
                "invoke".to_string(),
                "success".to_string()
            )]
        );
        assert!(collector.recorded_errors().is_empty());
        assert!(collector.recorded_counters().is_empty());
    }

    #[test]
    fn late_registered_collector_reaches_registry_context() {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let handle = Arc::new(camel_api::MetricsHandle::new());
        let handle_dyn: Arc<dyn MetricsCollector> = handle.clone();
        let recording = Arc::new(RecordingMetrics::new());
        let ctx = RegistryComponentContext::new(registry, Some(handle_dyn), false);

        handle.register(recording.clone());
        ComponentContext::metrics(&ctx).increment_errors("wasm", "e:wasm:invoke");

        assert_eq!(
            recording.recorded_errors(),
            vec![("wasm".to_string(), "e:wasm:invoke".to_string())]
        );
    }

    #[test]
    fn none_falls_back_to_noop_semantics() {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let ctx = RegistryComponentContext::new(registry, None, false);

        // None of these may panic: metrics() resolves the fallback
        // collector, the facade builds over it, and the error flows into
        // NoOp silently.
        ComponentContext::metrics(&ctx);
        RuntimeObservability::component_metrics(&ctx).observe("wasm", "invoke", true);

        assert!(!ComponentContext::component_metrics_enabled(&ctx));
    }

    #[test]
    fn resolve_component_unaffected_by_observability_params() {
        let mut registry = Registry::new();
        registry.register(Arc::new(TimerComponent::new()));
        let registry = Arc::new(std::sync::Mutex::new(registry));
        let ctx = RegistryComponentContext::new(registry, None, false);

        assert!(ctx.resolve_component("timer").is_some());
    }
}
