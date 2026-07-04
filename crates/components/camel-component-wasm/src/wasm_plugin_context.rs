use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use wasmtime::component::{Component, Linker};
use wasmtime::{AsContextMut, Config, Engine, Store};

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
    pub capabilities: crate::capabilities::WasmCapabilities,
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
        capabilities: crate::capabilities::WasmCapabilities,
    ) -> Result<Self, WasmError> {
        let module_path = module_path.as_ref().to_path_buf();

        let mut config = Config::new();
        config.wasm_component_model(true);
        config.epoch_interruption(true);
        // Required for component-model async imports/exports (the `async func`
        // WIT feature). Without this, guests that import `camel:plugin/host`
        // (which contains async `camel-call`/`camel-poll`) fail instantiation
        // with "matching implementation not found in the linker". Mirrors
        // `WasmRuntime::new` in runtime.rs.
        config.concurrency_support(true);

        let engine =
            Engine::new(&config).map_err(|e| WasmError::CompilationFailed(e.to_string()))?;

        // Existence check first (preserve ModuleNotFound error variant)
        if !module_path.exists() {
            return Err(WasmError::ModuleNotFound(format!(
                "WASM {module_label} not found: {}",
                module_path.display()
            )));
        }

        // Size cap: reject oversized modules before compilation (R4-H3)
        crate::config::validate_wasm_size(&module_path, wasm_config.max_wasm_size_bytes)
            .map_err(WasmError::CompilationFailed)?;

        let component = Component::from_file(&engine, &module_path).map_err(|e| {
            // File exists and size is OK — compilation error is genuine
            WasmError::CompilationFailed(format!(
                "Failed to compile WASM {module_label} {}: {}",
                module_path.display(),
                e
            ))
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
            capabilities,
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
            crate::capabilities::WasmCapabilities::denied(),
        )
        .await?;

        {
            let host_state = WasmRuntime::create_host_state(
                registry,
                HashMap::new(),
                ctx.state_store.clone(),
                ctx.config.max_memory_bytes,
                ctx.capabilities.clone(),
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
            // 2-layer peel (see error::peel_concurrent). Both wasmtime
            // layers are mapped to WasmError; the innermost String
            // (the WIT result error) is mapped to WasmError::GuestPanic.
            let init_result: Result<(), String> = crate::error::peel_concurrent(
                store
                    .as_context_mut()
                    .run_concurrent(async |accessor| plugin.call_init(accessor, config_pairs).await)
                    .await,
                |e| WasmError::GuestPanic(e.to_string()),
                |_e| WasmError::GuestPanic("plugin init() trapped".to_string()),
            )?;

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
        let caps = crate::capabilities::WasmCapabilities::from_scheme_list(
            &wasm_config.allow_call_schemes,
        );
        let ctx = Self::build(
            module_path,
            wasm_config,
            registry.clone(),
            |linker| {
                crate::host_functions::add_bean_to_linker(linker)
                    .map_err(|e| WasmError::CompilationFailed(e.to_string()))
            },
            "bean",
            caps,
        )
        .await?;

        let methods = {
            let host_state = WasmRuntime::create_host_state(
                registry,
                HashMap::new(),
                ctx.state_store.clone(),
                ctx.config.max_memory_bytes,
                ctx.capabilities.clone(),
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
            // 2-layer peel (see error::peel_concurrent). Both wasmtime
            // layers are mapped to WasmError; the innermost String
            // (the WIT result error) is mapped to WasmError::GuestPanic.
            let init_result: Result<(), String> = crate::error::peel_concurrent(
                store
                    .as_context_mut()
                    .run_concurrent(async |accessor| plugin.call_init(accessor, config_pairs).await)
                    .await,
                |e| WasmError::GuestPanic(e.to_string()),
                |_e| WasmError::GuestPanic("bean init() trapped".to_string()),
            )?;

            if let Err(e) = init_result {
                return Err(WasmError::GuestPanic(format!(
                    "bean init() failed for {}: {}",
                    ctx.module_path.display(),
                    e
                )));
            }

            // call_methods returns Vec<String> directly. The 2-layer shape
            // has wasmtime::Error as both layers (no inner WIT result),
            // so peel_concurrent collapses to a single Result.
            crate::error::peel_concurrent(
                store
                    .as_context_mut()
                    .run_concurrent(async |accessor| plugin.call_methods(accessor).await)
                    .await,
                |e| WasmError::GuestPanic(e.to_string()),
                |e| WasmError::GuestPanic(format!("call_methods trapped: {e}")),
            )?
        };

        Ok((ctx, methods))
    }

    pub fn create_store(&self, properties: HashMap<String, Value>) -> Store<WasmHostState> {
        let host_state = WasmRuntime::create_host_state(
            self.registry.clone(),
            properties,
            self.state_store.clone(),
            self.config.max_memory_bytes,
            self.capabilities.clone(),
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
