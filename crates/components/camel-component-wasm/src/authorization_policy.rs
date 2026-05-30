use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use camel_api::security_policy::store_principal_properties;
use camel_api::{Exchange, Message};
use camel_auth::PermissionEvaluatorRegistry;
use camel_auth::permission::{PermissionDecision, PermissionEvaluator, PermissionRequest};
use camel_auth::types::AuthError;
use camel_config::config::PermissionProviderConfig;
use camel_core::Registry;

use crate::config::WasmConfig;
use crate::error::WasmError;
use crate::security_policy_bindings::AuthorizationPolicy as AuthorizationPolicyGuest;
use crate::serde_bridge;
use crate::wasm_plugin_context::WasmPluginContext;

mod props {
    pub const RESOURCE: &str = "camel.permission.resource";
    pub const ACTION: &str = "camel.permission.action";
    pub const REQUESTED_SCOPES: &str = "camel.permission.requested_scopes";
    pub const CONTEXT: &str = "camel.permission.context";
}

pub struct WasmAuthorizationPolicyEvaluator {
    ctx: WasmPluginContext,
}

impl WasmAuthorizationPolicyEvaluator {
    pub async fn new(
        module_path: impl AsRef<std::path::Path>,
        wasm_config: WasmConfig,
        registry: Arc<std::sync::Mutex<Registry>>,
        init_config: HashMap<String, String>,
    ) -> Result<Self, WasmError> {
        let ctx = WasmPluginContext::new(module_path, wasm_config, registry, init_config).await?;
        Ok(Self { ctx })
    }

    fn build_synthetic_exchange(request: &PermissionRequest) -> Exchange {
        let mut exchange = Exchange::new(Message::default());

        store_principal_properties(&mut exchange, &request.principal);

        exchange.set_property(props::RESOURCE, request.resource.clone());
        exchange.set_property(props::ACTION, request.action.clone());
        exchange.set_property(
            props::REQUESTED_SCOPES,
            serde_json::to_string(&request.requested_scopes).unwrap_or_else(|_| "[]".to_string()),
        );
        exchange.set_property(
            props::CONTEXT,
            serde_json::to_string(&request.context).unwrap_or_else(|_| "null".to_string()),
        );

        let keys_to_remove: Vec<String> = exchange
            .properties
            .keys()
            .filter(|k| !k.starts_with("camel.auth.") && !k.starts_with("camel.permission."))
            .cloned()
            .collect();
        for key in keys_to_remove {
            exchange.properties.remove(&key);
        }

        exchange
    }
}

pub async fn build_permission_registry(
    permissions: &HashMap<String, PermissionProviderConfig>,
    registry: Arc<std::sync::Mutex<Registry>>,
) -> Result<PermissionEvaluatorRegistry, AuthError> {
    let evaluator_registry = PermissionEvaluatorRegistry::new();

    for (name, provider_config) in permissions {
        match provider_config.provider.as_str() {
            "wasm" => {
                let path = provider_config.path.as_ref().ok_or_else(|| {
                    AuthError::ConfigError(format!(
                        "permission provider '{}' requires 'path' for wasm provider",
                        name
                    ))
                })?;

                let evaluator = WasmAuthorizationPolicyEvaluator::new(
                    path,
                    WasmConfig::default(),
                    registry.clone(),
                    provider_config.config.clone().unwrap_or_default(),
                )
                .await
                .map_err(|e| {
                    AuthError::ConfigError(format!(
                        "failed to create WASM permission evaluator '{}': {}",
                        name, e
                    ))
                })?;

                evaluator_registry.register(name.clone(), Arc::new(evaluator));
            }
            other => {
                return Err(AuthError::ConfigError(format!(
                    "unknown permission provider '{}'",
                    other
                )));
            }
        }
    }

    Ok(evaluator_registry)
}

