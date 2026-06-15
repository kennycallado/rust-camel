//! SurrealDbBundle — groups SurrealDB component schemes and shared config.
//!
//! Follows SqlBundle's `with_catalog()` pattern. Registers the `surrealdb`
//! scheme and installs SurrealDbPoolFactory into the datasource catalog
//! when a catalog is provided.

use std::sync::Arc;

use camel_component_api::{CamelError, ComponentBundle, ComponentRegistrar};

use crate::SurrealDbComponent;

/// Configuration bundle for the SurrealDB component.
///
/// Owns the `[components.surrealdb]` TOML key. When a `DatasourceCatalog`
/// is provided via `with_catalog()`, registers `SurrealDbPoolFactory`
/// so the datasource system can create SurrealDB connections.
#[derive(Default)]
pub struct SurrealDbBundle {
    catalog: Option<Arc<dyn camel_api::datasource::DatasourceCatalog>>,
}

impl ComponentBundle for SurrealDbBundle {
    fn config_key() -> &'static str {
        "surrealdb"
    }

    fn from_toml(_value: toml::Value) -> Result<Self, CamelError> {
        // SurrealDbBundle has no global config (connection params go through
        // the datasource catalog). Accept any TOML value, including empty.
        Ok(Self { catalog: None })
    }

    fn register_all(self, ctx: &mut dyn ComponentRegistrar) {
        let component = match self.catalog {
            Some(catalog) => SurrealDbComponent::with_catalog(catalog),
            None => SurrealDbComponent::default(),
        };
        ctx.register_component_dyn(Arc::new(component));
    }
}

impl SurrealDbBundle {
    /// Attach a datasource catalog and register `SurrealDbPoolFactory`.
    ///
    /// Factory registration happens here (same pattern as SqlBundle) so
    /// that the datasource system knows how to create SurrealDB connections
    /// for the named datasources in the catalog.
    pub fn with_catalog(
        mut self,
        catalog: Arc<dyn camel_api::datasource::DatasourceCatalog>,
    ) -> Self {
        if let Err(e) = catalog.register_factory(
            "surrealdb",
            Arc::new(crate::pool_factory::SurrealDbPoolFactory),
        ) {
            // log-policy: outside-contract
            tracing::warn!("failed to register surrealdb pool factory: {e}");
        }
        self.catalog = Some(catalog);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::ComponentBundle;

    struct TestRegistrar {
        schemes: Vec<String>,
    }

    impl ComponentRegistrar for TestRegistrar {
        fn register_component_dyn(
            &mut self,
            component: std::sync::Arc<dyn camel_component_api::Component>,
        ) {
            self.schemes.push(component.scheme().to_string());
        }
    }

    #[test]
    fn config_key_is_surrealdb() {
        assert_eq!(SurrealDbBundle::config_key(), "surrealdb");
    }

    #[test]
    fn bundle_from_toml_empty_succeeds() {
        let value: toml::Value = toml::from_str("").unwrap();
        let result = SurrealDbBundle::from_toml(value);
        assert!(
            result.is_ok(),
            "empty TOML must succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn bundle_registers_surrealdb_scheme() {
        let bundle = SurrealDbBundle::default();
        let mut registrar = TestRegistrar { schemes: vec![] };

        bundle.register_all(&mut registrar);

        assert_eq!(registrar.schemes, vec!["surrealdb"]);
    }

    #[test]
    fn bundle_without_catalog_still_registers() {
        // Verify that a bundle without a catalog still registers schemes
        // (the pool factory registration is optional).
        let bundle = SurrealDbBundle { catalog: None };
        let mut registrar = TestRegistrar { schemes: vec![] };

        bundle.register_all(&mut registrar);

        assert_eq!(registrar.schemes, vec!["surrealdb"]);
    }
}
