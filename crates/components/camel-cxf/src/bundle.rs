use std::sync::Arc;

use camel_component_api::{CamelError, ComponentBundle, ComponentRegistrar};

use crate::{CxfBridgePool, CxfComponent, config::CxfPoolConfig};

pub struct CxfBundle {
    pool: Arc<CxfBridgePool>,
}

impl CxfBundle {
    pub fn new(cfg: CxfPoolConfig) -> Result<Self, CamelError> {
        let pool = CxfBridgePool::from_config(cfg)?;
        Ok(Self {
            pool: Arc::new(pool),
        })
    }

    pub fn pool(&self) -> Arc<CxfBridgePool> {
        Arc::clone(&self.pool)
    }
}

impl ComponentBundle for CxfBundle {
    fn config_key() -> &'static str {
        "cxf"
    }

    fn from_toml(value: toml::Value) -> Result<Self, CamelError> {
        let cfg: CxfPoolConfig = value
            .try_into()
            .map_err(|e: toml::de::Error| CamelError::Config(e.to_string()))?;
        Self::new(cfg)
    }

    fn register_all(self, ctx: &mut dyn ComponentRegistrar) {
        ctx.register_component_dyn(Arc::new(CxfComponent::new(self.pool)));
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
    fn cxf_bundle_from_toml_valid() {
        let toml_str = r#"
            max_bridges = 2
        "#;
        let value: toml::Value = toml::from_str(toml_str).expect("valid toml");
        let result = CxfBundle::from_toml(value);
        assert!(
            result.is_ok(),
            "valid CXF toml must parse: {:?}",
            result.err()
        );
    }

    #[test]
    fn cxf_bundle_registers_cxf_scheme() {
        let toml_str = r#"
            max_bridges = 1
        "#;
        let value: toml::Value = toml::from_str(toml_str).expect("valid toml");
        let bundle = CxfBundle::from_toml(value).expect("bundle from valid toml");
        let mut registrar = TestRegistrar { schemes: vec![] };

        bundle.register_all(&mut registrar);

        assert_eq!(registrar.schemes, vec!["cxf"]);
    }

    #[test]
    fn cxf_bundle_from_toml_returns_error_on_invalid_config() {
        let mut table = toml::map::Map::new();
        table.insert(
            "max_bridges".to_string(),
            toml::Value::String("not-a-number".to_string()),
        );

        let result = CxfBundle::from_toml(toml::Value::Table(table));
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
