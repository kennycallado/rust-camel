use std::sync::Arc;

use camel_component_api::{CamelError, ComponentBundle, ComponentRegistrar};

use crate::{SqlComponent, config::SqlGlobalConfig};

pub struct SqlBundle {
    config: SqlGlobalConfig,
    catalog: Option<Arc<dyn camel_api::datasource::DatasourceCatalog>>,
}

impl ComponentBundle for SqlBundle {
    fn config_key() -> &'static str {
        "sql"
    }

    fn from_toml(value: toml::Value) -> Result<Self, CamelError> {
        let config: SqlGlobalConfig = value
            .try_into()
            .map_err(|e: toml::de::Error| CamelError::Config(e.to_string()))?;
        Ok(Self {
            config,
            catalog: None,
        })
    }

    fn register_all(self, ctx: &mut dyn ComponentRegistrar) {
        let component = match self.catalog {
            Some(catalog) => SqlComponent::with_config_and_catalog(self.config, catalog),
            None => SqlComponent::with_config(self.config),
        };
        ctx.register_component_dyn(Arc::new(component));
    }
}

impl SqlBundle {
    pub fn with_catalog(
        mut self,
        catalog: Arc<dyn camel_api::datasource::DatasourceCatalog>,
    ) -> Self {
        if let Err(e) =
            catalog.register_factory("sqlx", Arc::new(crate::pool_factory::SqlPoolFactory))
        {
            // log-policy: outside-contract
            tracing::warn!("failed to register sqlx pool factory: {}", e);
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

    impl camel_component_api::ComponentRegistrar for TestRegistrar {
        fn register_component_dyn(
            &mut self,
            component: std::sync::Arc<dyn camel_component_api::Component>,
        ) {
            self.schemes.push(component.scheme().to_string());
        }
    }

    #[test]
    fn sql_bundle_from_toml_empty_uses_defaults() {
        let value: toml::Value = toml::from_str("").unwrap();
        let result = SqlBundle::from_toml(value);
        assert!(
            result.is_ok(),
            "empty TOML must use defaults: {:?}",
            result.err()
        );
    }

    #[test]
    fn sql_bundle_registers_expected_schemes() {
        let bundle = SqlBundle::from_toml(toml::Value::Table(toml::map::Map::new())).unwrap();
        let mut registrar = TestRegistrar { schemes: vec![] };

        bundle.register_all(&mut registrar);

        assert_eq!(registrar.schemes, vec!["sql"]);
    }

    #[test]
    fn sql_bundle_from_toml_returns_error_on_invalid_config() {
        let mut table = toml::map::Map::new();
        table.insert(
            "max_connections".to_string(),
            toml::Value::String("not-a-number".to_string()),
        );

        let result = SqlBundle::from_toml(toml::Value::Table(table));
        assert!(result.is_err(), "expected Err on malformed config");
        let err_msg = match result {
            Err(err) => err.to_string(),
            Ok(_) => panic!("expected Err on malformed config"),
        };
        assert!(
            err_msg.contains("Configuration error"),
            "expected CamelError::Config, got: {err_msg}"
        );
    }
}
