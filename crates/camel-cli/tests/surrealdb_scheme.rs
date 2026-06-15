//! Integration test: SurrealDB scheme is registered in the CLI catalog.
//!
//! Verifies that `SurrealDbBundle` deserializes from TOML and registers
//! a component whose scheme is "surrealdb".

#[cfg(feature = "surrealdb")]
#[test]
fn surrealdb_bundle_registers_surrealdb_scheme() {
    use std::sync::{Arc, Mutex};

    use camel_component_api::{Component, ComponentBundle, ComponentRegistrar};
    use camel_component_surrealdb::SurrealDbBundle;

    // Minimal TOML config (empty table is fine — bundle has no required fields)
    let raw: toml::Value = toml::Value::Table(toml::map::Map::new());

    let bundle = SurrealDbBundle::from_toml(raw)
        .expect("SurrealDbBundle::from_toml must accept empty table");

    // Stub registrar to capture the registered component
    #[derive(Default)]
    struct CapturedComponents {
        components: Mutex<Vec<Arc<dyn Component>>>,
    }

    impl ComponentRegistrar for CapturedComponents {
        fn register_component_dyn(&mut self, component: Arc<dyn Component>) {
            self.components.lock().unwrap().push(component);
        }
    }

    let mut registrar = CapturedComponents::default();
    bundle.register_all(&mut registrar);

    let registered = registrar.components.lock().unwrap();
    assert!(
        !registered.is_empty(),
        "SurrealDbBundle must register at least one component"
    );
    assert_eq!(
        registered[0].scheme(),
        "surrealdb",
        "registered component scheme must be 'surrealdb'"
    );
}

#[cfg(not(feature = "surrealdb"))]
#[test]
fn surrealdb_feature_optional() {
    println!("surrealdb feature disabled — skipping");
}
