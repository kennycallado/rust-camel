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

    #[test]
    fn all_phase2_schemes_have_options() {
        use camel_component_container::ContainerComponent;
        use camel_component_cron::CronComponent;
        use camel_component_file::FileComponent;
        use camel_component_opensearch::OpenSearchComponent;
        use camel_component_sql::SqlComponent;
        use camel_component_ws::WsComponent;

        let registry = Arc::new(Mutex::new(Registry::new()));
        {
            let mut reg = registry
                .lock()
                .expect("mutex poisoned: another thread panicked while holding this lock"); // allow-unwrap
            reg.register(Arc::new(SqlComponent::new()));
            reg.register(Arc::new(FileComponent::new()));
            reg.register(Arc::new(CronComponent::new()));
            reg.register(Arc::new(OpenSearchComponent::new()));
            reg.register(Arc::new(WsComponent::new()));
            reg.register(Arc::new(ContainerComponent::new()));
            reg.register(Arc::new(TimerComponent::new()));
        }

        let catalog = RuntimeComponentMetadataCatalog::new(Arc::clone(&registry));

        let schemes = &[
            "sql",
            "file",
            "cron",
            "opensearch",
            "ws",
            "container",
            "timer",
        ];

        for scheme in schemes {
            let meta = catalog
                .get_metadata(scheme)
                .unwrap_or_else(|| panic!("missing metadata for scheme '{scheme}'"));
            assert!(
                !meta.uri_options.is_empty(),
                "uri_options must be non-empty for scheme '{scheme}'"
            );
        }
    }

    #[test]
    fn no_duplicate_option_names() {
        use camel_component_container::ContainerComponent;
        use camel_component_cron::CronComponent;
        use camel_component_file::FileComponent;
        use camel_component_opensearch::OpenSearchComponent;
        use camel_component_sql::SqlComponent;
        use camel_component_ws::WsComponent;

        let registry = Arc::new(Mutex::new(Registry::new()));
        {
            let mut reg = registry
                .lock()
                .expect("mutex poisoned: another thread panicked while holding this lock"); // allow-unwrap
            reg.register(Arc::new(SqlComponent::new()));
            reg.register(Arc::new(FileComponent::new()));
            reg.register(Arc::new(CronComponent::new()));
            reg.register(Arc::new(OpenSearchComponent::new()));
            reg.register(Arc::new(WsComponent::new()));
            reg.register(Arc::new(ContainerComponent::new()));
            reg.register(Arc::new(TimerComponent::new()));
        }

        let catalog = RuntimeComponentMetadataCatalog::new(Arc::clone(&registry));

        let schemes = &[
            "sql",
            "file",
            "cron",
            "opensearch",
            "ws",
            "container",
            "timer",
        ];

        for scheme in schemes {
            let meta = catalog
                .get_metadata(scheme)
                .unwrap_or_else(|| panic!("missing metadata for scheme '{scheme}'"));
            let mut names: Vec<&str> = meta.uri_options.iter().map(|o| o.name.as_str()).collect();
            let original_len = names.len();
            names.sort_unstable();
            names.dedup();
            assert_eq!(
                names.len(),
                original_len,
                "duplicate option names found in scheme '{scheme}'"
            );
        }
    }

    #[test]
    fn all_components_in_catalog() {
        use camel_component_container::ContainerComponent;
        use camel_component_cron::CronComponent;
        use camel_component_direct::DirectComponent;
        use camel_component_file::FileComponent;
        use camel_component_http::HttpComponent;
        use camel_component_log::LogComponent;
        use camel_component_mock::MockComponent;
        use camel_component_opensearch::OpenSearchComponent;
        use camel_component_seda::SedaComponent;
        use camel_component_sql::SqlComponent;
        use camel_component_ws::WsComponent;

        let registry = Arc::new(Mutex::new(Registry::new()));
        {
            let mut reg = registry
                .lock()
                .expect("mutex poisoned: another thread panicked while holding this lock"); // allow-unwrap
            reg.register(Arc::new(SqlComponent::new()));
            reg.register(Arc::new(FileComponent::new()));
            reg.register(Arc::new(CronComponent::new()));
            reg.register(Arc::new(OpenSearchComponent::new()));
            reg.register(Arc::new(WsComponent::new()));
            reg.register(Arc::new(ContainerComponent::new()));
            reg.register(Arc::new(TimerComponent::new()));
            reg.register(Arc::new(DirectComponent::new()));
            reg.register(Arc::new(SedaComponent::new()));
            reg.register(Arc::new(LogComponent::new()));
            reg.register(Arc::new(MockComponent::new()));
            reg.register(Arc::new(HttpComponent::new()));
        }

        let catalog = RuntimeComponentMetadataCatalog::new(Arc::clone(&registry));

        let expected_schemes: &[&str] = &[
            "sql",
            "file",
            "cron",
            "opensearch",
            "ws",
            "container",
            "timer",
            "direct",
            "seda",
            "log",
            "mock",
            "http",
        ];

        for scheme in expected_schemes {
            let meta = catalog
                .get_metadata(scheme)
                .unwrap_or_else(|| panic!("missing metadata for scheme '{scheme}'"));
            // mock may have empty options; others should be non-empty
            if *scheme != "mock" {
                assert!(
                    !meta.uri_options.is_empty(),
                    "uri_options must be non-empty for scheme '{scheme}'"
                );
            }
            assert!(
                !meta.scheme.is_empty(),
                "scheme must be non-empty for '{scheme}'"
            );
            assert!(
                !meta.description.is_empty(),
                "description must be non-empty for scheme '{scheme}'"
            );
        }
    }

    #[test]
    fn no_duplicate_option_names_all() {
        use camel_component_container::ContainerComponent;
        use camel_component_cron::CronComponent;
        use camel_component_direct::DirectComponent;
        use camel_component_file::FileComponent;
        use camel_component_http::HttpComponent;
        use camel_component_log::LogComponent;
        use camel_component_mock::MockComponent;
        use camel_component_opensearch::OpenSearchComponent;
        use camel_component_seda::SedaComponent;
        use camel_component_sql::SqlComponent;
        use camel_component_ws::WsComponent;

        let registry = Arc::new(Mutex::new(Registry::new()));
        {
            let mut reg = registry
                .lock()
                .expect("mutex poisoned: another thread panicked while holding this lock"); // allow-unwrap
            reg.register(Arc::new(SqlComponent::new()));
            reg.register(Arc::new(FileComponent::new()));
            reg.register(Arc::new(CronComponent::new()));
            reg.register(Arc::new(OpenSearchComponent::new()));
            reg.register(Arc::new(WsComponent::new()));
            reg.register(Arc::new(ContainerComponent::new()));
            reg.register(Arc::new(TimerComponent::new()));
            reg.register(Arc::new(DirectComponent::new()));
            reg.register(Arc::new(SedaComponent::new()));
            reg.register(Arc::new(LogComponent::new()));
            reg.register(Arc::new(MockComponent::new()));
            reg.register(Arc::new(HttpComponent::new()));
        }

        let catalog = RuntimeComponentMetadataCatalog::new(Arc::clone(&registry));

        let schemes = &[
            "sql",
            "file",
            "cron",
            "opensearch",
            "ws",
            "container",
            "timer",
            "direct",
            "seda",
            "log",
            "mock",
            "http",
        ];

        for scheme in schemes {
            let meta = catalog
                .get_metadata(scheme)
                .unwrap_or_else(|| panic!("missing metadata for scheme '{scheme}'"));
            let mut names: Vec<&str> = meta.uri_options.iter().map(|o| o.name.as_str()).collect();
            let original_len = names.len();
            names.sort_unstable();
            names.dedup();
            assert_eq!(
                names.len(),
                original_len,
                "duplicate option names found in scheme '{scheme}'"
            );
        }
    }
}
