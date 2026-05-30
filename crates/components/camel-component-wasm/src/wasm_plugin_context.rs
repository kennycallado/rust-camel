use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store};

use camel_core::Registry;

use crate::config::WasmConfig;
use crate::error::WasmError;
use crate::runtime::{WasmHostState, WasmRuntime};

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
    async fn build(
        module_path: impl AsRef<Path>,
        wasm_config: WasmConfig,
        registry: Arc<std::sync::Mutex<Registry>>,
        setup_linker: impl FnOnce(&mut Linker<WasmHostState>) -> Result<(), WasmError>,
        module_label: &str,
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
                    "WASM {module_label} not found: {}",
                    module_path.display()
                ))
            } else {
                WasmError::CompilationFailed(format!(
                    "Failed to compile WASM {module_label} {}: {}",
                    module_path.display(),
                    e
                ))
            }
        })?;

        let mut linker: Linker<WasmHostState> = Linker::new(&engine);

        wasmtime_wasi::p2::add_to_linker_async(&mut linker)
            .map_err(|e| WasmError::CompilationFailed(e.to_string()))?;

        setup_linker(&mut linker)?;

        let epoch_ticker =
            crate::epoch::EpochTicker::start(engine.clone(), wasm_config.epoch_interval());

        let state_store = crate::state_store::StateStore::new();

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

    pub async fn new(
        module_path: impl AsRef<Path>,
        wasm_config: WasmConfig,
        registry: Arc<std::sync::Mutex<Registry>>,
        init_config: HashMap<String, String>,
    ) -> Result<Self, WasmError> {
        let ctx = Self::build(
            module_path,
            wasm_config,
            registry.clone(),
            |linker| {
                crate::host_functions::add_security_policy_to_linker(linker)
                    .map_err(|e| WasmError::CompilationFailed(e.to_string()))
            },
            "authorization policy",
        )
        .await?;

        {
            let host_state = WasmRuntime::create_host_state(
                registry,
                HashMap::new(),
                ctx.state_store.clone(),
                tokio::runtime::Handle::current(),
            );
            let mut store = Store::new(&ctx.engine, host_state);
            store.limiter(|state| &mut state.limits);
            store.set_epoch_deadline(ctx.config.epoch_deadline());

            let plugin = crate::security_policy_bindings::AuthorizationPolicy::instantiate_async(
                &mut store,
                &ctx.component,
                &ctx.linker,
            )
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
                    ctx.module_path.display(),
                    e
                )));
            }
        }

        Ok(ctx)
    }

    pub async fn new_bean(
        module_path: impl AsRef<Path>,
        wasm_config: WasmConfig,
        registry: Arc<std::sync::Mutex<Registry>>,
        bean_config: HashMap<String, String>,
    ) -> Result<(Self, Vec<String>), WasmError> {
        let ctx = Self::build(
            module_path,
            wasm_config,
            registry.clone(),
            |linker| {
                crate::host_functions::add_bean_to_linker(linker)
                    .map_err(|e| WasmError::CompilationFailed(e.to_string()))
            },
            "bean",
        )
        .await?;

        let methods = {
            let host_state = WasmRuntime::create_host_state(
                registry,
                HashMap::new(),
                ctx.state_store.clone(),
                tokio::runtime::Handle::current(),
            );
            let mut store = Store::new(&ctx.engine, host_state);
            store.limiter(|state| &mut state.limits);
            store.set_epoch_deadline(ctx.config.epoch_deadline());

            let plugin = crate::bean_bindings::Bean::instantiate_async(
                &mut store,
                &ctx.component,
                &ctx.linker,
            )
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
                    ctx.module_path.display(),
                    e
                )));
            }

            plugin
                .call_methods(&mut store)
                .await
                .map_err(|e| WasmError::GuestPanic(e.to_string()))?
        };

        Ok((ctx, methods))
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
        self.config.classify_error(&self.module_path, e)
    }
}
