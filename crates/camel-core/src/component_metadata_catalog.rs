//! Runtime implementation of [`ComponentMetadataCatalog`].
//!
//! Thin wrapper around the component [`Registry`]'s `Arc<Mutex<Registry>>`
//! that implements the query trait. Created on-demand via
//! [`CamelContext::metadata_catalog`](crate::context::CamelContext::metadata_catalog).

use std::sync::{Arc, Mutex};

use camel_api::component_metadata::{ComponentMetadata, ComponentMetadataCatalog};

use crate::shared::components::domain::Registry;

/// Runtime catalog of component metadata backed by the live component
/// [`Registry`].
pub struct RuntimeComponentMetadataCatalog {
    registry: Arc<Mutex<Registry>>,
}

impl RuntimeComponentMetadataCatalog {
    /// Wrap an existing `Arc<Mutex<Registry>>` to expose it as a
    /// [`ComponentMetadataCatalog`].
    pub fn new(registry: Arc<Mutex<Registry>>) -> Self {
        Self { registry }
    }
}

impl ComponentMetadataCatalog for RuntimeComponentMetadataCatalog {
    fn get_metadata(&self, scheme: &str) -> Option<ComponentMetadata> {
        self.registry.lock().ok()?.get_metadata(scheme)
    }

    fn schemes(&self) -> Vec<String> {
        self.registry
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock") // allow-unwrap
            .metadata_schemes()
    }

    fn all_metadata(&self) -> Vec<ComponentMetadata> {
        self.registry
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock") // allow-unwrap
            .all_metadata()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::component_metadata::{CapabilityQuery, ComponentMetadataCatalog};
    use camel_component_timer::TimerComponent;

    #[test]
    fn catalog_exposes_registered_metadata() {
        let registry = Arc::new(Mutex::new(Registry::new()));
        registry
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock") // allow-unwrap
            .register(Arc::new(TimerComponent::new()));

        let catalog = RuntimeComponentMetadataCatalog::new(Arc::clone(&registry));

        let meta = catalog.get_metadata("timer");
        assert!(meta.is_some());
        assert_eq!(meta.unwrap().scheme, "timer"); // allow-unwrap
        assert_eq!(catalog.schemes(), vec!["timer".to_string()]);
        assert_eq!(catalog.all_metadata().len(), 1);
    }

    #[test]
    fn catalog_query_capabilities_default_impl() {
        let registry = Arc::new(Mutex::new(Registry::new()));
        registry
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock") // allow-unwrap
            .register(Arc::new(TimerComponent::new()));

        let catalog = RuntimeComponentMetadataCatalog::new(Arc::clone(&registry));

        // No constraints => all metadata returned via the trait default impl.
        let results = catalog.query_capabilities(&CapabilityQuery::default());
        assert_eq!(results.len(), 1);
    }
}
