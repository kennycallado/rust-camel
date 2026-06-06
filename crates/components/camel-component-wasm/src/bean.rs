use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;

use camel_api::{CamelError, Exchange};
use camel_bean::BeanProcessor;
use camel_core::Registry;

use crate::bean_bindings::Bean as BeanGuest;
use crate::error::WasmError;
use crate::serde_bridge;
use crate::wasm_plugin_context::WasmPluginContext;

pub struct WasmBean {
    ctx: WasmPluginContext,
    methods: Vec<String>,
}

impl WasmBean {
    pub async fn new(
        module_path: impl AsRef<Path>,
        wasm_config: crate::config::WasmConfig,
        registry: Arc<std::sync::Mutex<Registry>>,
        bean_config: HashMap<String, String>,
    ) -> Result<Self, WasmError> {
        let (ctx, methods) =
            WasmPluginContext::new_bean(module_path, wasm_config, registry, bean_config).await?;
        Ok(Self { ctx, methods })
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

        let mut store = self.ctx.create_store(exchange.properties.clone());

        let plugin =
            BeanGuest::instantiate_async(&mut store, &self.ctx.component, &self.ctx.linker)
                .await
                .map_err(|e| WasmError::InstantiationFailed(e.to_string()))?;

        let wasm_exchange = serde_bridge::exchange_to_wasm(exchange)?;
        let bean_exchange = wasm_exchange.into();

        let result = plugin
            .call_invoke(&mut store, method, &bean_exchange)
            .await
            .map_err(|e| self.ctx.classify_error(e))?;

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
    use std::sync::OnceLock;

    // Note: WasmBean::new requires a real WASM file. The WasmConfig propagation
    // chain (limits → from_limits → WasmConfig → create_host_state) is tested at
    // the runtime layer (memory_growth_*, timeout_kills_infinite_loop_guest).
    // All plugin types share that runtime layer, so coverage is implicit.

    fn test_tokio_handle() -> tokio::runtime::Handle {
        static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
        RT.get_or_init(|| tokio::runtime::Runtime::new().expect("test runtime"))
            .handle()
            .clone()
    }

    #[test]
    fn test_wasm_bean_host_state_creation() {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let host_state = crate::runtime::WasmRuntime::create_host_state(
            registry,
            HashMap::new(),
            crate::state_store::StateStore::new(),
            test_tokio_handle(),
            0,
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
