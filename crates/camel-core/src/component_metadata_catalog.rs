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
    use camel_api::CamelError;
    use camel_api::component_metadata::{
        CapabilityQuery, ComponentMetadataCatalog, OptionKind, UriOption,
    };
    use camel_component_api::{Component, ComponentContext, Endpoint};
    use camel_component_timer::TimerComponent;

    /// Stand-in for the real `http`/`ws` components in catalog tests.
    ///
    /// `Registry::register` only harvests metadata at registration time, so a
    /// stub carrying the real scheme name and a synthetic non-empty option list
    /// satisfies the catalog assertions without pulling the cyclic
    /// `camel-component-http`/`camel-component-ws` dev-deps into `camel-core`.
    /// The real-option catalog tests for those schemes are relocated to
    /// `crates/camel-test/tests/core_catalog_real_metadata_test.rs` (ADR-0055).
    struct StubComponent {
        scheme: String,
        metadata: ComponentMetadata,
    }

    impl StubComponent {
        fn new(scheme: &str, with_options: Vec<&str>) -> Self {
            let uri_options: Vec<_> = with_options
                .into_iter()
                .map(|name| UriOption::new(name, "stub option", OptionKind::String))
                .collect();
            let metadata = ComponentMetadata::minimal(scheme)
                .with_description("stub component for catalog tests")
                .with_uri_options(uri_options);
            Self {
                scheme: scheme.to_string(),
                metadata,
            }
        }
    }

    impl Component for StubComponent {
        fn scheme(&self) -> &str {
            &self.scheme
        }

        fn metadata(&self) -> ComponentMetadata {
            self.metadata.clone()
        }

        fn create_endpoint(
            &self,
            _uri: &str,
            _ctx: &dyn ComponentContext,
        ) -> Result<Box<dyn Endpoint>, CamelError> {
            Err(CamelError::EndpointCreationFailed(
                "StubComponent: create_endpoint not implemented".to_string(),
            ))
        }
    }

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
    fn phase1_schemes_expose_uri_options() {
        use std::sync::Arc;

        use camel_component_api::Component;
        use camel_component_api::NoOpComponentContext;
        use camel_component_controlbus::ControlBusComponent;
        use camel_component_grpc::GrpcComponent;
        use camel_component_jms::JmsBridgePool;
        use camel_component_jms::JmsComponent;
        use camel_component_jms::JmsPoolConfig;
        use camel_component_kafka::KafkaComponent;
        use camel_component_llm::LlmComponent;
        use camel_component_llm::LlmGlobalConfig;
        use camel_component_mqtt::MqttComponent;
        use camel_component_redis::RedisComponent;
        use camel_component_wasm::WasmComponent;

        // Note: SurrealDbComponent and KeycloakComponent are excluded.
        // SurrealDB deps exceed 3 GB (OOM risk). KeycloakComponent::new()
        // is async + network. Both are verified by their own per-component
        // parity tests.

        let registry = Arc::new(Mutex::new(Registry::new()));
        {
            let mut reg = registry
                .lock()
                .expect("mutex poisoned: another thread panicked while holding this lock"); // allow-unwrap
            reg.register(Arc::new(KafkaComponent::new()));
            reg.register(Arc::new(MqttComponent::new()));
            reg.register(Arc::new(RedisComponent::new()));
            reg.register(Arc::new(GrpcComponent::new()));
            reg.register(Arc::new(ControlBusComponent::new()));
            reg.register(Arc::new(WasmComponent::new(
                Arc::new(NoOpComponentContext),
                std::env::temp_dir(),
            )));
        }

        let catalog = RuntimeComponentMetadataCatalog::new(Arc::clone(&registry));
        let registry_schemes = &["kafka", "mqtt", "redis", "grpc", "controlbus", "wasm"];
        for scheme in registry_schemes {
            let meta = catalog
                .get_metadata(scheme)
                .unwrap_or_else(|| panic!("missing metadata for scheme '{scheme}'"));
            assert!(
                !meta.uri_options.is_empty(),
                "uri_options must be non-empty for scheme '{scheme}'"
            );
        }

        // JMS: needs a pool.
        let jms_pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                camel_component_jms::BrokerType::ActiveMq,
            ))
            .expect("JMS pool construction should succeed for metadata test"),
        );
        let jms = JmsComponent::with_scheme("jms", jms_pool);
        assert!(
            !jms.metadata().uri_options.is_empty(),
            "jms uri_options empty"
        );

        // LLM: default config, no providers.
        let llm = LlmComponent::new(LlmGlobalConfig::default())
            .expect("LLM construction should succeed with default config");
        assert!(
            !llm.metadata().uri_options.is_empty(),
            "llm uri_options empty"
        );
    }

    #[test]
    fn all_components_in_catalog() {
        use camel_component_container::ContainerComponent;
        use camel_component_cron::CronComponent;
        use camel_component_direct::DirectComponent;
        use camel_component_file::FileComponent;
        use camel_component_log::LogComponent;
        use camel_component_mock::MockComponent;
        use camel_component_opensearch::OpenSearchComponent;
        use camel_component_seda::SedaComponent;
        use camel_component_sql::SqlComponent;

        let registry = Arc::new(Mutex::new(Registry::new()));
        {
            let mut reg = registry
                .lock()
                .expect("mutex poisoned: another thread panicked while holding this lock"); // allow-unwrap
            reg.register(Arc::new(SqlComponent::new()));
            reg.register(Arc::new(FileComponent::new()));
            reg.register(Arc::new(CronComponent::new()));
            reg.register(Arc::new(OpenSearchComponent::new()));
            reg.register(Arc::new(StubComponent::new("ws", vec!["opt"])));
            reg.register(Arc::new(ContainerComponent::new()));
            reg.register(Arc::new(TimerComponent::new()));
            reg.register(Arc::new(DirectComponent::new()));
            reg.register(Arc::new(SedaComponent::new()));
            reg.register(Arc::new(LogComponent::new()));
            reg.register(Arc::new(MockComponent::new()));
            reg.register(Arc::new(StubComponent::new("http", vec!["opt"])));
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
        use camel_component_log::LogComponent;
        use camel_component_mock::MockComponent;
        use camel_component_opensearch::OpenSearchComponent;
        use camel_component_seda::SedaComponent;
        use camel_component_sql::SqlComponent;

        let registry = Arc::new(Mutex::new(Registry::new()));
        {
            let mut reg = registry
                .lock()
                .expect("mutex poisoned: another thread panicked while holding this lock"); // allow-unwrap
            reg.register(Arc::new(SqlComponent::new()));
            reg.register(Arc::new(FileComponent::new()));
            reg.register(Arc::new(CronComponent::new()));
            reg.register(Arc::new(OpenSearchComponent::new()));
            reg.register(Arc::new(StubComponent::new("ws", vec!["opt"])));
            reg.register(Arc::new(ContainerComponent::new()));
            reg.register(Arc::new(TimerComponent::new()));
            reg.register(Arc::new(DirectComponent::new()));
            reg.register(Arc::new(SedaComponent::new()));
            reg.register(Arc::new(LogComponent::new()));
            reg.register(Arc::new(MockComponent::new()));
            reg.register(Arc::new(StubComponent::new("http", vec!["opt"])));
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
