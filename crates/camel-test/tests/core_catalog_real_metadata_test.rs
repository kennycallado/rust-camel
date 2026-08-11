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
