use std::sync::Arc;

use camel_component_api::{CamelError, ComponentBundle, ComponentRegistrar};

use crate::{WsComponent, WssComponent, config::WsConfig};

pub struct WsBundle {
    config: WsConfig,
}

impl ComponentBundle for WsBundle {
    fn config_key() -> &'static str {
        "ws"
    }

    fn from_toml(value: toml::Value) -> Result<Self, CamelError> {
        let config: WsConfig = value
            .try_into()
            .map_err(|e: toml::de::Error| CamelError::Config(e.to_string()))?;
        Ok(Self { config })
    }

    fn register_all(self, ctx: &mut dyn ComponentRegistrar) {
        ctx.register_component_dyn(Arc::new(WsComponent::with_config(self.config.clone())));
        ctx.register_component_dyn(Arc::new(WssComponent::with_config(self.config)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn ws_bundle_from_toml_empty_uses_defaults() {
        let value: toml::Value = toml::from_str("").unwrap();
        assert!(WsBundle::from_toml(value).is_ok());
    }

    #[test]
    fn ws_bundle_registers_expected_schemes() {
        let bundle = WsBundle::from_toml(toml::Value::Table(toml::map::Map::new())).unwrap();
        let mut registrar = TestRegistrar { schemes: vec![] };

        bundle.register_all(&mut registrar);

        assert_eq!(registrar.schemes, vec!["ws", "wss"]);
    }

    #[test]
    fn ws_bundle_from_toml_returns_error_on_invalid_config() {
        let mut table = toml::map::Map::new();
        table.insert(
            "max_message_size".to_string(),
            toml::Value::String("not-a-number".to_string()),
        );

        let result = WsBundle::from_toml(toml::Value::Table(table));
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
