use std::sync::Arc;

use camel_component_api::{CamelError, ComponentBundle, ComponentRegistrar};

use crate::component::VectorComponent;

/// Bundle for the vector component.
///
/// Vector has no global config — all configuration is per-URI.
pub struct VectorBundle;

impl ComponentBundle for VectorBundle {
    fn config_key() -> &'static str {
        "vector"
    }

    fn from_toml(_value: toml::Value) -> Result<Self, CamelError> {
        // Vector has no global config; accept any TOML block.
        Ok(Self)
    }

    fn register_all(self, ctx: &mut dyn ComponentRegistrar) {
        ctx.register_component_dyn(Arc::new(VectorComponent));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestRegistrar {
        schemes: Vec<String>,
    }

    impl ComponentRegistrar for TestRegistrar {
        fn register_component_dyn(&mut self, component: Arc<dyn camel_component_api::Component>) {
            self.schemes.push(component.scheme().to_string());
        }
    }

    #[test]
    fn vector_bundle_config_key() {
        assert_eq!(VectorBundle::config_key(), "vector");
    }

    #[test]
    fn vector_bundle_from_toml_accepts_any() {
        let value: toml::Value = toml::from_str("").unwrap();
        let result = VectorBundle::from_toml(value);
        assert!(result.is_ok(), "vector bundle must accept any TOML");
    }

    #[test]
    fn vector_bundle_registers_expected_scheme() {
        let bundle = VectorBundle::from_toml(toml::Value::Table(toml::map::Map::new())).unwrap();
        let mut registrar = TestRegistrar { schemes: vec![] };

        bundle.register_all(&mut registrar);

        assert_eq!(registrar.schemes, vec!["vector"]);
    }
}
