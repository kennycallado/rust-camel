use std::sync::Arc;

use camel_component_api::{CamelError, ComponentBundle, ComponentRegistrar};

use crate::{RedisComponent, RedisConfig, RedisSentinelComponent, RedissSentinelComponent};

pub struct RedisBundle {
    config: RedisConfig,
    catalog: Option<Arc<dyn camel_api::datasource::DatasourceCatalog>>,
}

impl RedisBundle {
    /// Registers the Redis datasource factory with the boot-owned catalog.
    pub fn with_catalog(
        mut self,
        catalog: Arc<dyn camel_api::datasource::DatasourceCatalog>,
    ) -> Self {
        if let Err(error) = catalog.register_factory("redis", Arc::new(crate::RedisPoolFactory)) {
            tracing::warn!(%error, "Redis datasource factory registration failed");
        }
        self.catalog = Some(catalog);
        self
    }
}

impl ComponentBundle for RedisBundle {
    fn config_key() -> &'static str {
        "redis"
    }

    fn from_toml(value: toml::Value) -> Result<Self, CamelError> {
        let config: RedisConfig = value
            .try_into()
            .map_err(|e: toml::de::Error| CamelError::Config(e.to_string()))?;
        Ok(Self {
            config,
            catalog: None,
        })
    }

    fn register_all(self, ctx: &mut dyn ComponentRegistrar) {
        // The registry resolves route URIs by scheme, so the sentinel URI
        // schemes need their own registered components.
        ctx.register_component_dyn(Arc::new(RedisComponent::with_config(self.config.clone())));
        ctx.register_component_dyn(Arc::new(RedisSentinelComponent::with_config(
            self.config.clone(),
        )));
        ctx.register_component_dyn(Arc::new(RedissSentinelComponent::with_config(self.config)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::ComponentBundle;

    #[test]
    fn bundle_with_catalog_registers_redis_factory() {
        let catalog = Arc::new(camel_core::datasource::RuntimeDatasourceCatalog::new(
            std::collections::HashMap::new(),
        ));
        let _bundle = RedisBundle::from_toml(toml::Value::Table(toml::map::Map::new()))
            .expect("empty config")
            .with_catalog(catalog.clone());
        use camel_api::datasource::DatasourceCatalog;
        assert!(
            catalog
                .register_factory("redis", Arc::new(crate::RedisPoolFactory))
                .is_err()
        );
    }

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
    fn redis_bundle_from_toml_empty_uses_defaults() {
        let value: toml::Value = toml::from_str("").unwrap();
        let result = RedisBundle::from_toml(value);
        assert!(
            result.is_ok(),
            "empty TOML must use defaults: {:?}",
            result.err()
        );
    }

    #[test]
    fn redis_bundle_registers_expected_schemes() {
        let bundle = RedisBundle::from_toml(toml::Value::Table(toml::map::Map::new())).unwrap();
        let mut registrar = TestRegistrar { schemes: vec![] };

        bundle.register_all(&mut registrar);

        assert_eq!(
            registrar.schemes,
            vec!["redis", "redis-sentinel", "rediss-sentinel"]
        );
    }

    #[test]
    fn redis_bundle_from_toml_returns_error_on_invalid_config() {
        let mut table = toml::map::Map::new();
        table.insert(
            "port".to_string(),
            toml::Value::String("not-a-number".to_string()),
        );

        let result = RedisBundle::from_toml(toml::Value::Table(table));
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
