//! Origin: camel-core/src/component_metadata_catalog.rs cfg(test) (relocated per ADR-0055).
//!
//! These two tests assert the REAL `http`/`ws`/`sql`/... option catalog — a stub
//! cannot reproduce them. They moved here so `camel-core` no longer needs the
//! cyclic `camel-component-http`/`camel-component-ws` dev-dependencies.
//! `camel-test` is the publish-order leaf sink, so those deps are acyclic here.

use std::sync::{Arc, Mutex};

use camel_api::component_metadata::ComponentMetadataCatalog;
use camel_component_timer::TimerComponent;
use camel_core::Registry;
use camel_core::component_metadata_catalog::RuntimeComponentMetadataCatalog;

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

/// Phase-1 connector schemes (kafka/mqtt/redis/grpc/controlbus/wasm + jms/llm)
/// publish non-empty `uri_options`. Relocated from camel-core per ADR-0055:
/// these components depend back on camel-core (wasm via normal dep; http/llm
/// via dev-deps), so camel-core cannot host this test without closing a
/// publish cycle. camel-test is the publish-order leaf sink — acyclic here.
/// Gated on `integration-tests` because the components are optional deps.
#[cfg(feature = "integration-tests")]
#[test]
fn phase1_schemes_expose_uri_options() {
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
