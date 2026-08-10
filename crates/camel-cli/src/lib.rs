pub mod commands;
pub mod template;

use camel_api::CamelError;
use camel_dsl::SecurityCompileContext;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Lint catalog registration — handle-free mirror of `commands::run`
// ---------------------------------------------------------------------------

/// Register a `ComponentBundle` from an empty TOML table, dropping any handle.
/// On empty-config build error, log at warn and skip — lint must degrade
/// gracefully (the skipped scheme surfaces as `unverified-scheme`), never fail
/// to construct its catalog.
macro_rules! register_bundle_empty {
    ($ctx:expr, $Bundle:ty) => {{
        let key = <$Bundle as camel_component_api::ComponentBundle>::config_key();
        match <$Bundle as camel_component_api::ComponentBundle>::from_toml(
            ::toml::Value::Table(::toml::map::Map::new()),
        ) {
            Ok(bundle) => {
                <$Bundle as camel_component_api::ComponentBundle>::register_all(
                    bundle,
                    &mut *$ctx,
                );
            }
            Err(err) => {
                tracing::warn!(
                    bundle = %key,
                    error = %err,
                    "lint catalog: empty-config build failed; skipping bundle (surfaces as unverified-scheme)"
                );
            }
        }
    }};
}

/// Like [`register_bundle_empty!`], but wires an empty datasource catalog
/// (sql/surrealdb bundles). An empty catalog is acceptable — metadata is
/// queryable regardless of configured datasources.
macro_rules! register_datasource_bundle_empty {
    ($ctx:expr, $Bundle:ty, $catalog:expr) => {{
        let key = <$Bundle as camel_component_api::ComponentBundle>::config_key();
        match <$Bundle as camel_component_api::ComponentBundle>::from_toml(
            ::toml::Value::Table(::toml::map::Map::new()),
        ) {
            Ok(bundle) => {
                let bundle = bundle.with_catalog(::std::sync::Arc::clone(&$catalog));
                <$Bundle as camel_component_api::ComponentBundle>::register_all(
                    bundle,
                    &mut *$ctx,
                );
            }
            Err(err) => {
                tracing::warn!(
                    bundle = %key,
                    error = %err,
                    "lint catalog: empty-config build failed; skipping datasource bundle (surfaces as unverified-scheme)"
                );
            }
        }
    }};
}

