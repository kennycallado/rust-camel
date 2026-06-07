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

pub mod authorization_policy;
pub mod bean;
pub mod bean_bindings;
pub mod bindings;
pub mod bundle;
pub mod config;
pub mod endpoint;
pub mod epoch;
pub mod error;
pub mod health;
pub mod host_functions;
pub mod producer;
pub mod runtime;
pub mod security_policy;
pub mod security_policy_bindings;
pub mod serde_bridge;
pub mod state_store;
pub mod wasm_plugin_context;

pub use authorization_policy::{WasmAuthorizationPolicyEvaluator, build_permission_registry};
pub use bundle::WasmBundle;
pub use config::WasmConfig;
pub use endpoint::WasmEndpoint;
pub use epoch::EpochTicker;
pub use error::{TrapReason, WasmError};
pub use health::WasmHealthCheck;
pub use security_policy::{WasmSecurityPolicy, build_security_policy_registry};
pub use state_store::StateStore;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use camel_api::CamelError;
use camel_component_api::{Component, ComponentContext, Endpoint};
use camel_core::Registry;

pub struct WasmComponent {
    registry: Arc<std::sync::Mutex<Registry>>,
    base_dir: PathBuf,
    engine_loaded: Arc<AtomicBool>,
}

impl WasmComponent {
    pub fn new(registry: Arc<std::sync::Mutex<Registry>>, base_dir: PathBuf) -> Self {
        Self {
            registry,
            base_dir,
            // TODO: set to true when WASM module is successfully loaded/instantiated
            engine_loaded: Arc::new(AtomicBool::new(false)),
        }
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
        ctx: &dyn ComponentContext,
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

        let health_check = WasmHealthCheck::new(Arc::clone(&self.engine_loaded));
        ctx.register_current_route_health_check(Arc::new(health_check));

        Ok(Box::new(WasmEndpoint::new(
            uri.to_string(),
            module_path,
            self.registry.clone(),
            wasm_config,
        )))
    }
}
