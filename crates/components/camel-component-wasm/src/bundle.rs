use std::path::PathBuf;
use std::sync::Arc;

use camel_component_api::{CamelError, ComponentBundle, ComponentRegistrar};
use camel_core::Registry;

use crate::WasmComponent;

pub struct WasmBundle {
    registry: Arc<std::sync::Mutex<Registry>>,
    base_dir: PathBuf,
}

impl WasmBundle {
    pub fn new(registry: Arc<std::sync::Mutex<Registry>>, base_dir: PathBuf) -> Self {
        Self { registry, base_dir }
    }
}

impl ComponentBundle for WasmBundle {
    fn config_key() -> &'static str {
        "wasm"
    }

    fn from_toml(_value: toml::Value) -> Result<Self, CamelError> {
        Err(CamelError::Config(
            "WasmBundle requires registry and base_dir — use WasmBundle::new() instead".to_string(),
        ))
    }

    fn register_all(self, ctx: &mut dyn ComponentRegistrar) {
        let component = WasmComponent::new(self.registry, self.base_dir);
        ctx.register_component_dyn(Arc::new(component));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_core::Registry;

    struct TestRegistrar {
        schemes: Vec<String>,
    }

    impl ComponentRegistrar for TestRegistrar {
        fn register_component_dyn(&mut self, component: Arc<dyn camel_component_api::Component>) {
            self.schemes.push(component.scheme().to_string());
        }
    }

    #[test]
    fn wasm_bundle_from_toml_returns_error() {
        let value: toml::Value = toml::from_str("").unwrap();
        let result = WasmBundle::from_toml(value);
        assert!(result.is_err());
    }

    #[test]
    fn wasm_bundle_registers_wasm_scheme() {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let bundle = WasmBundle::new(registry, PathBuf::from("."));
        let mut registrar = TestRegistrar { schemes: vec![] };

        bundle.register_all(&mut registrar);

        assert_eq!(registrar.schemes, vec!["wasm"]);
    }
}