/// Register the built-in components into `ctx` for lint catalog population.
///
/// This mirrors `commands::run`'s registration list but is HANDLE-FREE: it
/// passes empty/default config to every bundle, registers bridge components
/// without their runtime handles (no `xsd_bridge_backend` / `bridge_runtime`,
/// no `BridgeCleanup`), and drops any pool returned by jms/cxf. It SKIPS the
/// path/route-coupled bundles: `wasm` (needs a config-relative `base_dir`) and
/// `exec` (route-conditional registration). Skipped schemes surface as
/// `unverified-scheme` notes in lint output, which is the accepted
/// graceful-degradation behaviour.
///
/// Because `Component::metadata()` has a trait default returning
/// `ComponentMetadata::minimal(scheme)` and `Registry::register()` harvests
/// it unconditionally, registering each builtin makes its scheme queryable by
/// the lint catalog (rich metadata where the component opts into it, a
/// minimal-but-present entry otherwise).
///
/// Drift tradeoff: this list and `run`'s list are kept separate on purpose —
/// `run`'s registration is lifecycle-entangled with bridge/pool/datasource/
/// path handles that lint has no use for. The drift is bounded and caught by
/// the corpus baseline (`tests/lint_corpus.rs`); unification is tracked by a
/// bd follow-up.
pub fn register_builtin_components_for_lint(ctx: &mut camel_core::CamelContext) {
    use camel_api::datasource::DatasourceCatalog;
    use camel_core::datasource::RuntimeDatasourceCatalog;

    // --- Config-independent components (handle-free) ---
    ctx.register_component(camel_component_timer::TimerComponent::new());
    ctx.register_component(camel_component_cron::CronComponent::new());
    ctx.register_component(camel_component_log::LogComponent::new());
    ctx.register_component(camel_component_direct::DirectComponent::new());
    ctx.register_component(camel_component_seda::SedaComponent::new());
    ctx.register_component(camel_component_mock::MockComponent::new());
    ctx.register_component(camel_component_controlbus::ControlBusComponent::new());

    // --- Bridge components WITHOUT their runtime handles ---
    // validator: xsd_bridge_backend() not captured, no backend stored.
    ctx.register_component(camel_component_validator::ValidatorComponent::new());
    // xslt / xj: bridge_runtime() not captured, no BridgeCleanup installed.
    // Lint is short-lived; no shutdown cleanup needed.
    ctx.register_component(camel_xslt::XsltComponent::default());
    ctx.register_component(camel_xj::XjComponent::default());

    // --- Empty datasource catalog for sql/surrealdb bundles ---
    let datasource_catalog: Arc<dyn DatasourceCatalog> = Arc::new(RuntimeDatasourceCatalog::new(
        std::collections::HashMap::new(),
    ));

    // --- Always-on bundles with empty config ---
    register_bundle_empty!(ctx, camel_component_http::HttpBundle);
    #[cfg(feature = "http-static")]
    register_bundle_empty!(ctx, camel_component_http::HttpStaticBundle);
    register_bundle_empty!(ctx, camel_component_ws::WsBundle);
    register_bundle_empty!(ctx, camel_component_file::FileBundle);
    register_bundle_empty!(ctx, camel_component_container::ContainerBundle);
    register_bundle_empty!(ctx, camel_template::TemplateBundle);
    register_bundle_empty!(ctx, camel_master::MasterBundle);
    register_bundle_empty!(ctx, camel_component_opensearch::OpenSearchBundle);
    register_bundle_empty!(ctx, camel_component_redis::RedisBundle);

    // jms / cxf: registered without capturing their pool handle (lint has no
    // runtime use for a bridge pool; metadata is harvested at register time).
    register_bundle_empty!(ctx, camel_component_jms::JmsBundle);
    register_bundle_empty!(ctx, camel_component_cxf::CxfBundle);

    // --- Datasource bundles (empty datasource catalog) ---
    register_datasource_bundle_empty!(ctx, camel_component_sql::SqlBundle, datasource_catalog);
    #[cfg(feature = "surrealdb")]
    register_datasource_bundle_empty!(
        ctx,
        camel_component_surrealdb::SurrealDbBundle,
        datasource_catalog
    );

    // --- Feature-gated bundles (empty config) ---
    #[cfg(feature = "kafka")]
    register_bundle_empty!(ctx, camel_component_kafka::KafkaBundle);
    #[cfg(feature = "mqtt")]
    register_bundle_empty!(ctx, camel_component_mqtt::MqttBundle);
    #[cfg(feature = "grpc")]
    register_bundle_empty!(ctx, camel_component_grpc::GrpcBundle);
    #[cfg(feature = "llm")]
    register_bundle_empty!(ctx, camel_component_llm::LlmBundle);

    // --- Skipped (path/route-coupled) ---
    // wasm: needs a config-relative `base_dir`; lint has no canonical config.
    // exec: route-conditional; lint has no discovered routes to gate on.
    // Both surface as `unverified-scheme` in lint output by design.
}

// ---------------------------------------------------------------------------
// Auth helpers — shared between wasm and non-wasm build paths
// ---------------------------------------------------------------------------

fn native_authenticator(
    native: &camel_config::config::NativeAuthConfig,
) -> Result<Arc<dyn camel_auth::TokenAuthenticator>, CamelError> {
    let token = native.bearer_token.clone().ok_or_else(|| {
        CamelError::Config("security.native.bearer_token is required for route auth".into())
    })?;
    let principal = camel_api::security_policy::Principal {
        subject: native.subject.clone(),
        issuer: native.issuer.clone().unwrap_or_else(|| "native".into()),
        audience: Vec::new(),
        roles: native.roles.clone(),
        scopes: native.scopes.clone(),
        claims: serde_json::Value::Object(serde_json::Map::new()),
    };
    let store = camel_auth::native_auth::NativeCredentialStore::try_new(vec![
        camel_auth::NativeCredential {
            secret: camel_auth::NativeCredentialSecret::Plaintext {
                value: token.into(),
            },
            principal,
        },
    ])?;
    Ok(Arc::new(camel_auth::StaticTokenAuthenticator::new(store)))
}

