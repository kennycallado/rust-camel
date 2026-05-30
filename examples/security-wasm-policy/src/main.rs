//! WASM authorization-policy example for rust-camel.
//!
//! Demonstrates the full auth pipeline with a WASM authorization policy:
//!   - Issues a JWT with roles using NativeTokenIssuer
//!   - Validates tokens via LocalJwtValidator + NativeJwksProvider
//!   - A wrapper SecurityPolicy authenticates a pre-issued token,
//!     populates camel.auth.* properties, then delegates to WASM
//!   - The WASM plugin reads those properties and grants/denies access
//!
//! In production the Bearer token comes from HTTP requests. This example
//! uses a timer consumer so the token is injected directly into the policy.
//!
//! Run:
//!
//!   cargo run -p security-wasm-policy

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use camel_api::security_policy::{
    AuthorizationDecision, SecurityPolicy, SecurityPolicyConfig, store_principal_properties,
};
use camel_api::{CamelError, Exchange};
use camel_auth::{
    ClaimPaths, JsonPointerClaimsMapper, LocalJwtValidator, M2mClient, M2mClientSecret,
    M2mClientStore, NativeJwksProvider, NativeSigningKey, NativeTokenIssuer, TokenAuthenticator,
};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_component_wasm::{WasmConfig, WasmSecurityPolicy};
use camel_core::context::CamelContext;

const RSA_PRIVATE_PEM: &str = include_str!("../fixtures/test_key.pem");

struct AuthenticatedWasmPolicy {
    authenticator: Arc<dyn TokenAuthenticator>,
    token: String,
    inner: WasmSecurityPolicy,
}

impl AuthenticatedWasmPolicy {
    fn new(
        authenticator: Arc<dyn TokenAuthenticator>,
        token: String,
        inner: WasmSecurityPolicy,
    ) -> Self {
        Self {
            authenticator,
            token,
            inner,
        }
    }
}

#[async_trait]
impl SecurityPolicy for AuthenticatedWasmPolicy {
    async fn evaluate(&self, exchange: &mut Exchange) -> Result<AuthorizationDecision, CamelError> {
        let principal = self.authenticator.authenticate_bearer(&self.token).await?;
        store_principal_properties(exchange, &principal);
        self.inner.evaluate(exchange).await
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_target(false).init();

    let issuer = "https://wasm-example.local/realms/test";
    let audience = vec!["camel-api".to_string()];

    let signing_key = NativeSigningKey::from_pem(RSA_PRIVATE_PEM, "wasm-example-key".to_string())?;
    let signing_key_val =
        NativeSigningKey::from_pem(RSA_PRIVATE_PEM, "wasm-example-key".to_string())?;

    let client_store = M2mClientStore::try_new(vec![
        M2mClient {
            client_id: "alice".to_string(),
            secret: M2mClientSecret::Plaintext {
                value: "alice-secret".to_string(),
            },
            scopes: vec!["read".to_string(), "write".to_string()],
            roles: vec!["admin".to_string(), "user".to_string()],
        },
        M2mClient {
            client_id: "bob".to_string(),
            secret: M2mClientSecret::Plaintext {
                value: "bob-secret".to_string(),
            },
            scopes: vec!["read".to_string()],
            roles: vec!["viewer".to_string()],
        },
    ])?;

    let token_issuer = NativeTokenIssuer::try_new(
        issuer.to_string(),
        audience.clone(),
        Duration::from_secs(300),
        signing_key,
        client_store,
    )?;

    let jwks = Arc::new(NativeJwksProvider::new(signing_key_val)?);
    let mapper = Arc::new(JsonPointerClaimsMapper::new(ClaimPaths {
        subject: "/sub".to_string(),
        roles: vec!["/roles".to_string()],
        scopes: Some("/scope".to_string()),
    }));
    let validator = Arc::new(LocalJwtValidator::new(
        audience,
        issuer.to_string(),
        jwks,
        mapper,
    )) as Arc<dyn TokenAuthenticator>;

    let alice_token = token_issuer
        .issue_token("alice", "alice-secret", Some("read write"), None)
        .await?;

    let fixtures_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    let wasm_path = fixtures_dir.join("role-check.wasm");
    let registry = Arc::new(std::sync::Mutex::new(camel_core::Registry::new()));

    let wasm_policy =
        WasmSecurityPolicy::new(&wasm_path, WasmConfig::default(), registry, HashMap::new())
            .await?;

    let policy =
        AuthenticatedWasmPolicy::new(validator, alice_token.access_token.clone(), wasm_policy);

    println!("\n=== WASM Security Policy Example ===");
    println!("Plugin:    role-check.wasm (authorization-policy world)");
    println!("Auth:      NativeTokenIssuer + LocalJwtValidator");
    println!("Alice:     admin,user roles -> JWT issued");
    println!();

    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from("timer:tick?period=1000&repeatCount=5")
        .route_id("wasm-secured-route")
        .security_policy(SecurityPolicyConfig::new(policy))
        .to("log:info?showHeaders=true")
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("Route: timer -> [AuthenticatedWasmPolicy: auth + WASM check] -> log");
    println!("Flow: policy authenticates Alice JWT, populates camel.auth.roles,");
    println!("      WASM plugin reads property, grants access (admin role present).");
    println!("Running for ~6s...\n");

    tokio::time::sleep(tokio::time::Duration::from_secs(7)).await;

    ctx.stop().await?;
    println!("\nDone.");
    Ok(())
}
