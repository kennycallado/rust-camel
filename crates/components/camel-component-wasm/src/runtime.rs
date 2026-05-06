use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::WasiCtxBuilder;

use camel_core::Registry;

use crate::bindings::camel::plugin::types::WasmExchange;
use crate::bindings::Plugin;
use crate::error::WasmError;

pub struct WasmHostState {
    pub table: ResourceTable,
    pub wasi: wasmtime_wasi::WasiCtx,
    pub properties: HashMap<String, Value>,
    pub registry: Arc<std::sync::Mutex<Registry>>,
    pub call_depth: u32,
}

impl wasmtime_wasi::IoView for WasmHostState {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

impl wasmtime_wasi::WasiView for WasmHostState {
    fn ctx(&mut self) -> &mut wasmtime_wasi::WasiCtx {
        &mut self.wasi
    }
}

pub struct WasmRuntime {
    engine: Engine,
    linker: Linker<WasmHostState>,
    component: Component,
    module_path: PathBuf,
}

impl WasmRuntime {
    pub async fn new(
        module_path: impl AsRef<Path>,
    ) -> Result<Self, WasmError> {
        let module_path = module_path.as_ref().to_path_buf();

        let mut config = Config::new();
        config.async_support(true);
        config.wasm_component_model(true);

        let engine =
            Engine::new(&config).map_err(|e| WasmError::CompilationFailed(e.to_string()))?;

        let component = Component::from_file(&engine, &module_path).map_err(|e| {
            WasmError::ModuleNotFound(format!(
                "Failed to load WASM module {}: {}",
                module_path.display(),
                e
            ))
        })?;

        let mut linker: Linker<WasmHostState> = Linker::new(&engine);

        wasmtime_wasi::add_to_linker_async(&mut linker)
            .map_err(|e| WasmError::CompilationFailed(e.to_string()))?;

        crate::host_functions::add_to_linker(&mut linker)
            .map_err(|e| WasmError::CompilationFailed(e.to_string()))?;

        Ok(Self {
            engine,
            linker,
            component,
            module_path,
        })
    }

    pub fn create_host_state(
        registry: Arc<std::sync::Mutex<Registry>>,
        properties: HashMap<String, Value>,
    ) -> WasmHostState {
        WasmHostState {
            table: ResourceTable::new(),
            wasi: WasiCtxBuilder::new().inherit_stderr().build(),
            properties,
            registry,
            call_depth: 0,
        }
    }

    pub async fn call_init_once(&self, host_state: WasmHostState) -> Result<(), WasmError> {
        let mut store = Store::new(&self.engine, host_state);
        let plugin = Plugin::instantiate_async(&mut store, &self.component, &self.linker)
            .await
            .map_err(|e| WasmError::InstantiationFailed(e.to_string()))?;

        let result: Result<(), String> = plugin.call_init(&mut store).await.map_err(|e| {
            WasmError::InstantiationFailed(format!("init() invocation failed: {}", e))
        })?;

        if let Err(e) = result {
            tracing::debug!(
                "WASM init() returned error (optional hook): {} — {}",
                self.module_path.display(),
                e
            );
        }
        Ok(())
    }

    pub async fn call_process(
        &self,
        host_state: WasmHostState,
        exchange: WasmExchange,
    ) -> Result<WasmExchange, WasmError> {
        let mut store = Store::new(&self.engine, host_state);

        let plugin = Plugin::instantiate_async(&mut store, &self.component, &self.linker)
            .await
            .map_err(|e| WasmError::InstantiationFailed(e.to_string()))?;

        let result: Result<WasmExchange, crate::bindings::camel::plugin::types::WasmError> =
            plugin
                .call_process(&mut store, &exchange)
                .await
                .map_err(|e: wasmtime::Error| WasmError::GuestPanic(e.to_string()))?;

        result.map_err(|wasm_err| match wasm_err {
            crate::bindings::camel::plugin::types::WasmError::ProcessorError(s) => {
                WasmError::GuestPanic(s)
            }
            crate::bindings::camel::plugin::types::WasmError::TypeConversion(s) => {
                WasmError::TypeConversion(s)
            }
            crate::bindings::camel::plugin::types::WasmError::Io(s) => WasmError::Io(s),
            crate::bindings::camel::plugin::types::WasmError::Timeout => {
                WasmError::GuestPanic("guest timeout".to_string())
            }
        })
    }

    pub fn module_path(&self) -> &Path {
        &self.module_path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasm_host_state_creation() {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let props = HashMap::new();
        let state = WasmHostState {
            table: ResourceTable::new(),
            wasi: WasiCtxBuilder::new().inherit_stderr().build(),
            properties: props,
            registry,
            call_depth: 0,
        };
        assert!(state.properties.is_empty());
        assert_eq!(state.call_depth, 0);
    }
}
