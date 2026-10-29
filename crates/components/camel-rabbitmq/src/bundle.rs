//! `RabbitMqBundle`: registers the `rabbitmq` scheme from
//! `[components.rabbitmq]` (kafka `bundle.rs` shape, extended with the
//! slot-bound lifecycle context that lets reconnect chains observe the
//! CURRENT runtime shutdown token).

use std::sync::Arc;

use camel_component_api::{CamelError, ComponentBundle, ComponentContext, ComponentRegistrar};

use crate::RabbitMqComponent;
use crate::config::RabbitComponentConfig;
use crate::tls;

/// Bundle owning the `rabbitmq` scheme.
pub struct RabbitMqBundle {
    config: RabbitComponentConfig,
    lifecycle_context: Option<Arc<dyn ComponentContext>>,
}

impl RabbitMqBundle {
    /// Bind the registration-time lifecycle context. The boot cascade hands
    /// in a slot-bound `RegistryComponentContext` so every manager resolves
    /// the current runtime token per call (fresh across stop/start).
    pub fn with_lifecycle_context(mut self, context: Arc<dyn ComponentContext>) -> Self {
        self.lifecycle_context = Some(context);
        self
    }
}

impl ComponentBundle for RabbitMqBundle {
    fn config_key() -> &'static str {
        "rabbitmq"
    }

    fn from_toml(value: toml::Value) -> Result<Self, CamelError> {
        // `deny_unknown_fields` on `RabbitComponentConfig` rejects unknown
        // keys; an empty brokers map is a supported, registerable config.
        let config: RabbitComponentConfig = value
            .try_into()
            .map_err(|e: toml::de::Error| CamelError::Config(e.to_string()))?;
        tls::validate(&config)?;
        Ok(Self {
            config,
            lifecycle_context: None,
        })
    }

    fn register_all(self, ctx: &mut dyn ComponentRegistrar) {
        let mut component = RabbitMqComponent::new(self.config);
        if let Some(context) = self.lifecycle_context {
            component = component.with_lifecycle_context(context);
        }
        ctx.register_component_dyn(Arc::new(component));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::{Component, ComponentBundle};

    struct TestRegistrar {
        schemes: Vec<String>,
    }

    impl ComponentRegistrar for TestRegistrar {
        fn register_component_dyn(&mut self, component: Arc<dyn Component>) {
            self.schemes.push(component.scheme().to_string());
        }
    }

    #[test]
    fn rabbit_bundle_from_toml_empty_registers_scheme() {
        let value: toml::Value = toml::from_str("").unwrap();
        let bundle = RabbitMqBundle::from_toml(value).expect("empty TOML must use defaults");
        let mut registrar = TestRegistrar { schemes: vec![] };

        bundle.register_all(&mut registrar);

        assert_eq!(registrar.schemes, vec!["rabbitmq"]);
    }

    #[test]
    fn rabbit_bundle_from_toml_rejects_unknown_key() {
        let mut table = toml::map::Map::new();
        table.insert("bogus".to_string(), toml::Value::Integer(1));

        let result = RabbitMqBundle::from_toml(toml::Value::Table(table));

        assert!(result.is_err(), "expected deny_unknown_fields rejection");
    }

    /// Parse the `[components.rabbitmq]` value shape the boot cascade hands to
    /// `from_toml`.
    fn parse_at_boot(raw: &str) -> Result<RabbitMqBundle, CamelError> {
        let value: toml::Value = toml::from_str(raw).expect("test TOML must parse");
        RabbitMqBundle::from_toml(value)
    }

    fn error_text(result: Result<RabbitMqBundle, CamelError>) -> String {
        match result {
            Ok(_) => panic!("expected a boot rejection, got Ok"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn bundle_from_toml_rejects_tls_on_amqp_url() {
        let result = parse_at_boot(
            "[brokers.main]\n\
             url = \"amqp://localhost:5672\"\n\
             [brokers.main.tls]\n\
             ca_cert = \"/etc/rabbitmq/ca.pem\"\n",
        );
        let message = error_text(result);
        assert!(
            message.contains("require an amqps:// URL"),
            "got: {message}"
        );
        assert!(
            message.contains("the URL scheme is 'amqp'"),
            "got: {message}"
        );
    }

    #[test]
    fn bundle_from_toml_rejects_invalid_tls_at_boot() {
        let result = parse_at_boot(
            "[brokers.payments]\n\
             url = \"amqps://localhost:5671\"\n\
             [brokers.payments.tls]\n\
             server_name = \"broker.internal\"\n",
        );
        let message = error_text(result);
        let expected = crate::tls::SERVER_NAME_UNSUPPORTED_ERROR.replace("{broker}", "payments");
        assert!(message.contains(&expected), "got: {message}");
    }

    #[test]
    fn tls_unknown_sub_key_rejects_at_boot() {
        let result = parse_at_boot(
            "[brokers.main]\n\
             url = \"amqps://localhost:5671\"\n\
             [brokers.main.tls]\n\
             ca_cert = \"/etc/rabbitmq/ca.pem\"\n\
             skip_verify = true\n",
        );
        assert!(
            result.is_err(),
            "an unknown tls sub-key must fail closed via deny_unknown_fields"
        );
    }
}
