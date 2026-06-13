use std::sync::Arc;

use camel_component_api::{CamelError, ComponentBundle, ComponentRegistrar};

use crate::LlmComponent;
use crate::config::LlmGlobalConfig;
use crate::provider_factory::{ProviderMap, build_provider_map};

pub struct LlmBundle {
    config: LlmGlobalConfig,
    providers: ProviderMap,
}

impl ComponentBundle for LlmBundle {
    fn config_key() -> &'static str {
        "llm"
    }

    fn from_toml(value: toml::Value) -> Result<Self, CamelError> {
        let config: LlmGlobalConfig = value
            .try_into()
            .map_err(|e: toml::de::Error| CamelError::Config(e.to_string()))?;
        // Fail-fast: validate all providers can be constructed at startup
        let providers = build_provider_map(&config).map_err(CamelError::from)?;
        Ok(Self { config, providers })
    }

    fn register_all(self, ctx: &mut dyn ComponentRegistrar) {
        let component = LlmComponent::from_parts(self.config, self.providers);
        ctx.register_component_dyn(Arc::new(component));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::{ComponentBundle, ComponentRegistrar};
    use std::sync::Arc;

    struct TestRegistrar {
        schemes: Vec<String>,
    }

    impl ComponentRegistrar for TestRegistrar {
        fn register_component_dyn(&mut self, component: Arc<dyn camel_component_api::Component>) {
            self.schemes.push(component.scheme().to_string());
        }
    }

    #[test]
    fn bundle_from_toml_registers_llm_scheme() {
        let toml_str = r#"
[providers.test]
type = "mock"
response = "echo"
"#;
        let value: toml::Value = toml::from_str(toml_str).expect("parse toml");
        let bundle = LlmBundle::from_toml(value).expect("bundle");
        let mut registrar = TestRegistrar { schemes: vec![] };
        bundle.register_all(&mut registrar);
        assert_eq!(registrar.schemes, vec!["llm"]);
    }

    #[test]
    fn config_key_is_llm() {
        assert_eq!(LlmBundle::config_key(), "llm");
    }

    #[test]
    fn from_toml_fails_on_unsupported_provider() {
        // A provider type that doesn't match any enum variant should fail
        // at TOML deserialization, regardless of feature flags.
        let toml_str = r#"
[providers.unknown-test]
type = "anthropic"
api_key = "test-key"
default_model = "claude-3"
"#;
        let value: toml::Value = toml::from_str(toml_str).expect("parse toml");
        let result = LlmBundle::from_toml(value);
        assert!(
            result.is_err(),
            "from_toml should fail for unsupported provider type"
        );
    }
}
