//! Test helpers for camel-lint rule tests.
//!
//! Provides `StubCatalog` — a test-only implementation of
//! `ComponentMetadataCatalog` for controlled metadata injection in rule unit
//! tests.

use std::collections::HashMap;

use camel_api::component_metadata::{ComponentMetadata, ComponentMetadataCatalog};

/// A test catalog that holds a fixed set of metadata entries.
pub(crate) struct StubCatalog {
    entries: HashMap<String, ComponentMetadata>,
}

impl StubCatalog {
    /// Create an empty stub catalog.
    pub(crate) fn empty() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Add a metadata entry for the given scheme (builder pattern).
    pub(crate) fn with(mut self, scheme: &str, meta: ComponentMetadata) -> Self {
        self.entries.insert(scheme.to_string(), meta);
        self
    }
}

impl ComponentMetadataCatalog for StubCatalog {
    fn get_metadata(&self, scheme: &str) -> Option<ComponentMetadata> {
        self.entries.get(scheme).cloned()
    }

    fn schemes(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    fn all_metadata(&self) -> Vec<ComponentMetadata> {
        self.entries.values().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_catalog_trait_object_safe() {
        let catalog = StubCatalog::empty().with("timer", ComponentMetadata::minimal("timer"));
        let dyn_catalog: &dyn ComponentMetadataCatalog = &catalog;

        assert!(dyn_catalog.get_metadata("timer").is_some());
        assert!(dyn_catalog.get_metadata("log").is_none());
        assert_eq!(dyn_catalog.schemes(), vec!["timer"]);
    }
}