async fn keycloak_authenticator(
    keycloak: &camel_config::config::KeycloakSecurityConfig,
) -> Result<Arc<dyn camel_auth::TokenAuthenticator>, CamelError> {
    let realm = camel_component_keycloak::KeycloakRealmConfig::new(
        keycloak.server_url.clone(),
        keycloak.realm.clone(),
        keycloak.client_id.clone(),
    )
    .with_client_secret(keycloak.client_secret.clone())
    .with_allow_internal(keycloak.allow_internal);

    match keycloak.validation.method.as_str() {
        "local" => {
            let jwks = Arc::new(
                camel_auth::RemoteJwksProvider::new(realm.jwks_uri(), realm.policy())
                    .await
                    .map_err(|e| CamelError::Config(e.to_string()))?,
            );
            let mapper = Arc::new(camel_auth::JsonPointerClaimsMapper::new(
                camel_component_keycloak::keycloak_claim_paths(&keycloak.client_id),
            ));
            Ok(Arc::new(camel_auth::LocalJwtValidator::new(
                keycloak.validation.audience.clone(),
                realm.realm_url(),
                jwks,
                mapper,
            )))
        }
        "introspection" => {
            let opts = camel_auth::IntrospectionCacheOptions {
                max_entries: keycloak.introspection.max_entries,
                default_ttl: std::time::Duration::from_secs(
                    keycloak.introspection.default_ttl_secs,
                ),
                negative_ttl: std::time::Duration::from_secs(
                    keycloak.introspection.negative_ttl_secs,
                ),
            };
            let auth = realm.introspection_authenticator(opts).await?;
            Ok(Arc::new(auth))
        }
        other => Err(CamelError::Config(format!(
            "unsupported security.keycloak.validation.method: {other}"
        ))),
    }
}

/// Resolve the authenticator from `[security.*]`.
///
/// Chooses at most one of `keycloak`, `oidc`, `native`.  Returns `None` if
/// none is configured (anonymous routes are allowed).  Errors if more than
/// one is present.
async fn resolve_authenticator(
    security: &camel_config::config::SecurityConfig,
) -> Result<Option<Arc<dyn camel_auth::TokenAuthenticator>>, CamelError> {
    let has_keycloak = security.keycloak.is_some();
    let has_oidc = security.oidc.is_some();
    let has_native = security.native.is_some();

    let count = [has_keycloak, has_oidc, has_native]
        .iter()
        .filter(|&&x| x)
        .count();
    if count > 1 {
        return Err(CamelError::Config(
            "configure only one of security.keycloak, security.oidc, security.native for route authentication"
                .into(),
        ));
    }

    if let Some(ref keycloak) = security.keycloak {
        Ok(Some(keycloak_authenticator(keycloak).await?))
    } else if let Some(ref native) = security.native {
        Ok(Some(native_authenticator(native)?))
    } else {
        // oidc alone: leave authenticator None for now (scope creep avoidance)
        Ok(None)
    }
}

