//! Multi-credential native auth end-to-end (auth-reinforcement task 2.4).
//!
//! Loads a real `Camel.toml` through `CamelConfig::from_file`, synthesizes the
//! native credential store from the deserialized `[security.native]` block
//! (mirroring `camel-cli::native_authenticator`, which camel-test cannot reach
//! under ADR-0055), then drives an HTTP route through the `http_test.rs`
//! harness. Closes the route-enforcement half of the credentials array and the
//! api-key-only scenario.
//!
//! Requires `integration-tests` feature to compile and run.

#![cfg(feature = "integration-tests")]

mod support;
use support::install_crypto_provider;

use std::sync::Arc;
use std::time::Duration;

use camel_api::security_policy::{CredentialSource, Principal, SecurityPolicyConfig};
use camel_api::{CamelError, Value};
use camel_auth::native_auth::{NativeCredential, NativeCredentialSecret, NativeCredentialStore};
use camel_auth::{RolePolicy, StaticTokenAuthenticator, TokenAuthenticator};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_http::HttpComponent;
use camel_config::CamelConfig;
use camel_config::config::NativeAuthConfig;
use camel_test::CamelTestContext;

fn find_free_port() -> u16 {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind to free port");
    listener.local_addr().unwrap().port()
}

fn native_principal(subject: &str, issuer: &str, roles: &[String], scopes: &[String]) -> Principal {
    Principal {
        subject: subject.to_string(),
        issuer: issuer.to_string(),
        audience: Vec::new(),
        roles: roles.to_vec(),
        scopes: scopes.to_vec(),
        claims: serde_json::json!({}),
    }
}

/// Synthesize the native credential store from a deserialized `NativeAuthConfig`,
/// mirroring `camel-cli::native_authenticator`. camel-test cannot depend on
/// camel-cli (ADR-0055), so the store synthesis is reproduced here from the
/// same `camel_auth` primitives the CLI uses.
/// Lockstep with `native_authenticator` (crates/camel-cli/src/security.rs)
/// and `SecurityConfigFixture` (crates/camel-test/src/security_fixture.rs).
fn native_store_from_config(
    native: &NativeAuthConfig,
) -> Result<NativeCredentialStore, CamelError> {
    let issuer = native.issuer.clone().unwrap_or_else(|| "native".into());
    let mut credentials: Vec<NativeCredential> = Vec::new();

    if let Some(token) = &native.bearer_token {
        credentials.push(NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: token.clone().into(),
            },
            principal: native_principal(&native.subject, &issuer, &native.roles, &native.scopes),
        });
    }

    if let Some(api_key) = &native.api_key {
        credentials.push(NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: api_key.clone().into(),
            },
            principal: native_principal(&native.subject, &issuer, &native.roles, &native.scopes),
        });
    }

    for entry in &native.credentials {
        let secret = match (&entry.secret_env, &entry.secret) {
            (Some(name), None) => NativeCredentialSecret::Env { name: name.clone() },
            (None, Some(value)) => NativeCredentialSecret::Plaintext {
                value: value.clone().into(),
            },
            // `NativeAuthConfig::validate_credentials` (config load) guarantees
            // exactly one of `secret_env` / `secret`; fail closed defensively.
            _ => {
                return Err(CamelError::Config(
                    "security.native.credentials must set exactly one of secret_env or secret"
                        .to_string(),
                ));
            }
        };
        credentials.push(NativeCredential {
            secret,
            principal: native_principal(&entry.subject, &issuer, &entry.roles, &entry.scopes),
        });
    }

    if credentials.is_empty() {
        return Err(CamelError::Config(
            "security.native configured without any credential: set bearer_token, api_key, or [[security.native.credentials]]"
                .into(),
        ));
    }

    NativeCredentialStore::try_new(credentials)
}

