use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::WasiCtxBuilder;

use camel_core::Registry;

use crate::bindings::Plugin;
use crate::bindings::camel::plugin::types::WasmExchange;
use crate::error::WasmError;

pub struct WasmHostState {
    pub table: ResourceTable,
    pub wasi: wasmtime_wasi::WasiCtx,
    pub properties: HashMap<String, Value>,
    pub registry: Arc<std::sync::Mutex<Registry>>,
    pub call_depth: u32,
    pub limits: wasmtime::StoreLimits,
    pub state_store: crate::state_store::StateStore,
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
    config: crate::config::WasmConfig,
    #[allow(dead_code)]
    epoch_ticker: crate::epoch::EpochTicker,
}

impl WasmRuntime {
    pub async fn new(
        module_path: impl AsRef<Path>,
        wasm_config: crate::config::WasmConfig,
    ) -> Result<Self, WasmError> {
        let module_path = module_path.as_ref().to_path_buf();

        let mut config = Config::new();
        config.async_support(true);
        config.wasm_component_model(true);
        config.epoch_interruption(true);

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

        let epoch_ticker =
            crate::epoch::EpochTicker::start(engine.clone(), wasm_config.epoch_interval());

        Ok(Self {
            engine,
            linker,
            component,
            module_path,
            config: wasm_config,
            epoch_ticker,
        })
    }

    pub fn create_host_state(
        registry: Arc<std::sync::Mutex<Registry>>,
        properties: HashMap<String, Value>,
        state_store: crate::state_store::StateStore,
    ) -> WasmHostState {
        WasmHostState {
            table: ResourceTable::new(),
            wasi: WasiCtxBuilder::new().inherit_stderr().build(),
            properties,
            registry,
            call_depth: 0,
            limits: wasmtime::StoreLimits::default(),
            state_store,
        }
    }

    /// Classify a wasmtime error into a structured WasmError.
    ///
    /// Downcasts to `wasmtime::Trap` first — if successful, routes to
    /// Timeout/OutOfMemory/Trap variants. Otherwise falls back to GuestPanic.
    fn classify_error(&self, e: wasmtime::Error) -> WasmError {
        let plugin_name = self.module_path.display().to_string();
        if let Some(trap) = e.downcast_ref::<wasmtime::Trap>() {
            let reason = WasmError::classify_trap(trap);
            match reason {
                crate::error::TrapReason::Timeout => WasmError::Timeout {
                    plugin: plugin_name,
                    timeout_secs: self.config.timeout_secs,
                },
                crate::error::TrapReason::OutOfMemory => WasmError::OutOfMemory {
                    plugin: plugin_name,
                    max_memory_bytes: self.config.max_memory_bytes,
                },
                other => WasmError::Trap {
                    plugin: plugin_name,
                    reason: other,
                },
            }
        } else {
            WasmError::GuestPanic(e.to_string())
        }
    }

    pub async fn call_init_once(
        &self,
        registry: Arc<std::sync::Mutex<Registry>>,
        properties: HashMap<String, Value>,
        state_store: crate::state_store::StateStore,
    ) -> Result<(), WasmError> {
        let host_state = Self::create_host_state(registry, properties, state_store);
        let mut store = Store::new(&self.engine, host_state);
        store.limiter(|state| &mut state.limits);
        store.set_epoch_deadline(self.config.epoch_deadline());

        let plugin = Plugin::instantiate_async(&mut store, &self.component, &self.linker)
            .await
            .map_err(|e| WasmError::InstantiationFailed(e.to_string()))?;

        let result: Result<(), String> = plugin
            .call_init(&mut store)
            .await
            .map_err(|e| self.classify_error(e))?;

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
        registry: Arc<std::sync::Mutex<Registry>>,
        properties: HashMap<String, Value>,
        state_store: crate::state_store::StateStore,
        exchange: WasmExchange,
    ) -> Result<WasmExchange, WasmError> {
        let host_state = Self::create_host_state(registry, properties, state_store);
        let mut store = Store::new(&self.engine, host_state);
        store.limiter(|state| &mut state.limits);
        store.set_epoch_deadline(self.config.epoch_deadline());

        let plugin = Plugin::instantiate_async(&mut store, &self.component, &self.linker)
            .await
            .map_err(|e| WasmError::InstantiationFailed(e.to_string()))?;

        let result: Result<WasmExchange, crate::bindings::camel::plugin::types::WasmError> = plugin
            .call_process(&mut store, &exchange)
            .await
            .map_err(|e| self.classify_error(e))?;

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
            limits: wasmtime::StoreLimits::default(),
            state_store: crate::state_store::StateStore::new(),
        };
        assert!(state.properties.is_empty());
        assert_eq!(state.call_depth, 0);
    }

    #[test]
    fn test_host_state_has_limits_field() {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let state = WasmRuntime::create_host_state(
            registry,
            HashMap::new(),
            crate::state_store::StateStore::new(),
        );
        let _limits: &wasmtime::StoreLimits = &state.limits;
    }

    #[test]
    fn test_epoch_deadline_set_on_store() {
        let mut config = wasmtime::Config::new();
        config.epoch_interruption(true);
        config.wasm_component_model(true);
        config.async_support(true);
        let engine = Engine::new(&config).unwrap();
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let host_state = WasmRuntime::create_host_state(
            registry,
            HashMap::new(),
            crate::state_store::StateStore::new(),
        );
        let mut store = Store::new(&engine, host_state);
        store.set_epoch_deadline(500);
        // NOTE: wasmtime v31 does not expose `get_epoch_deadline()` on Store,
        // so we cannot assert the value was set. This test verifies the API
        // compiles and does not panic at runtime. The actual deadline enforcement
        // is validated indirectly by the epoch_interruption integration tests.
    }

    #[test]
    fn test_store_limiter_uses_host_state_limits() {
        let mut config = wasmtime::Config::new();
        config.epoch_interruption(true);
        config.wasm_component_model(true);
        config.async_support(true);
        let engine = Engine::new(&config).unwrap();
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let mut host_state = WasmRuntime::create_host_state(
            registry,
            HashMap::new(),
            crate::state_store::StateStore::new(),
        );
        host_state.limits = wasmtime::StoreLimitsBuilder::new()
            .memory_size(1024)
            .build();
        let mut store = Store::new(&engine, host_state);
        store.limiter(|state| &mut state.limits);
        // Verifies store.limiter accepts WasmHostState::limits — compilation test
    }
}
