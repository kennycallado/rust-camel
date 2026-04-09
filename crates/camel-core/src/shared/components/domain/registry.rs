use std::collections::HashMap;
use std::sync::Arc;

use camel_api::CamelError;
use camel_component_api::Component;

/// Registry that stores components by their URI scheme.
pub struct Registry {
    components: HashMap<String, Arc<dyn Component>>,
}

impl Registry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            components: HashMap::new(),
        }
    }

    /// Register a component. Replaces any existing component with the same scheme.
    pub fn register(&mut self, component: Arc<dyn Component>) {
        self.components
            .insert(component.scheme().to_string(), component);
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
    }
}
