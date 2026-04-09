use std::sync::Arc;

use camel_component_api::{CamelError, ComponentBundle, ComponentRegistrar};

use crate::{JmsBridgePool, JmsComponent, config::JmsPoolConfig};

/// Bundles the JMS component (jms/activemq/artemis schemes) with its pool lifecycle.
pub struct JmsBundle {
    pool: Arc<JmsBridgePool>,
}

impl JmsBundle {
    pub fn new(cfg: JmsPoolConfig) -> Result<Self, CamelError> {
        let pool =
            JmsBridgePool::from_config(cfg).map_err(|e| CamelError::Config(e.to_string()))?;
        Ok(Self {
            pool: Arc::new(pool),
        })
    }

    pub fn pool(&self) -> Arc<JmsBridgePool> {
        Arc::clone(&self.pool)
    }
}

impl ComponentBundle for JmsBundle {
    fn config_key() -> &'static str {
        "jms"
    }

    fn from_toml(value: toml::Value) -> Result<Self, CamelError> {
        let cfg: JmsPoolConfig = value
            .try_into()
            .map_err(|e: toml::de::Error| CamelError::Config(e.to_string()))?;
        Self::new(cfg)
    }

    fn register_all(self, ctx: &mut dyn ComponentRegistrar) {
        ctx.register_component_dyn(Arc::new(JmsComponent::with_scheme(
            "jms",
            Arc::clone(&self.pool),
        )));
        ctx.register_component_dyn(Arc::new(JmsComponent::with_scheme(
            "activemq",
            Arc::clone(&self.pool),
        )));
        ctx.register_component_dyn(Arc::new(JmsComponent::with_scheme("artemis", self.pool)));
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
    fn jms_bundle_from_toml_valid() {
        let toml_str = r#"
            default_broker = "default"
            [brokers.default]
            broker_url = "tcp://localhost:61616"
            broker_type = "activemq"
        "#;
        let value: toml::Value = toml::from_str(toml_str).expect("valid toml");
        let result = JmsBundle::from_toml(value);
        assert!(
            result.is_ok(),
            "valid JMS toml must parse: {:?}",
            result.err()
        );
    }

    #[test]
    fn jms_bundle_registers_expected_schemes() {
        let toml_str = r#"
            default_broker = "default"
            [brokers.default]
            broker_url = "tcp://localhost:61616"
            broker_type = "activemq"
        "#;
        let value: toml::Value = toml::from_str(toml_str).expect("valid toml");
        let bundle = JmsBundle::from_toml(value).expect("bundle from valid toml");
        let mut registrar = TestRegistrar { schemes: vec![] };

        bundle.register_all(&mut registrar);

        assert_eq!(registrar.schemes, vec!["jms", "activemq", "artemis"]);
    }

    #[test]
    fn jms_bundle_from_toml_returns_error_on_invalid_config() {
        let mut table = toml::map::Map::new();
        table.insert(
            "max_bridges".to_string(),
            toml::Value::String("not-a-number".to_string()),
        );

        let result = JmsBundle::from_toml(toml::Value::Table(table));
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
