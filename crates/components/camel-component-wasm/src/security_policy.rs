use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;

use camel_api::security_policy::{AuthorizationDecision, SecurityPolicy, principal_from_exchange};
use camel_api::{CamelError, Exchange};
use camel_core::Registry;

use crate::error::WasmError;
use crate::security_policy_bindings::AuthorizationPolicy as AuthorizationPolicyGuest;
use crate::serde_bridge;
use crate::wasm_plugin_context::WasmPluginContext;

pub struct WasmSecurityPolicy {
    ctx: WasmPluginContext,
}

impl WasmSecurityPolicy {
    pub async fn new(
        module_path: impl AsRef<Path>,
        wasm_config: crate::config::WasmConfig,
        registry: Arc<std::sync::Mutex<Registry>>,
        init_config: HashMap<String, String>,
    ) -> Result<Self, WasmError> {
        let ctx = WasmPluginContext::new(module_path, wasm_config, registry, init_config).await?;
        Ok(Self { ctx })
    }
}

#[async_trait]
impl SecurityPolicy for WasmSecurityPolicy {
    async fn evaluate(&self, exchange: &mut Exchange) -> Result<AuthorizationDecision, CamelError> {
        let mut store = self.ctx.create_store(exchange.properties.clone());

        let plugin = AuthorizationPolicyGuest::instantiate_async(
            &mut store,
            &self.ctx.component,
            &self.ctx.linker,
        )
        .await
        .map_err(|e| WasmError::InstantiationFailed(e.to_string()))?;

        let wasm_exchange = serde_bridge::exchange_to_wasm(exchange)?;
        let sp_exchange: crate::security_policy_bindings::camel::plugin::types::WasmExchange =
            wasm_exchange.into();

        let result = plugin
            .call_evaluate(&mut store, &sp_exchange)
            .await
            .map_err(|e| self.ctx.classify_error(e))?;

        match result {
            Ok(None) => {
                let principal = principal_from_exchange(exchange).ok_or_else(|| {
                    CamelError::ProcessorError(
                        "authorization policy granted but no principal found in exchange".into(),
                    )
                })?;
                Ok(AuthorizationDecision::Granted { principal })
            }
            Ok(Some(reason)) => Ok(AuthorizationDecision::Denied {
                reason,
                required: vec![],
                actual: vec![],
            }),
            Err(wasm_err) => Err(CamelError::ProcessorError(format!(
                "wasm authorization policy evaluate failed: {wasm_err:?}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    fn test_tokio_handle() -> tokio::runtime::Handle {
        static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
        RT.get_or_init(|| tokio::runtime::Runtime::new().expect("test runtime"))
            .handle()
            .clone()
    }

    #[tokio::test]
    async fn test_wasm_security_policy_new_missing_file() {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let config = crate::config::WasmConfig::default();
        let result =
            WasmSecurityPolicy::new("/nonexistent/module.wasm", config, registry, HashMap::new())
                .await;
        let err = match result {
            Ok(_) => panic!("expected error for non-existent module"),
            Err(e) => e,
        };
        assert!(
            matches!(err, WasmError::ModuleNotFound(_)),
            "expected ModuleNotFound, got: {err:?}"
        );
    }

    #[test]
    fn test_wasm_security_policy_host_state_creation() {
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
}
