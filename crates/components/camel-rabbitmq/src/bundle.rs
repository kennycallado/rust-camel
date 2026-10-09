//! `RabbitMqBundle`: registers the `rabbitmq` scheme from
//! `[components.rabbitmq]` (kafka `bundle.rs` shape, extended with the
//! slot-bound lifecycle context that lets reconnect chains observe the
//! CURRENT runtime shutdown token).

use std::sync::Arc;

use camel_component_api::{CamelError, ComponentBundle, ComponentContext, ComponentRegistrar};

use crate::{RabbitMqComponent, config::RabbitComponentConfig};

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
    /// `from_toml` and return the rejection message, panicking on success.
    fn reject_at_boot(raw: &str) -> String {
        let value: toml::Value = toml::from_str(raw).expect("test TOML must parse");
        match RabbitMqBundle::from_toml(value) {
            Ok(_) => panic!("tls.* must be rejected at boot, not parsed and ignored"),
            Err(error) => error.to_string(),
        }
    }

    /// A `tls` key is not a supported broker parameter: the error must name
    /// `tls`, the broker section, and the supported params — never secrets.
    fn assert_tls_unsupported(message: &str) {
        assert!(message.contains("tls"), "must name tls; got: {message}");
        assert!(
            message.contains("brokers"),
            "must name the broker section; got: {message}"
        );
        for key in ["url", "username", "password", "vhost"] {
            assert!(
                message.contains(key),
                "must list supported param '{key}'; got: {message}"
            );
        }
    }

    #[test]
    fn tls_ca_cert_rejected_at_boot() {
        let message = reject_at_boot(
            "[brokers.main]\n\
             url = \"amqp://user:hunter2@localhost:5672\"\n\
             password = \"hunter2\"\n\
             [brokers.main.tls]\n\
             ca_cert = \"/etc/rabbitmq/ca.pem\"\n",
        );
        assert_tls_unsupported(&message);
        assert!(
            !message.contains("hunter2"),
            "must not leak secrets: {message}"
        );
        assert!(
            !message.contains("ca.pem"),
            "must not echo tls values: {message}"
        );
    }

    #[test]
    fn tls_server_name_rejected_at_boot() {
        let message = reject_at_boot(
            "[brokers.main]\n\
             url = \"amqp://localhost:5672\"\n\
             tls = { server_name = \"broker.internal\" }\n",
        );
        assert_tls_unsupported(&message);
        assert!(
            !message.contains("broker.internal"),
            "must not echo tls values: {message}"
        );
    }

    #[test]
    fn empty_tls_section_rejected_at_boot() {
        let message = reject_at_boot(
            "[brokers.main]\n\
             url = \"amqp://localhost:5672\"\n\
             tls = {}\n",
        );
        assert_tls_unsupported(&message);
    }
}
