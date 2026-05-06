pub mod bindings;
pub mod bundle;
pub mod endpoint;
pub mod error;
pub mod host_functions;
pub mod producer;
pub mod runtime;
pub mod serde_bridge;

pub use bundle::WasmBundle;
pub use endpoint::WasmEndpoint;
pub use error::WasmError;

use std::path::PathBuf;
use std::sync::Arc;

use camel_api::CamelError;
use camel_component_api::{Component, ComponentContext, Endpoint};
use camel_core::Registry;

pub struct WasmComponent {
    registry: Arc<std::sync::Mutex<Registry>>,
    base_dir: PathBuf,
}

impl WasmComponent {
    pub fn new(registry: Arc<std::sync::Mutex<Registry>>, base_dir: PathBuf) -> Self {
        Self { registry, base_dir }
    }

    fn validate_and_resolve_path(&self, uri_path: &str) -> Result<PathBuf, CamelError> {
        if PathBuf::from(uri_path).is_absolute() {
            return Err(CamelError::InvalidUri(
                "WASM path must be relative (not absolute)".to_string(),
            ));
        }

        if PathBuf::from(uri_path)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(CamelError::InvalidUri(
                "WASM path must not contain '..'".to_string(),
            ));
        }

        let resolved = self.base_dir.join(uri_path);
        let canonical = resolved.canonicalize().map_err(|_| {
            CamelError::ComponentNotFound(format!("WASM module not found: {}", resolved.display()))
        })?;

        let canonical_base = self.base_dir.canonicalize().map_err(|_| {
            CamelError::EndpointCreationFailed(format!(
                "failed to resolve base directory: {}",
                self.base_dir.display()
            ))
        })?;

        if !canonical.starts_with(&canonical_base) {
            return Err(CamelError::InvalidUri(
                "WASM path escapes project root".to_string(),
            ));
        }

        Ok(canonical)
    }
}

impl Component for WasmComponent {
    fn scheme(&self) -> &str {
        "wasm"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let uri_without_scheme = uri.strip_prefix("wasm:").ok_or_else(|| {
            CamelError::InvalidUri(format!("WASM URI must start with 'wasm:': {uri}"))
        })?;

        let path_part = uri_without_scheme.split('?').next().unwrap_or_default();
        if path_part.is_empty() {
            return Err(CamelError::InvalidUri(
                "WASM URI must include a module path".to_string(),
            ));
        }

        let module_path = self.validate_and_resolve_path(path_part)?;
        Ok(Box::new(WasmEndpoint::new(
            uri.to_string(),
            module_path,
            self.registry.clone(),
        )))
    }
}
