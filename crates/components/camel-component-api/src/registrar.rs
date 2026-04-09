use std::sync::Arc;

use crate::Component;

/// Registration interface for installing components into a context.
pub trait ComponentRegistrar {
    /// Register a component instance.
    fn register_component_dyn(&mut self, component: Arc<dyn Component>);
}
