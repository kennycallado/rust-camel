use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store};

use camel_api::{CamelError, Exchange};
use camel_bean::BeanProcessor;
use camel_core::Registry;

use crate::bean_bindings::Bean as BeanGuest;
use crate::error::WasmError;
use crate::runtime::{WasmHostState, WasmRuntime};
use crate::serde_bridge;

pub struct WasmBean {
    engine: Engine,
    linker: Linker<WasmHostState>,
    component: Component,
    module_path: PathBuf,
    methods: Vec<String>,
    config: crate::config::WasmConfig,
    registry: Arc<std::sync::Mutex<Registry>>,
    state_store: crate::state_store::StateStore,
    #[allow(dead_code)]
    epoch_ticker: crate::epoch::EpochTicker,
}

impl WasmBean {
    pub async fn new(
        module_path: impl AsRef<Path>,
        wasm_config: crate::config::WasmConfig,
        registry: Arc<std::sync::Mutex<Registry>>,
        bean_config: HashMap<String, String>,
    ) -> Result<Self, WasmError> {
        let module_path = module_path.as_ref().to_path_buf();

        let mut config = Config::new();
        config.wasm_component_model(true);
        config.epoch_interruption(true);

        let engine =
            Engine::new(&config).map_err(|e| WasmError::CompilationFailed(e.to_string()))?;

        let component = Component::from_file(&engine, &module_path).map_err(|e| {
            if !module_path.exists() {
                WasmError::ModuleNotFound(format!("WASM bean not found: {}", module_path.display()))
            } else {
                WasmError::CompilationFailed(format!(
                    "Failed to compile WASM bean {}: {}",
                    module_path.display(),
                    e
                ))
            }
        })?;

        let mut linker: Linker<WasmHostState> = Linker::new(&engine);

        wasmtime_wasi::p2::add_to_linker_async(&mut linker)
            .map_err(|e| WasmError::CompilationFailed(e.to_string()))?;

        crate::host_functions::add_bean_to_linker(&mut linker)
            .map_err(|e| WasmError::CompilationFailed(e.to_string()))?;

        let epoch_ticker =
            crate::epoch::EpochTicker::start(engine.clone(), wasm_config.epoch_interval());

        let state_store = crate::state_store::StateStore::new();
        let host_state =
            WasmRuntime::create_host_state(registry.clone(), HashMap::new(), state_store.clone());
        let mut store = Store::new(&engine, host_state);
        store.limiter(|state| &mut state.limits);
        store.set_epoch_deadline(wasm_config.epoch_deadline());

        let plugin = BeanGuest::instantiate_async(&mut store, &component, &linker)
            .await
            .map_err(|e| WasmError::InstantiationFailed(e.to_string()))?;

        let mut config_pairs: Vec<(String, String)> = bean_config.into_iter().collect();
        config_pairs.sort_by(|a, b| a.0.cmp(&b.0));
        let init_result: Result<(), String> = plugin
            .call_init(&mut store, &config_pairs)
            .await
            .map_err(|e| WasmError::GuestPanic(e.to_string()))?;

        if let Err(e) = init_result {
            return Err(WasmError::GuestPanic(format!(
                "bean init() failed for {}: {}",
                module_path.display(),
                e
            )));
        }

        let methods = plugin
            .call_methods(&mut store)
            .await
            .map_err(|e| WasmError::GuestPanic(e.to_string()))?;

        let _ = plugin;

        Ok(Self {
            engine,
            linker,
            component,
            module_path,
            methods,
            config: wasm_config,
            registry,
            state_store,
            epoch_ticker,
        })
    }

    fn classify_error(&self, e: wasmtime::Error) -> WasmError {
        let name = self.module_path.display().to_string();
        if let Some(trap) = e.downcast_ref::<wasmtime::Trap>() {
            let reason = WasmError::classify_trap(trap);
            match reason {
                crate::error::TrapReason::Timeout => WasmError::Timeout {
                    plugin: name,
                    timeout_secs: self.config.timeout_secs,
                },
                crate::error::TrapReason::OutOfMemory => WasmError::OutOfMemory {
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

#[async_trait]
impl BeanProcessor for WasmBean {
    async fn call(&self, method: &str, exchange: &mut Exchange) -> Result<(), CamelError> {
        if !self.methods.iter().any(|m| m == method) {
            return Err(CamelError::ProcessorError(format!(
                "unknown bean method: {method}"
            )));
        }

        let host_state = WasmRuntime::create_host_state(
            self.registry.clone(),
            exchange.properties.clone(),
            self.state_store.clone(),
        );
        let mut store = Store::new(&self.engine, host_state);
        store.limiter(|state| &mut state.limits);
        store.set_epoch_deadline(self.config.epoch_deadline());

        let plugin = BeanGuest::instantiate_async(&mut store, &self.component, &self.linker)
            .await
            .map_err(|e| WasmError::InstantiationFailed(e.to_string()))?;

        let wasm_exchange = serde_bridge::exchange_to_wasm(exchange);
        let bean_exchange = wasm_exchange.into();

        let result = plugin
            .call_invoke(&mut store, method, &bean_exchange)
            .await
            .map_err(|e| self.classify_error(e))?;

        match result {
            Ok(bean_result) => {
                let wasm_result =
                    crate::bindings::camel::plugin::types::WasmExchange::from(bean_result);
                serde_bridge::wasm_to_exchange(wasm_result, exchange);
                Ok(())
            }
            Err(wasm_err) => Err(WasmError::GuestPanic(format!(
                "bean method '{method}' failed: {wasm_err:?}"
            ))
            .into()),
        }
    }

    fn methods(&self) -> Vec<String> {
        self.methods.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasm_bean_host_state_creation() {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let host_state = WasmRuntime::create_host_state(
            registry,
            HashMap::new(),
            crate::state_store::StateStore::new(),
        );
        assert!(host_state.properties.is_empty());
    }

    #[test]
    fn test_config_vec_conversion() {
        let config = HashMap::from([
            ("key1".to_string(), "val1".to_string()),
            ("key2".to_string(), "val2".to_string()),
        ]);
        let pairs: Vec<(String, String)> = config.into_iter().collect();
        assert_eq!(pairs.len(), 2);
    }

    #[test]
    fn test_empty_config_vec() {
        let config: HashMap<String, String> = HashMap::new();
        let pairs: Vec<(String, String)> = config.into_iter().collect();
        assert!(pairs.is_empty());
    }
}
