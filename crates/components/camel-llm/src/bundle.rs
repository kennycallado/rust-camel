use std::sync::Arc;

use camel_component_api::{CamelError, ComponentBundle, ComponentRegistrar};

use crate::LlmComponent;

pub struct LlmBundle;

impl ComponentBundle for LlmBundle {
    fn config_key() -> &'static str {
        "llm"
    }

    fn from_toml(_value: toml::Value) -> Result<Self, CamelError> {
        Ok(Self)
    }

    fn register_all(self, ctx: &mut dyn ComponentRegistrar) {
        ctx.register_component_dyn(Arc::new(LlmComponent));
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
    fn llm_bundle_from_toml_valid() {
        let value: toml::Value = toml::from_str("").expect("valid toml");
        let result = LlmBundle::from_toml(value);
        assert!(result.is_ok(), "valid LLM toml must parse: {:?}", result.err());
    }

    #[test]
    fn llm_bundle_registers_llm_scheme() {
        let value: toml::Value = toml::from_str("").expect("valid toml");
        let bundle = LlmBundle::from_toml(value).expect("bundle from valid toml");
        let mut registrar = TestRegistrar { schemes: vec![] };

        bundle.register_all(&mut registrar);

        assert_eq!(registrar.schemes, vec!["llm"]);
    }
}
