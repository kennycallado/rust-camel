use std::sync::Arc;

use camel_component_api::{CamelError, ComponentBundle, ComponentRegistrar};

use crate::LlmComponent;
use crate::config::LlmGlobalConfig;
use crate::decision::factory;
use crate::decision::factory::DecisionProviderMap;
use crate::provider_factory::{ProviderMap, build_provider_map};

pub struct LlmBundle {
    config: LlmGlobalConfig,
    providers: ProviderMap,
    decision_providers: DecisionProviderMap,
}

impl ComponentBundle for LlmBundle {
    fn config_key() -> &'static str {
        "llm"
    }

    fn from_toml(value: toml::Value) -> Result<Self, CamelError> {
        let config: LlmGlobalConfig = value
            .try_into()
            .map_err(|e: toml::de::Error| CamelError::Config(e.to_string()))?;
        // Validate config semantics (reject zero timeouts/concurrency)
        config.validate()?;
        // Fail-fast: validate all providers can be constructed at startup
        let providers = build_provider_map(&config).map_err(CamelError::from)?;
        let decision_providers =
            factory::build_decision_provider_map(&config).map_err(CamelError::from)?;
        Ok(Self {
            config,
            providers,
            decision_providers,
        })
    }

    fn register_all(self, ctx: &mut dyn ComponentRegistrar) {
        let component = LlmComponent::from_parts(self.config, self.providers)
            .with_decision_providers(self.decision_providers);
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

    struct ComponentCapture {
        component: Option<Arc<dyn camel_component_api::Component>>,
    }

    impl ComponentRegistrar for ComponentCapture {
        fn register_component_dyn(&mut self, component: Arc<dyn camel_component_api::Component>) {
            self.component = Some(component);
        }
    }

    #[test]
    fn bundle_wires_decision_providers() {
        use camel_component_api::NoOpComponentContext;

        let toml_str = format!(
            "[decision_providers.fake]\ntype = \"mock\"\n{}",
            crate::decision::test_support::TRIAGE_TOML
        );
        let value: toml::Value = toml::from_str(&toml_str).expect("parse toml");
        let bundle = LlmBundle::from_toml(value).expect("bundle");
        let mut registrar = ComponentCapture { component: None };
        bundle.register_all(&mut registrar);
        let component = registrar.component.expect("llm component registered");
        component
            .create_endpoint(
                "llm:decide?provider=fake&questions=triage",
                &NoOpComponentContext,
            )
            .expect("decide endpoint resolves through bundle");
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
