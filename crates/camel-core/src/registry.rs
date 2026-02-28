use std::collections::HashMap;

use camel_api::CamelError;
use camel_component::Component;

/// Registry that stores components by their URI scheme.
pub struct Registry {
    components: HashMap<String, Box<dyn Component>>,
}

impl Registry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            components: HashMap::new(),
        }
    }

    /// Register a component. Replaces any existing component with the same scheme.
    pub fn register<C: Component + 'static>(&mut self, component: C) {
        self.components
            .insert(component.scheme().to_string(), Box::new(component));
    }

    /// Look up a component by scheme.
    pub fn get(&self, scheme: &str) -> Option<&dyn Component> {
        self.components.get(scheme).map(|c| c.as_ref())
    }

    /// Look up a component by scheme, returning an error if not found.
    pub fn get_or_err(&self, scheme: &str) -> Result<&dyn Component, CamelError> {
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
