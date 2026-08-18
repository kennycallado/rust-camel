//! Security compile-context construction.
//!
//! Authenticator builders for the `[security.*]` config blocks
//! (`native`, `keycloak`, `oidc`), provider resolution/registration, and
//! the cfg-gated [`build_security_compile_context_from_config`] entry point
//! shared between the wasm and non-wasm build paths.

use camel_api::CamelError;
use camel_auth::{JwksProvider, escape_json_pointer};
use camel_dsl::SecurityCompileContext;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Auth helpers — shared between wasm and non-wasm build paths
// ---------------------------------------------------------------------------

/// Synthesize a native principal from a subject/roles/scopes triple.
///
/// Emits the loud-synthesis warning (task 1.4) once per principal: native
/// principals carry no claims/audience, so downstream policy decisions only
/// see the configured roles/scopes.
fn native_principal(
    subject: &str,
    issuer: &str,
    roles: Vec<String>,
    scopes: Vec<String>,
) -> camel_api::security_policy::Principal {
    tracing::warn!(
        "native principal '{}' synthesized with empty claims/audience",
        subject
    );
    camel_api::security_policy::Principal {
        subject: subject.to_string(),
        issuer: issuer.to_string(),
        audience: Vec::new(),
        roles,
        scopes,
        claims: serde_json::Value::Object(serde_json::Map::new()),
    }
}

fn native_authenticator(
    native: &camel_config::config::NativeAuthConfig,
) -> Result<Arc<dyn camel_auth::TokenAuthenticator>, CamelError> {
    // Mirrored by `native_store_from_config` in
    // crates/camel-test/tests/auth_multi_credential_test.rs — update in lockstep.

    let issuer = native.issuer.clone().unwrap_or_else(|| "native".into());
    let mut credentials: Vec<camel_auth::NativeCredential> = Vec::new();

    // (b) scalar bearer_token — single entry keyed by its value.
    if let Some(token) = &native.bearer_token {
        credentials.push(camel_auth::NativeCredential {
            secret: camel_auth::NativeCredentialSecret::Plaintext {
                value: token.clone().into(),
            },
            principal: native_principal(
                &native.subject,
                &issuer,
                native.roles.clone(),
                native.scopes.clone(),
            ),
        });
    }

    // (c) scalar api_key — same store entry, keyed by its secret value.
    if let Some(api_key) = &native.api_key {
        credentials.push(camel_auth::NativeCredential {
            secret: camel_auth::NativeCredentialSecret::Plaintext {
                value: api_key.clone().into(),
            },
            principal: native_principal(
                &native.subject,
                &issuer,
                native.roles.clone(),
                native.scopes.clone(),
            ),
        });
    }

    // (a) per-entry credentials — each with its own principal/roles/scopes.
    for entry in &native.credentials {
        let secret = match (&entry.secret_env, &entry.secret) {
            (Some(name), None) => camel_auth::NativeCredentialSecret::Env { name: name.clone() },
            (None, Some(value)) => camel_auth::NativeCredentialSecret::Plaintext {
                value: value.clone().into(),
            },
            // `NativeAuthConfig::validate_credentials` (config load) guarantees
            // exactly one of `secret_env` / `secret`, but
            // `build_security_compile_context_from_config` is pub and accepts
            // caller-built configs, so fail closed instead of panicking.
            _ => {
                return Err(CamelError::Config(
                    "security.native.credentials must set exactly one of secret_env or secret"
                        .to_string(),
                ));
            }
        };
        credentials.push(camel_auth::NativeCredential {
            secret,
            principal: native_principal(
                &entry.subject,
                &issuer,
                entry.roles.clone(),
                entry.scopes.clone(),
            ),
        });
    }

    if credentials.is_empty() {
        return Err(CamelError::Config(
            "security.native configured without any credential: set bearer_token, api_key, or [[security.native.credentials]]"
                .into(),
        ));
    }

    let store = camel_auth::native_auth::NativeCredentialStore::try_new(credentials)?;
    Ok(Arc::new(camel_auth::StaticTokenAuthenticator::new(store)))
}

