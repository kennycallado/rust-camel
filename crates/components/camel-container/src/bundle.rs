use std::sync::Arc;

use camel_component_api::{CamelError, ComponentBundle, ComponentRegistrar};

use crate::{ContainerComponent, ContainerGlobalConfig};

pub struct ContainerBundle {
    config: ContainerGlobalConfig,
}

impl ComponentBundle for ContainerBundle {
    fn config_key() -> &'static str {
        "container"
    }

    fn from_toml(value: toml::Value) -> Result<Self, CamelError> {
        let config: ContainerGlobalConfig = value
            .try_into()
            .map_err(|e: toml::de::Error| CamelError::Config(e.to_string()))?;
        Ok(Self { config })
    }

    fn register_all(self, ctx: &mut dyn ComponentRegistrar) {
        ctx.register_component_dyn(Arc::new(ContainerComponent::with_config(self.config)));
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
    fn container_bundle_from_toml_empty_uses_defaults() {
        let value: toml::Value = toml::from_str("").unwrap();
        let result = ContainerBundle::from_toml(value);
        assert!(
            result.is_ok(),
            "empty TOML must use defaults: {:?}",
            result.err()
        );
    }

    #[test]
    fn container_bundle_registers_expected_schemes() {
        let bundle = ContainerBundle::from_toml(toml::Value::Table(toml::map::Map::new())).unwrap();
        let mut registrar = TestRegistrar { schemes: vec![] };

        bundle.register_all(&mut registrar);

        assert_eq!(registrar.schemes, vec!["container"]);
    }

    #[test]
    fn container_bundle_from_toml_returns_error_on_invalid_config() {
        let mut table = toml::map::Map::new();
        table.insert("docker_host".to_string(), toml::Value::Integer(123));

        let result = ContainerBundle::from_toml(toml::Value::Table(table));
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
