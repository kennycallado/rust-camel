use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store};

use camel_core::Registry;

use crate::config::WasmConfig;
use crate::error::{TrapReason, WasmError};
use crate::runtime::{WasmHostState, WasmRuntime};
use crate::security_policy_bindings::AuthorizationPolicy as AuthorizationPolicyGuest;

pub struct WasmPluginContext {
    pub engine: Engine,
    pub linker: Linker<WasmHostState>,
    pub component: Component,
    pub module_path: PathBuf,
    pub config: WasmConfig,
    pub registry: Arc<std::sync::Mutex<Registry>>,
    pub state_store: crate::state_store::StateStore,
    #[allow(dead_code)]
    pub epoch_ticker: crate::epoch::EpochTicker,
}

impl WasmPluginContext {
    pub async fn new(
        module_path: impl AsRef<Path>,
        wasm_config: WasmConfig,
        registry: Arc<std::sync::Mutex<Registry>>,
        init_config: HashMap<String, String>,
    ) -> Result<Self, WasmError> {
        let module_path = module_path.as_ref().to_path_buf();

        let mut config = Config::new();
        config.wasm_component_model(true);
        config.epoch_interruption(true);

        let engine =
            Engine::new(&config).map_err(|e| WasmError::CompilationFailed(e.to_string()))?;

        let component = Component::from_file(&engine, &module_path).map_err(|e| {
            if !module_path.exists() {
                WasmError::ModuleNotFound(format!(
                    "WASM authorization policy not found: {}",
                    module_path.display()
                ))
            } else {
                WasmError::CompilationFailed(format!(
                    "Failed to compile WASM authorization policy {}: {}",
                    module_path.display(),
                    e
                ))
            }
        })?;

        let mut linker: Linker<WasmHostState> = Linker::new(&engine);

        wasmtime_wasi::p2::add_to_linker_async(&mut linker)
            .map_err(|e| WasmError::CompilationFailed(e.to_string()))?;

        crate::host_functions::add_security_policy_to_linker(&mut linker)
            .map_err(|e| WasmError::CompilationFailed(e.to_string()))?;

        let epoch_ticker =
            crate::epoch::EpochTicker::start(engine.clone(), wasm_config.epoch_interval());

        let state_store = crate::state_store::StateStore::new();

        {
            let host_state = WasmRuntime::create_host_state(
                registry.clone(),
                HashMap::new(),
                state_store.clone(),
                tokio::runtime::Handle::current(),
            );
            let mut store = Store::new(&engine, host_state);
            store.limiter(|state| &mut state.limits);
            store.set_epoch_deadline(wasm_config.epoch_deadline());

            let plugin =
                AuthorizationPolicyGuest::instantiate_async(&mut store, &component, &linker)
                    .await
                    .map_err(|e| WasmError::InstantiationFailed(e.to_string()))?;

            let mut config_pairs: Vec<(String, String)> = init_config.into_iter().collect();
            config_pairs.sort_by(|a, b| a.0.cmp(&b.0));
            let init_result: Result<(), String> = plugin
                .call_init(&mut store, &config_pairs)
                .await
                .map_err(|e| WasmError::GuestPanic(e.to_string()))?;

            if let Err(e) = init_result {
                return Err(WasmError::GuestPanic(format!(
                    "authorization policy init() failed for {}: {}",
                    module_path.display(),
                    e
                )));
            }
        }

        Ok(Self {
            engine,
            linker,
            component,
            module_path,
            config: wasm_config,
            registry,
            state_store,
            epoch_ticker,
        })
    }

    pub fn create_store(&self, properties: HashMap<String, Value>) -> Store<WasmHostState> {
        let host_state = WasmRuntime::create_host_state(
            self.registry.clone(),
            properties,
            self.state_store.clone(),
            tokio::runtime::Handle::current(),
        );
        let mut store = Store::new(&self.engine, host_state);
        store.limiter(|state| &mut state.limits);
        store.set_epoch_deadline(self.config.epoch_deadline());
        store
    }

    pub fn classify_error(&self, e: wasmtime::Error) -> WasmError {
        let name = self.module_path.display().to_string();
        if let Some(trap) = e.downcast_ref::<wasmtime::Trap>() {
            match WasmError::classify_trap(trap) {
                TrapReason::Timeout => WasmError::Timeout {
                    plugin: name,
                    timeout_secs: self.config.timeout_secs,
                },
                TrapReason::OutOfMemory => WasmError::OutOfMemory {
                    plugin: name,
                    max_memory_bytes: self.config.max_memory_bytes,
                },
                other => WasmError::Trap {
                    plugin: name,
                    reason: other,
                },
            }
        } else {
            WasmError::GuestPanic(e.to_string())
        }
    }
}
