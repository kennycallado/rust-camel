//! Component bundle for the `validator:` scheme.
//!
//! [`ValidatorBundle`] owns the `[components.validator]` TOML key and carries
//! the format-enforcement opt-out into the registered
//! [`ValidatorComponent`].

use std::sync::Arc;

use camel_component_api::{CamelError, ComponentBundle, ComponentRegistrar};

use crate::component::ValidatorComponent;
use crate::config::DEFAULT_VALIDATE_FORMATS;
use crate::xsd_bridge::XsdBridgeBackend;

/// Bundle that registers the `validator:` scheme from the
/// `[components.validator]` config block.
pub struct ValidatorBundle {
    validate_formats: bool,
    xsd_backend: Arc<XsdBridgeBackend>,
}

impl ValidatorBundle {
    /// The XSD bridge backend owned by the component this bundle registers,
    /// retained so boot can drain it on shutdown.
    pub fn xsd_bridge_backend(&self) -> Option<Arc<XsdBridgeBackend>> {
        Some(Arc::clone(&self.xsd_backend))
    }
}

impl ComponentBundle for ValidatorBundle {
    fn config_key() -> &'static str {
        "validator"
    }

    fn from_toml(value: toml::Value) -> Result<Self, CamelError> {
        let table = match value {
            toml::Value::Table(table) => table,
            other => {
                return Err(CamelError::Config(format!(
                    "components.validator must be a table, got {other}"
                )));
            }
        };

        let mut validate_formats = DEFAULT_VALIDATE_FORMATS;
        for (key, value) in table {
            match key.as_str() {
                "validate-formats" => {
                    validate_formats = match value {
                        toml::Value::Boolean(flag) => flag,
                        other => {
                            return Err(CamelError::Config(format!(
                                "components.validator.validate-formats must be a boolean, got {other}"
                            )));
                        }
                    };
                }
                other => {
                    return Err(CamelError::Config(format!(
                        "unknown field `{other}` in components.validator"
                    )));
                }
            }
        }

        Ok(Self {
            validate_formats,
            xsd_backend: Arc::new(XsdBridgeBackend::new()),
        })
    }

    fn register_all(self, ctx: &mut dyn ComponentRegistrar) {
        let component = ValidatorComponent::with_validate_formats_and_backend(
            self.validate_formats,
            self.xsd_backend,
        );
        ctx.register_component_dyn(Arc::new(component));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::test_support::PanicRuntimeObservability;
    use camel_component_api::{
        Body, Component, Exchange, Message, NoOpComponentContext, ProducerContext,
        RuntimeObservability,
    };
    use tower::ServiceExt;

    struct TestRegistrar {
        components: Vec<Arc<dyn Component>>,
    }

    impl ComponentRegistrar for TestRegistrar {
        fn register_component_dyn(&mut self, component: Arc<dyn Component>) {
            self.components.push(component);
        }
    }

    fn bundle_from(raw: &str) -> ValidatorBundle {
        let value: toml::Value = toml::from_str(raw).expect("valid TOML");
        ValidatorBundle::from_toml(value).expect("bundle must parse")
    }

    fn email_schema_file() -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut file = tempfile::Builder::new().suffix(".json").tempfile().unwrap();
        file.write_all(
            br#"{"type":"object","properties":{"email":{"type":"string","format":"email"}},"required":["email"]}"#,
        )
        .unwrap();
        file
    }

    fn registered_component(raw: &str) -> Arc<dyn Component> {
        let mut registrar = TestRegistrar { components: vec![] };
        bundle_from(raw).register_all(&mut registrar);
        assert_eq!(
            registrar.components.len(),
            1,
            "bundle must register exactly one component"
        );
        registrar.components.pop().expect("registered component")
    }

    fn rt() -> Arc<dyn RuntimeObservability> {
        Arc::new(PanicRuntimeObservability)
    }

    async fn validate_malformed_email(raw_config: &str) -> bool {
        let schema = email_schema_file();
        let uri = format!("validator:{}", schema.path().display());
        let component = registered_component(raw_config);
        let endpoint = component
            .create_endpoint(&uri, &NoOpComponentContext)
            .expect("registered component must create the endpoint");
        let producer = endpoint
            .create_producer(rt(), &ProducerContext::new())
            .expect("producer");
        let exchange = Exchange::new(Message::new(Body::Text(
            "{\"email\":\"not-an-email\"}".to_string(),
        )));
        producer.oneshot(exchange).await.is_ok()
    }

    #[test]
    fn config_key_is_validator() {
        assert_eq!(ValidatorBundle::config_key(), "validator");
    }

    #[test]
    fn from_toml_empty_uses_default_true() {
        let bundle = bundle_from("");
        assert!(bundle.validate_formats);
    }

    #[test]
    fn from_toml_parses_false() {
        let bundle = bundle_from("validate-formats = false\n");
        assert!(!bundle.validate_formats);
    }

    #[test]
    fn from_toml_parses_true() {
        let bundle = bundle_from("validate-formats = true\n");
        assert!(bundle.validate_formats);
    }

    #[test]
    fn from_toml_rejects_string_value() {
        let value: toml::Value = toml::from_str("validate-formats = \"no\"\n").unwrap();
        let err = ValidatorBundle::from_toml(value)
            .err()
            .expect("a string must be rejected");
        assert!(
            err.to_string()
                .contains("components.validator.validate-formats"),
            "error must name components.validator.validate-formats: {err}"
        );
    }

    #[test]
    fn from_toml_rejects_unknown_field() {
        let value: toml::Value = toml::from_str("bogus = 1\n").unwrap();
        let err = ValidatorBundle::from_toml(value)
            .err()
            .expect("unknown field must be rejected");
        assert!(
            err.to_string().contains("bogus"),
            "error must name bogus: {err}"
        );
    }

    #[tokio::test]
    async fn opt_out_flows_through_registered_component() {
        assert!(
            validate_malformed_email("validate-formats = false\n").await,
            "opt-out must reach the registered component and accept the malformed \
             email"
        );
    }

    #[tokio::test]
    async fn default_config_rejects_format_violation() {
        assert!(
            !validate_malformed_email("").await,
            "default config must reject the malformed email"
        );
    }
}