async fn keycloak_authenticator(
    keycloak: &camel_config::config::KeycloakSecurityConfig,
) -> Result<Arc<dyn camel_auth::TokenAuthenticator>, CamelError> {
    camel_auth::native_auth::ensure_no_placeholder_markers(&keycloak.client_secret)
        // allow-secret: names the config field, not a secret value
        .map_err(|e| CamelError::Config(format!("keycloak.client_secret: {e}")))?;
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

/// OIDC claim-path preset, mirroring
/// `camel_component_keycloak::keycloak_claim_paths`.
fn oidc_claim_paths(oidc: &camel_config::config::OidcSecurityConfig) -> camel_auth::ClaimPaths {
    let mut roles = vec!["/realm_access/roles".to_string()];
    if let Some(ref client_id) = oidc.client_id {
        roles.push(format!(
            "/resource_access/{}/roles",
            escape_json_pointer(client_id)
        ));
    }
    camel_auth::ClaimPaths {
        subject: "/sub".into(),
        roles,
        scopes: Some("/scope".into()),
    }
}

/// Assemble the OIDC JWT validator from config plus a JWKS provider.
///
/// Split from [`oidc_authenticator`] so tests can inject an in-memory
/// `JwksProvider` and exercise validation + claim mapping without network.
fn oidc_validator(
    oidc: &camel_config::config::OidcSecurityConfig,
    jwks: Arc<dyn camel_auth::JwksProvider>,
) -> camel_auth::LocalJwtValidator {
    let mapper = Arc::new(camel_auth::JsonPointerClaimsMapper::new(oidc_claim_paths(
        oidc,
    )));
    camel_auth::LocalJwtValidator::new(oidc.audience.clone(), oidc.issuer.clone(), jwks, mapper)
}

async fn oidc_authenticator(
    oidc: &camel_config::config::OidcSecurityConfig,
    ssrf: &camel_api::SsrfPolicy,
) -> Result<Arc<dyn camel_auth::TokenAuthenticator>, CamelError> {
    // Guard before any network: reject unresolved placeholder markers in the
    // client secret (mirrors keycloak).
    if let Some(secret) = &oidc.client_secret {
        camel_auth::native_auth::ensure_no_placeholder_markers(secret)
            // allow-secret: names the config field, not a secret value
            .map_err(|e| CamelError::Config(format!("oidc.client_secret: {e}")))?;
    }
    let jwks_uri = oidc
        .jwks_uri
        .as_ref()
        .ok_or_else(|| CamelError::Config("security.oidc.jwks_uri is required".to_string()))?;

    let provider = camel_auth::RemoteJwksProvider::new(jwks_uri.clone(), *ssrf)
        .await
        .map_err(|e| CamelError::Config(format!("invalid OIDC jwks_uri {jwks_uri}: {e}")))?;

    // Prefetch keys so a misconfigured/unreachable JWKS fails at startup,
    // not on the first authenticated request.
    provider.get_signing_keys().await.map_err(|e| {
        CamelError::Config(format!("OIDC JWKS prefetch failed for {jwks_uri}: {e}"))
    })?;

    let jwks: Arc<dyn camel_auth::JwksProvider> = Arc::new(provider);
    Ok(Arc::new(oidc_validator(oidc, jwks)))
}

/// Resolve every configured authenticator provider from `[security.*]`.
///
/// Builds a `("keycloak", _)`, `("oidc", _)`, and `("native", _)` entry for
/// each configured block — all of them, with no XOR restriction. Returns an
/// empty vec when none is configured (anonymous routes allowed). A failure
/// from any one provider aborts the whole resolution, so a broken config
/// never yields a partially-registered set.
async fn resolve_authenticators(
    security: &camel_config::config::SecurityConfig,
) -> Result<Vec<(String, Arc<dyn camel_auth::TokenAuthenticator>)>, CamelError> {
    let mut providers: Vec<(String, Arc<dyn camel_auth::TokenAuthenticator>)> = Vec::new();

    if let Some(ref keycloak) = security.keycloak {
        providers.push((
            "keycloak".to_string(),
            keycloak_authenticator(keycloak).await?,
        ));
    }
    if let Some(ref oidc) = security.oidc {
        providers.push((
            "oidc".to_string(),
            oidc_authenticator(oidc, &camel_api::SsrfPolicy::PublicHttpsOnly).await?,
        ));
    }
    if let Some(ref native) = security.native {
        providers.push(("native".to_string(), native_authenticator(native)?));
    }

    Ok(providers)
}

/// Register resolved providers onto a [`SecurityCompileContext`].
///
/// Every provider (first included) is registered by name onto a default
/// context. A single named provider therefore resolves unnamed routes to it;
/// the reserved `"default"` provider is never synthesized here. An empty
/// provider list returns a default context (the anonymous-routes case).
fn register_providers(
    providers: Vec<(String, Arc<dyn camel_auth::TokenAuthenticator>)>,
) -> SecurityCompileContext {
    let mut registered = SecurityCompileContext::new(None, None);

    for (name, auth) in providers {
        registered = registered.with_named_authenticator(&name, auth);
    }

    registered
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
pub(crate) async fn build_security_compile_context_from_config(
    camel_config: &camel_config::config::CamelConfig,
    registry: Arc<std::sync::Mutex<camel_core::Registry>>,
) -> Result<SecurityCompileContext, CamelError> {
    let wasm_ctx: Arc<dyn camel_component_api::ComponentContext> =
        Arc::new(camel_core::RegistryComponentContext::new(registry));
    let providers = resolve_authenticators(&camel_config.security).await?;
    let mut security_ctx = register_providers(providers);

    let evaluator_registry = camel_auth::PermissionEvaluatorRegistry::new();

    if let Some(ref policies) = camel_config.security.policies {
        let policy_registry =
            camel_component_wasm::build_security_policy_registry(&policies.wasm, wasm_ctx.clone())
                .await
                .map_err(|e| CamelError::Config(e.to_string()))?;
        if !policy_registry.is_empty() {
            security_ctx = security_ctx.with_security_policy_registry(Arc::new(policy_registry));
        }
    }

    if let Some(ref permissions) = camel_config.security.permissions {
        let wasm_registry = camel_component_wasm::build_permission_registry(permissions, wasm_ctx)
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
pub(crate) async fn build_security_compile_context_from_config(
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

    let providers = resolve_authenticators(&camel_config.security).await?;
    let mut security_ctx = register_providers(providers);

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
        let ctx = super::build_security_compile_context_from_config(&cfg, registry)
            .await
            .expect("security context builds");

        assert!(ctx.authenticator_for(Some("native")).unwrap().is_some());
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
        let err = match super::build_security_compile_context_from_config(&cfg, registry).await {
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
    async fn keycloak_guard_fires_before_network() {
        let cfg: camel_config::config::CamelConfig = toml::from_str(
            r#"
        [security.keycloak]
        server_url = "https://kc.example.com"
        realm = "camel"
        client_id = "camel-api"
        client_secret = "{{env:KC}}"
        "#,
        )
        .expect("config parses");

        let keycloak = cfg.security.keycloak.expect("keycloak configured");
        let err = match super::keycloak_authenticator(&keycloak).await {
            Ok(_) => panic!("marker client_secret must be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("marker"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn native_only_resolves_single_provider() {
        let cfg: camel_config::config::CamelConfig = toml::from_str(
            r#"
        [security.native]
        subject = "dev-user"
        issuer = "native"
        bearer_token = "dev-token"
        "#,
        )
        .expect("config parses");

        let providers = super::resolve_authenticators(&cfg.security)
            .await
            .expect("native provider resolves");

        assert_eq!(providers.len(), 1, "expected a single provider");
        assert_eq!(providers[0].0, "native", "expected the native name");
    }

    // -----------------------------------------------------------------------
    // OIDC wiring tests — fail-closed semantics without network (in-memory
    // JWKS + fixture-signed JWTs, mirroring camel-auth jwt.rs tests)
    // -----------------------------------------------------------------------

    static TEST_RSA_PRIVATE_PEM: &[u8] = include_bytes!("../tests/fixtures/test_rsa_private.pem");
    static TEST_RSA_PUBLIC_PEM: &[u8] = include_bytes!("../tests/fixtures/test_rsa_public.pem");

    /// In-memory JWKS provider returning a single `Jwk` built from the
    /// fixture RSA public PEM — no network, no TLS.
    struct InMemoryJwks {
        kid: String,
        public_pem: &'static [u8],
    }

    #[async_trait::async_trait]
    impl camel_auth::JwksProvider for InMemoryJwks {
        async fn get_signing_keys(&self) -> Result<Vec<camel_auth::Jwk>, camel_auth::AuthError> {
            Ok(vec![camel_auth::Jwk {
                kid: self.kid.clone(),
                kty: "RSA".into(),
                alg: Some("RS256".into()),
                r#use: None,
                n: String::from_utf8_lossy(self.public_pem).into_owned(),
                e: "AQAB".into(),
            }])
        }

        async fn refresh(&self) -> Result<(), camel_auth::AuthError> {
            Ok(())
        }
    }

    /// Build an [`camel_config::config::OidcSecurityConfig`] with every field
    /// explicit (7 fields, no `Default`).
    fn oidc_config(
        jwks_uri: Option<&str>,
        client_secret: Option<&str>,
    ) -> camel_config::config::OidcSecurityConfig {
        camel_config::config::OidcSecurityConfig {
            issuer: "https://issuer.example.com".into(),
            jwks_uri: jwks_uri.map(String::from),
            audience: vec!["my-client".into()],
            client_id: Some("my-client".into()),
            client_secret: client_secret.map(String::from),
            token_endpoint: None,
            introspection_endpoint: None,
        }
    }

    #[tokio::test]
    async fn oidc_missing_jwks_uri_fails_closed() {
        let oidc = oidc_config(None, None);
        let security = camel_config::config::SecurityConfig {
            oidc: Some(oidc),
            ..Default::default()
        };

        let err = match super::resolve_authenticators(&security).await {
            Ok(_) => panic!("oidc without jwks_uri must fail closed"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("security.oidc.jwks_uri"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn oidc_unreachable_jwks_fails_closed_at_startup() {
        let oidc = oidc_config(Some("https://127.0.0.1:1/certs"), None);

        let err =
            match super::oidc_authenticator(&oidc, &camel_api::SsrfPolicy::PublicHttpsOnly).await {
                Ok(_) => panic!("loopback jwks_uri must fail closed"),
                Err(err) => err,
            };
        assert!(
            err.to_string().contains("https://127.0.0.1:1/certs"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn oidc_validator_authenticates_jwt() {
        use camel_auth::TokenAuthenticator;

        let oidc = oidc_config(Some("https://issuer.example.com/certs"), None);
        let jwks: Arc<dyn camel_auth::JwksProvider> = Arc::new(InMemoryJwks {
            kid: "test-key".into(),
            public_pem: TEST_RSA_PUBLIC_PEM,
        });
        let validator = super::oidc_validator(&oidc, jwks);

        let now = chrono::Utc::now().timestamp() as u64;
        let claims = serde_json::json!({
            "sub": "user-123",
            "iss": "https://issuer.example.com",
            "aud": "my-client",
            "exp": now + 3600,
            "iat": now,
        });
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = Some("test-key".into());
        let key = jsonwebtoken::EncodingKey::from_rsa_pem(TEST_RSA_PRIVATE_PEM).unwrap();
        let token = jsonwebtoken::encode(&header, &claims, &key).unwrap();

        let principal = validator.authenticate_bearer(&token).await.unwrap();
        assert_eq!(principal.subject, "user-123");
    }

    #[tokio::test]
    async fn oidc_marker_secret_guard_fires_before_network() {
        // loopback jwks_uri would be SSRF-rejected at provider construction,
        // so a "marker" (not "loopback") error proves the guard ran first.
        let oidc = oidc_config(Some("https://127.0.0.1:1/certs"), Some("{{env:X}}"));

        let err =
            match super::oidc_authenticator(&oidc, &camel_api::SsrfPolicy::PublicHttpsOnly).await {
                Ok(_) => panic!("marker client_secret must be rejected"),
                Err(err) => err,
            };
        assert!(
            err.to_string().contains("marker"),
            "unexpected error: {err}"
        );
    }

    // -----------------------------------------------------------------------
    // native multi-credential store tests (task 2.2) — construct
    // `NativeAuthConfig` directly (not via TOML) with every field explicit.
    // Filter `native_auth` matches the submodule name.
    // -----------------------------------------------------------------------

    mod native_auth {
        fn native_config() -> camel_config::config::NativeAuthConfig {
            camel_config::config::NativeAuthConfig {
                subject: "dev-user".into(),
                issuer: None,
                bearer_token: None,
                api_key: None,
                roles: vec![],
                scopes: vec![],
                credentials: vec![],
            }
        }

        fn native_entry(
            subject: &str,
            secret_env: Option<&str>,
            secret: Option<&str>,
            roles: Vec<&str>,
        ) -> camel_config::config::NativeCredentialEntry {
            camel_config::config::NativeCredentialEntry {
                subject: subject.into(),
                secret_env: secret_env.map(String::from),
                secret: secret.map(String::from),
                roles: roles.into_iter().map(String::from).collect(),
                scopes: vec![],
            }
        }

        #[tokio::test]
        async fn multi_entry_store_builds() {
            let native = camel_config::config::NativeAuthConfig {
                credentials: vec![
                    native_entry("ops", None, Some("ops-token"), vec!["admin"]),
                    native_entry("svc", None, Some("svc-token"), vec!["service"]),
                ],
                ..native_config()
            };

            let auth = super::super::native_authenticator(&native).expect("store builds");
            let ops = auth.authenticate_bearer("ops-token").await.unwrap();
            assert_eq!(ops.subject, "ops");
            assert_eq!(ops.roles, vec!["admin".to_string()]);
            let svc = auth.authenticate_bearer("svc-token").await.unwrap();
            assert_eq!(svc.subject, "svc");
            assert_eq!(svc.roles, vec!["service".to_string()]);
        }

        #[tokio::test]
        async fn api_key_only_starts() {
            let native = camel_config::config::NativeAuthConfig {
                api_key: Some("k-1".into()),
                roles: vec!["reader".into()],
                ..native_config()
            };

            assert!(
                super::super::native_authenticator(&native).is_ok(),
                "api_key-only config must build the store"
            );
        }

        #[tokio::test]
        async fn legacy_scalar_unchanged() {
            let native = camel_config::config::NativeAuthConfig {
                bearer_token: Some("legacy-token".into()),
                roles: vec!["admin".into()],
                ..native_config()
            };

            let auth = super::super::native_authenticator(&native).unwrap();
            let principal = auth.authenticate_bearer("legacy-token").await.unwrap();
            assert_eq!(principal.subject, "dev-user");
            assert_eq!(principal.roles, vec!["admin".to_string()]);
        }

        #[tokio::test]
        async fn empty_native_config_fails() {
            let native = native_config();

            let err = match super::super::native_authenticator(&native) {
                Ok(_) => panic!("credential-less [security.native] must fail closed"),
                Err(err) => err,
            };
            assert!(
                err.to_string().contains("without any credential"),
                "unexpected error: {err}"
            );
        }

        #[tokio::test]
        async fn missing_secret_env_fails_closed() {
            // SAFETY: this is the only test in the module that touches
            // environment variables, so no concurrent test can race on
            // AUTH_SVC_TOKEN.
            unsafe { std::env::remove_var("AUTH_SVC_TOKEN") };

            let native = camel_config::config::NativeAuthConfig {
                credentials: vec![native_entry("svc", Some("AUTH_SVC_TOKEN"), None, vec![])],
                ..native_config()
            };

            let err = match super::super::native_authenticator(&native) {
                Ok(_) => panic!("unset secret_env must fail closed"),
                Err(err) => err,
            };
            assert!(
                err.to_string().contains("AUTH_SVC_TOKEN"),
                "unexpected error: {err}"
            );
        }

        #[tokio::test]
        async fn entry_without_secret_errors_not_panics() {
            // Built directly, bypassing `validate_credentials` at config load.
            let native = camel_config::config::NativeAuthConfig {
                credentials: vec![native_entry("svc", None, None, vec![])],
                ..native_config()
            };

            let err = match super::super::native_authenticator(&native) {
                Ok(_) => panic!("entry without secret_env/secret must fail closed"),
                Err(err) => err,
            };
            assert!(
                err.to_string()
                    .contains("exactly one of secret_env or secret"),
                "unexpected error: {err}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Named-provider registration tests (task 3.3) — the CLI registers every
    // configured provider (XOR removed). Filter `multi_provider` matches the
    // submodule name.
    // -----------------------------------------------------------------------

    mod multi_provider {
        use std::sync::Arc;

        use camel_api::security_policy::AuthorizationDecision;

        use super::{InMemoryJwks, TEST_RSA_PRIVATE_PEM, TEST_RSA_PUBLIC_PEM, oidc_config};

        fn native_config_with_bearer(
            bearer_token: &str,
            roles: Vec<&str>,
        ) -> camel_config::config::NativeAuthConfig {
            camel_config::config::NativeAuthConfig {
                subject: "dev-user".into(),
                issuer: Some("native".into()),
                bearer_token: Some(bearer_token.into()),
                api_key: None,
                roles: roles.into_iter().map(String::from).collect(),
                scopes: vec![],
                credentials: vec![],
            }
        }

        fn oidc_auth_arc() -> Arc<dyn camel_auth::TokenAuthenticator> {
            let oidc = oidc_config(Some("https://issuer.example.com/certs"), None);
            let jwks: Arc<dyn camel_auth::JwksProvider> = Arc::new(InMemoryJwks {
                kid: "test-key".into(),
                public_pem: TEST_RSA_PUBLIC_PEM,
            });
            Arc::new(super::super::oidc_validator(&oidc, jwks))
        }

        fn exchange_with_bearer(token: &str) -> camel_api::Exchange {
            let mut msg = camel_api::Message::default();
            msg.set_header(
                "Authorization",
                serde_json::Value::String(format!("Bearer {token}")), // allow-secret: header name + token var, not a literal secret
            );
            camel_api::Exchange::new(msg)
        }

        fn admin_jwt() -> String {
            let now = chrono::Utc::now().timestamp() as u64;
            let claims = serde_json::json!({
                "sub": "user-123",
                "iss": "https://issuer.example.com",
                "aud": "my-client",
                "exp": now + 3600,
                "iat": now,
                "realm_access": { "roles": ["admin"] },
            });
            let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
            header.kid = Some("test-key".into());
            let key = jsonwebtoken::EncodingKey::from_rsa_pem(TEST_RSA_PRIVATE_PEM).unwrap();
            jsonwebtoken::encode(&header, &claims, &key).unwrap()
        }

        #[tokio::test]
        async fn native_only_registers_named() {
            let cfg: camel_config::config::CamelConfig = toml::from_str(
                r#"
        [security.native]
        subject = "dev-user"
        issuer = "native"
        bearer_token = "dev-token"
        "#,
            )
            .expect("config parses");

            let registry = Arc::new(std::sync::Mutex::new(camel_core::Registry::new()));
            let ctx = super::super::build_security_compile_context_from_config(&cfg, registry)
                .await
                .expect("security context builds");

            assert!(ctx.authenticator_for(Some("native")).unwrap().is_some());
        }

        #[tokio::test]
        async fn oidc_error_propagates() {
            let oidc = oidc_config(Some("https://127.0.0.1:1/certs"), None);
            let native = native_config_with_bearer("dev-token", vec![]);
            let security = camel_config::config::SecurityConfig {
                oidc: Some(oidc),
                native: Some(native),
                ..Default::default()
            };

            let err = match super::super::resolve_authenticators(&security).await {
                Ok(_) => panic!("broken oidc alongside valid native must abort resolution"),
                Err(err) => err,
            };
            assert!(
                err.to_string().contains("127.0.0.1"),
                "expected the oidc error, got: {err}"
            );
        }

        #[tokio::test]
        async fn sole_provider_back_compat() {
            let cfg: camel_config::config::CamelConfig = toml::from_str(
                r#"
        [security.native]
        subject = "dev-user"
        issuer = "native"
        bearer_token = "dev-token"
        "#,
            )
            .expect("config parses");

            let registry = Arc::new(std::sync::Mutex::new(camel_core::Registry::new()));
            let ctx = super::super::build_security_compile_context_from_config(&cfg, registry)
                .await
                .expect("security context builds");

            assert!(
                ctx.authenticator_for(Some("native")).unwrap().is_some(),
                "named provider must resolve"
            );
            assert!(
                ctx.authenticator_for(None).unwrap().is_some(),
                "sole named provider must resolve unnamed routes"
            );
        }

        #[tokio::test]
        async fn register_providers_registers_both() {
            let native_auth = super::super::native_authenticator(&native_config_with_bearer(
                "dev-token",
                vec!["admin"],
            ))
            .expect("native store builds");
            let oidc_auth = oidc_auth_arc();

            let ctx = super::super::register_providers(vec![
                ("native".to_string(), native_auth),
                ("oidc".to_string(), oidc_auth),
            ]);

            assert!(ctx.authenticator_for(Some("native")).unwrap().is_some());
            assert!(ctx.authenticator_for(Some("oidc")).unwrap().is_some());

            let err = match ctx.authenticator_for(None) {
                Err(err) => err,
                Ok(_) => panic!("ambiguous named providers must error"),
            };
            assert!(err.contains("native"), "got: {err}");
            assert!(err.contains("oidc"), "got: {err}");
        }

        #[tokio::test]
        async fn mixed_providers_route_selection_e2e() {
            let native_auth = super::super::native_authenticator(&native_config_with_bearer(
                "dev-token",
                vec!["admin"],
            ))
            .expect("native store builds");
            let oidc_auth = oidc_auth_arc();

            let ctx = super::super::register_providers(vec![
                ("native".to_string(), native_auth),
                ("oidc".to_string(), oidc_auth),
            ]);

            let yaml = r#"
routes:
  - id: route-native
    from: direct:start
    security_policy:
      roles: ["admin"]
      provider: "native"
    steps:
      - to: log:info
  - id: route-oidc
    from: direct:start
    security_policy:
      roles: ["admin"]
      provider: "oidc"
    steps:
      - to: log:info
"#;

            let defs = camel_dsl::parse_yaml_with_threshold_and_security(yaml, 1024, ctx)
                .expect("routes compile");
            assert_eq!(defs.len(), 2);

            let native_policy = defs[0].security_policy_config().expect("native policy");
            let oidc_policy = defs[1].security_policy_config().expect("oidc policy");

            // native token against route A (provider: native) -> granted (200).
            let mut ex = exchange_with_bearer("dev-token");
            let decision = native_policy.policy.evaluate(&mut ex).await.unwrap();
            assert!(matches!(decision, AuthorizationDecision::Granted { .. }));

            // JWT against route B (provider: oidc) -> granted (200).
            let token = admin_jwt();
            let mut ex = exchange_with_bearer(&token);
            let decision = oidc_policy.policy.evaluate(&mut ex).await.unwrap();
            assert!(matches!(decision, AuthorizationDecision::Granted { .. }));

            // native token against route B -> unauthenticated (401).
            let mut ex = exchange_with_bearer("dev-token");
            let result = oidc_policy.policy.evaluate(&mut ex).await;
            assert!(
                matches!(result, Err(camel_api::CamelError::Unauthenticated(_))),
                "native token must not authenticate against the oidc provider, got: {result:?}"
            );
        }
    }
}
