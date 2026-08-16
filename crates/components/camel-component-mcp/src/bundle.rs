//! MCP component bundle — owns the `mcp` TOML config key (ComponentBundle).
//!
//! Deserializes [`McpGlobalConfig`] fail-fast at startup and registers the
//! [`crate::McpComponent`] (scheme `mcp:`, both Consumer and Producer roles).

use std::sync::Arc;

use camel_component_api::{CamelError, ComponentBundle, ComponentRegistrar};

use crate::McpComponent;
use crate::config::{McpGlobalConfig, validate_server_policy};

/// Bundle for the `mcp` config key.
pub struct McpBundle {
    config: McpGlobalConfig,
}

impl ComponentBundle for McpBundle {
    fn config_key() -> &'static str {
        "mcp"
    }

    fn from_toml(value: toml::Value) -> Result<Self, CamelError> {
        let config: McpGlobalConfig = value
            .try_into()
            .map_err(|e: toml::de::Error| CamelError::Config(e.to_string()))?;
        // Fail-fast: every named server must pass bind-policy validation at
        // startup, so an invalid entry (missing security policy, non-IP
        // literal bind, zero catalog caps) errors here, not at first route
        // start. Remote (Producer) entries are validated by deserialization
        // itself: the transport enum rejects unknown transports and unknown
        // keys are denied. The non-loopback advisory warning stays with the
        // consumer, which emits it once per start and names the bind.
        for (name, cfg) in &config.servers {
            validate_server_policy(name, cfg).map_err(CamelError::from)?;
        }
        Ok(Self { config })
    }

    fn register_all(self, ctx: &mut dyn ComponentRegistrar) {
        // `McpComponent::new` is infallible — construction performs no network
        // I/O; the `Result` mirrors the LLM component's constructor shape.
        let component = McpComponent::new(self.config)
            .expect("McpComponent::new cannot fail: no I/O at construction"); // allow-unwrap
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
    fn bundle_registers_component() {
        let value: toml::Value = toml::from_str("").expect("parse toml");
        let bundle = McpBundle::from_toml(value).expect("bundle");
        let mut registrar = TestRegistrar { schemes: vec![] };
        bundle.register_all(&mut registrar);
        assert_eq!(registrar.schemes, vec!["mcp"]);
    }

    #[test]
    fn bundle_rejects_unknown_keys() {
        let toml_str = "session = true";
        let value: toml::Value = toml::from_str(toml_str).expect("parse toml");
        let result = McpBundle::from_toml(value);
        assert!(
            result.is_err(),
            "from_toml should reject unknown config keys"
        );
    }
}
