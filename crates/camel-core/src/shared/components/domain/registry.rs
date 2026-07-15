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

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::component_metadata::ComponentMetadata;
    use camel_component_log::LogComponent;
    use camel_component_timer::TimerComponent;

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
}