impl std::fmt::Debug for WasmAuthorizationPolicyEvaluator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmAuthorizationPolicyEvaluator")
            .field("module_path", &self.ctx.module_path)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl PermissionEvaluator for WasmAuthorizationPolicyEvaluator {
    async fn evaluate(&self, request: PermissionRequest) -> Result<PermissionDecision, AuthError> {
        let synthetic = Self::build_synthetic_exchange(&request);

        let mut store = self.ctx.create_store(synthetic.properties.clone());

        let plugin = AuthorizationPolicyGuest::instantiate_async(
            &mut store,
            &self.ctx.component,
            &self.ctx.linker,
        )
        .await
        .map_err(|e| {
            AuthError::ProviderUnavailable(format!(
                "wasm authorization policy instantiation failed: {e}"
            ))
        })?;

        let wasm_exchange = serde_bridge::exchange_to_wasm(&synthetic).map_err(|e| {
            AuthError::ProviderUnavailable(format!("wasm exchange serialization failed: {e}"))
        })?;
        let ap_exchange: crate::security_policy_bindings::camel::plugin::types::WasmExchange =
            wasm_exchange.into();

        let result = plugin
            .call_evaluate(&mut store, &ap_exchange)
            .await
            .map_err(|e| {
                let wasm_err = self.ctx.classify_error(e);
                AuthError::ProviderUnavailable(format!(
                    "wasm authorization policy evaluate failed: {wasm_err:?}"
                ))
            })?;

        match result {
            Ok(None) => Ok(PermissionDecision::Granted),
            Ok(Some(reason)) => Ok(PermissionDecision::Denied { reason }),
            Err(wasm_err) => Err(AuthError::ProviderUnavailable(format!(
                "wasm authorization policy returned error: {wasm_err:?}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::security_policy::Principal;
    use camel_config::config::PermissionCacheConfig;
    use serde_json::json;

    fn test_principal() -> Principal {
        Principal {
            subject: "billing-worker".into(),
            issuer: "https://kc.example.com".into(),
            audience: vec!["camel-api".into()],
            scopes: vec!["read".into()],
            roles: vec!["billing".into()],
            claims: json!({}),
        }
    }

    #[test]
    fn synthetic_exchange_contains_only_allowed_properties() {
        let request = PermissionRequest {
            principal: test_principal(),
            resource: "invoice:123".into(),
            action: "read".into(),
            requested_scopes: vec!["read".into()],
            context: json!({"tenant": "acme"}),
        };
        let exchange = WasmAuthorizationPolicyEvaluator::build_synthetic_exchange(&request);

        assert!(exchange.input.body.is_empty());
        assert!(
            exchange
                .properties
                .keys()
                .all(|k| k.starts_with("camel.auth.") || k.starts_with("camel.permission."))
        );
        assert_eq!(
            exchange.property(props::RESOURCE).and_then(|v| v.as_str()),
            Some("invoice:123")
        );
        assert_eq!(
            exchange.property(props::ACTION).and_then(|v| v.as_str()),
            Some("read")
        );
    }

    #[test]
    fn synthetic_exchange_no_secret_leakage() {
        let mut principal = test_principal();
        principal.claims = json!({
            "sub": "alice",
            "access_token": "super-secret-token",
            "client_secret": "should-not-appear"
        });
        let request = PermissionRequest {
            principal,
            resource: "res".into(),
            action: "read".into(),
            requested_scopes: vec![],
            context: json!({}),
        };
        let exchange = WasmAuthorizationPolicyEvaluator::build_synthetic_exchange(&request);

        assert!(
            exchange
                .properties
                .keys()
                .all(|k| k.starts_with("camel.auth.") || k.starts_with("camel.permission."))
        );

        let secret_present = exchange
            .properties
            .keys()
            .any(|k| k.contains("token") || k.contains("secret") || k.contains("rpt"));
        assert!(
            !secret_present,
            "no secret/token property keys should appear"
        );
    }

    #[tokio::test]
    async fn wasm_evaluator_new_missing_file_returns_error() {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let result = WasmAuthorizationPolicyEvaluator::new(
            "/nonexistent/policy.wasm",
            WasmConfig::default(),
            registry,
            HashMap::new(),
        )
        .await;
        assert!(result.is_err(), "expected error for nonexistent module");
    }

    #[test]
    fn debug_hides_internal_state() {
        let request = PermissionRequest {
            principal: test_principal(),
            resource: "invoice:123".into(),
            action: "read".into(),
            requested_scopes: vec!["read".into()],
            context: json!({"tenant": "acme"}),
        };
        let exchange = WasmAuthorizationPolicyEvaluator::build_synthetic_exchange(&request);
        let out = format!("{exchange:?}");
        assert!(out.contains("properties"));
    }

    #[test]
    fn build_permission_registry_empty() {
        let perms = HashMap::new();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let registry = rt
            .block_on(async {
                build_permission_registry(&perms, Arc::new(std::sync::Mutex::new(Registry::new())))
                    .await
            })
            .expect("empty permissions should succeed");
        assert!(registry.get("missing").is_none());
    }

    #[tokio::test]
    async fn build_permission_registry_wasm_missing_path_returns_error() {
        let mut perms = HashMap::new();
        perms.insert(
            "test".into(),
            PermissionProviderConfig {
                provider: "wasm".into(),
                path: None,
                config: None,
                cache: PermissionCacheConfig::default(),
            },
        );
        let result =
            build_permission_registry(&perms, Arc::new(std::sync::Mutex::new(Registry::new())))
                .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn build_permission_registry_unknown_provider_returns_error() {
        let mut perms = HashMap::new();
        perms.insert(
            "test".into(),
            PermissionProviderConfig {
                provider: "unknown".into(),
                path: None,
                config: None,
                cache: PermissionCacheConfig::default(),
            },
        );
        let result =
            build_permission_registry(&perms, Arc::new(std::sync::Mutex::new(Registry::new())))
                .await;
        assert!(result.is_err());
    }
}