/// Build and start an HTTP route protected by a `RolePolicy` over the store
/// synthesized from `native`. Mirrors `build_secure_route` in `http_test.rs`.
async fn build_secure_route(
    native: &NativeAuthConfig,
    path: &str,
    required_roles: Vec<String>,
    sources: Vec<CredentialSource>,
) -> (CamelTestContext, u16) {
    install_crypto_provider();
    let port = find_free_port();
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    let store = native_store_from_config(native).expect("store builds from config");
    let authenticator: Arc<dyn TokenAuthenticator> = Arc::new(StaticTokenAuthenticator::new(store));

    // The kernel compiles plans against the provider registry (sole-provider
    // rule); the bare authenticator alone is not a registered provider.
    let provider_registry = {
        let registry = camel_auth::ProviderRegistry::new();
        registry.register(
            "native",
            camel_auth::ProviderEntry {
                authenticator: Arc::clone(&authenticator),
                audience_binding: None,
            },
        );
        Arc::new(registry)
    };

    let policy = RolePolicy::new(required_roles, true);
    let config = SecurityPolicyConfig::new(policy).with_credential_sources(sources);

    let route = RouteBuilder::from(&format!("http://0.0.0.0:{port}/{path}"))
        .route_id(format!("secure-http-{path}"))
        .security_policy(config)
        .security_authenticator(authenticator)
        .provider_registry(provider_registry)
        .set_body(Value::String("ok".into()))
        .set_header("CamelHttpResponseCode", Value::Number(200.into()))
        .to(format!("mock:{path}"))
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    (h, port)
}

/// Write `toml_str` into a tempdir `Camel.toml` and load it through the same
/// `CamelConfig::from_file` path `camel run` uses.
fn load_camel_toml(toml_str: &str) -> CamelConfig {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("Camel.toml");
    std::fs::write(&path, toml_str).expect("write Camel.toml");
    CamelConfig::from_file(path.to_str().unwrap()).expect("Camel.toml loads")
}

#[tokio::test(flavor = "multi_thread")]
async fn two_principals_enforce_roles_e2e() {
    const ENV_VAR: &str = "CAMEL_TEST_OPS_TOKEN";
    const OPS_TOKEN: &str = "ops-secret-token";
    const SVC_TOKEN: &str = "svc-plaintext-token";

    // SAFETY: unique env var name, no other test reads or writes it; the store
    // resolves it synchronously during this test before it is removed.
    unsafe { std::env::set_var(ENV_VAR, OPS_TOKEN) };

    let toml_str = format!(
        r#"
[security.native]
subject = "native"
issuer = "native"

[[security.native.credentials]]
subject = "ops"
secret_env = "{ENV_VAR}"
roles = ["admin"]

[[security.native.credentials]]
subject = "svc"
secret = "{SVC_TOKEN}"
roles = ["service"]
"#
    );

    let config = load_camel_toml(&toml_str);
    let native = config
        .security
        .native
        .as_ref()
        .expect("native config present");
    assert_eq!(
        native.credentials.len(),
        2,
        "two credentials must deserialize"
    );

    let (_h, port) = build_secure_route(
        native,
        "secure",
        vec!["admin".to_string()],
        vec![CredentialSource::AuthorizationHeader],
    )
    .await;

    let client = reqwest::Client::new();

    // ops token (admin) → 200
    let resp = client
        .get(format!("http://127.0.0.1:{port}/secure"))
        .header("Authorization", format!("Bearer {OPS_TOKEN}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "ops admin token must be authorized");

    // svc token (service, not admin) → 403
    let resp = client
        .get(format!("http://127.0.0.1:{port}/secure"))
        .header("Authorization", format!("Bearer {SVC_TOKEN}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403, "svc service token must be forbidden");

    // unknown token → 401
    let resp = client
        .get(format!("http://127.0.0.1:{port}/secure"))
        .header("Authorization", "Bearer unknown-token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "unknown token must be unauthenticated");

    // SAFETY: see set_var above; restore a clean environment.
    unsafe { std::env::remove_var(ENV_VAR) };
}

#[tokio::test(flavor = "multi_thread")]
async fn api_key_only_custom_header_enforces_e2e() {
    const API_KEY: &str = "reader-api-key-123";
    const HEADER: &str = "X-API-Key";

    let toml_str = format!(
        r#"
[security.native]
subject = "reader-user"
api_key = "{API_KEY}"
roles = ["reader"]
"#
    );

    let config = load_camel_toml(&toml_str);
    let native = config
        .security
        .native
        .as_ref()
        .expect("native config present");
    assert!(
        native.credentials.is_empty(),
        "api_key-only config must have no credentials entries"
    );

    let (_h, port) = build_secure_route(
        native,
        "apikey",
        vec!["reader".to_string()],
        vec![CredentialSource::Header {
            name: HEADER.to_string(),
        }],
    )
    .await;

    let client = reqwest::Client::new();

    // valid X-API-Key header → 200
    let ok = client
        .get(format!("http://127.0.0.1:{port}/apikey"))
        .header(HEADER, API_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200, "valid API key must be authorized");

    // missing header → 401
    let missing = client
        .get(format!("http://127.0.0.1:{port}/apikey"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        missing.status(),
        401,
        "missing API key must be unauthenticated"
    );
}
