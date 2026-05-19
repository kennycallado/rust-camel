//! WebAssembly component for rust-camel — executes WASM modules as route processors via a Wasmtime runtime.
//!
//! Main types: `WasmComponent`, `WasmBundle`, `WasmConfig`, `WasmEndpoint`, `StateStore`.
//! Main modules: `runtime`, `bindings`, `state_store`, `host_functions`.
//!
//! # Limitations
//!
//! - Only the [Wasmtime](https://wasmtime.dev) runtime is supported; other WASM runtimes
//!   (Wasmer, WasmEdge, etc.) are not compatible.
//! - WASM modules must export a specific interface defined by `camel_wasm_bindings`; arbitrary
//!   WASM binaries cannot be dropped in without meeting this contract.
//! - The WASM sandbox has no access to the host filesystem or network by default;
//!   host functions are limited to what is explicitly exposed via `host_functions`.
//! - Epoch-based fuel/interruption requires the `epoch` feature; long-running modules
//!   without epoch support may block the async executor.

pub mod bean;
pub mod bean_bindings;
pub mod bindings;
pub mod bundle;
pub mod config;
pub mod endpoint;
pub mod epoch;
pub mod error;
pub mod host_functions;
pub mod producer;
pub mod runtime;
pub mod serde_bridge;
pub mod state_store;

pub use bundle::WasmBundle;
pub use config::WasmConfig;
pub use endpoint::WasmEndpoint;
pub use epoch::EpochTicker;
pub use error::{TrapReason, WasmError};
pub use state_store::StateStore;

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

        let (path_part, wasm_config) = crate::config::WasmConfig::from_uri(uri_without_scheme);
        if path_part.is_empty() {
            return Err(CamelError::InvalidUri(
                "WASM URI must include a module path".to_string(),
            ));
        }

        let module_path = self.validate_and_resolve_path(&path_part)?;
        Ok(Box::new(WasmEndpoint::new(
            uri.to_string(),
            module_path,
            self.registry.clone(),
            wasm_config,
        )))
    }
}