/// Register Keycloak UMA permission evaluator from `[security.keycloak.uma]`
/// config.  No-ops when no UMA config is present.
async fn register_keycloak_uma_evaluator(
    camel_config: &camel_config::config::CamelConfig,
    evaluator_registry: &camel_auth::PermissionEvaluatorRegistry,
) -> Result<(), CamelError> {
    if let Some(ref keycloak) = camel_config.security.keycloak
        && let Some(ref uma) = keycloak.uma
    {
        let realm = camel_component_keycloak::KeycloakRealmConfig::new(
            keycloak.server_url.clone(),
            keycloak.realm.clone(),
            keycloak.client_id.clone(),
        )
        .with_client_secret(keycloak.client_secret.clone())
        .with_allow_internal(keycloak.allow_internal);
        let evaluator = realm
            .uma_evaluator()
            .await
            .map_err(|e| CamelError::Config(e.to_string()))?;
        evaluator_registry.register(uma.provider.clone(), evaluator);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Public entry-point (cfg-gated) — matches the existing signature
// ---------------------------------------------------------------------------

#[cfg(feature = "wasm")]
pub async fn build_security_compile_context_from_config(
    camel_config: &camel_config::config::CamelConfig,
    registry: Arc<std::sync::Mutex<camel_core::Registry>>,
) -> Result<SecurityCompileContext, CamelError> {
    let authenticator = resolve_authenticator(&camel_config.security).await?;
    let mut security_ctx = SecurityCompileContext::new(authenticator, None);

    let evaluator_registry = camel_auth::PermissionEvaluatorRegistry::new();

    if let Some(ref policies) = camel_config.security.policies {
        let policy_registry =
            camel_component_wasm::build_security_policy_registry(&policies.wasm, registry.clone())
                .await
                .map_err(|e| CamelError::Config(e.to_string()))?;
        if !policy_registry.is_empty() {
            security_ctx = security_ctx.with_security_policy_registry(Arc::new(policy_registry));
        }
    }

    if let Some(ref permissions) = camel_config.security.permissions {
        let wasm_registry = camel_component_wasm::build_permission_registry(permissions, registry)
            .await
            .map_err(|e| CamelError::Config(e.to_string()))?;
        for (name, evaluator) in wasm_registry.entries() {
            evaluator_registry.register(name, evaluator);
        }
    }

    register_keycloak_uma_evaluator(camel_config, &evaluator_registry).await?;

    if !evaluator_registry.is_empty() {
        security_ctx = security_ctx.with_evaluator_registry(Arc::new(evaluator_registry));
    }

    Ok(security_ctx)
}

#[cfg(not(feature = "wasm"))]
pub async fn build_security_compile_context_from_config(
    camel_config: &camel_config::config::CamelConfig,
    _registry: Arc<std::sync::Mutex<camel_core::Registry>>,
) -> Result<SecurityCompileContext, CamelError> {
    if camel_config.security.permissions.is_some() {
        return Err(CamelError::Config(
            "security.permissions requires camel-cli wasm feature".into(),
        ));
    }

    if camel_config.security.policies.is_some() {
        return Err(CamelError::Config(
            "security.policies requires camel-cli wasm feature".into(),
        ));
    }

    let authenticator = resolve_authenticator(&camel_config.security).await?;
    let mut security_ctx = SecurityCompileContext::new(authenticator, None);

    let evaluator_registry = camel_auth::PermissionEvaluatorRegistry::new();

    register_keycloak_uma_evaluator(camel_config, &evaluator_registry).await?;

    if !evaluator_registry.is_empty() {
        security_ctx = security_ctx.with_evaluator_registry(Arc::new(evaluator_registry));
    }

    Ok(security_ctx)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    #[tokio::test]
    async fn native_static_token_builds_authenticator() {
        let cfg: camel_config::config::CamelConfig = toml::from_str(
            r#"
        [security.native]
        subject = "dev-user"
        issuer = "native"
        bearer_token = "dev-token"
        roles = ["admin"]
        scopes = ["read"]
        "#,
        )
        .expect("config parses");

        let registry = Arc::new(std::sync::Mutex::new(camel_core::Registry::new()));
        let ctx = crate::build_security_compile_context_from_config(&cfg, registry)
            .await
            .expect("security context builds");

        assert!(ctx.authenticator.is_some());
    }

    #[cfg(feature = "wasm")]
    #[tokio::test]
    async fn security_permissions_config_is_consumed_when_building_compile_context() {
        let cfg: camel_config::config::CamelConfig = toml::from_str(
            r#"
            [security.permissions.invoice-policy]
            provider = "wasm"
            "#,
        )
        .expect("config parses");

        let registry = Arc::new(std::sync::Mutex::new(camel_core::Registry::new()));
        let err = match crate::build_security_compile_context_from_config(&cfg, registry).await {
            Ok(_) => {
                panic!("wasm permission provider without path must fail during registry build")
            }
            Err(err) => err,
        };

        assert!(
            err.to_string().contains("requires 'path'"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn multiple_auth_providers_returns_config_error() {
        let cfg: camel_config::config::CamelConfig = toml::from_str(
            r#"
        [security.keycloak]
        server_url = "https://kc.example.com"
        realm = "camel"
        client_id = "camel-api"
        client_secret = "secret"

        [security.native]
        subject = "dev-user"
        issuer = "native"
        bearer_token = "dev-token"
        "#,
        )
        .expect("config parses");

        let registry = Arc::new(std::sync::Mutex::new(camel_core::Registry::new()));
        let err = match crate::build_security_compile_context_from_config(&cfg, registry).await {
            Ok(_) => panic!("multiple providers should fail"),
            Err(err) => err,
        };

        assert!(
            err.to_string().contains("configure only one"),
            "unexpected error: {err}"
        );
    }
}
