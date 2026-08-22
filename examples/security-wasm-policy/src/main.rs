//! WASM authorization-policy example for rust-camel.
//!
//! Demonstrates the full auth pipeline with a WASM authorization policy:
//!   - Registers a principal (alice) in a `NativeCredentialStore`
//!   - Authenticates a pre-shared credential via `StaticTokenAuthenticator`
//!   - A wrapper SecurityPolicy authenticates the credential, populates
//!     camel.auth.* properties, then delegates to WASM
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

use async_trait::async_trait;
use camel_api::security_policy::{
    AuthContext, AuthPrincipal, AuthorizationDecision, Principal, SecurityPolicy,
    SecurityPolicyConfig, TransportId, store_principal_properties,
};
use camel_api::{CamelError, Exchange};
use camel_auth::TokenAuthenticator;
use camel_auth::native_auth::{
    NativeCredential, NativeCredentialSecret, NativeCredentialStore, StaticTokenAuthenticator,
};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_component_wasm::{WasmConfig, WasmSecurityPolicy};
use camel_core::context::CamelContext;

struct ExamplePrincipal(Principal);

impl AuthPrincipal for ExamplePrincipal {
    fn principal(&self) -> &Principal {
        &self.0
    }
    fn provider_id(&self) -> &str {
        "example"
    }
}

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
    async fn evaluate(
        &self,
        exchange: &mut Exchange,
        _auth: &AuthContext<'_>,
    ) -> Result<AuthorizationDecision, CamelError> {
        let principal = self.authenticator.authenticate_bearer(&self.token).await?;
        store_principal_properties(exchange, &principal);
        let typed = ExamplePrincipal(principal);
        let auth = AuthContext {
            principal: &typed,
            transport: TransportId::Http,
        };
        self.inner.evaluate(exchange, &auth).await
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_target(false).init();

    let issuer = "https://wasm-example.local/realms/test";
    let audience = vec!["camel-api".to_string()];

    let store = NativeCredentialStore::try_new(vec![NativeCredential {
        secret: NativeCredentialSecret::Plaintext {
            value: "alice-token".to_string().into(),
        },
        principal: Principal {
            subject: "alice".to_string(),
            issuer: issuer.to_string(),
            audience,
            scopes: vec!["read".to_string(), "write".to_string()],
            roles: vec!["admin".to_string(), "user".to_string()],
            claims: serde_json::Value::Null,
        },
    }])?;

    let authenticator: Arc<dyn TokenAuthenticator> = Arc::new(StaticTokenAuthenticator::new(store));

    let alice_token = "alice-token";

    // ANCHOR: wasm-policy-setup
    let fixtures_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    let wasm_path = fixtures_dir.join("role-check.wasm");
    let registry = Arc::new(std::sync::Mutex::new(camel_core::Registry::new()));

    // This example uses the programmatic WasmSecurityPolicy::new() API.
    // For production routes, prefer Camel.toml registration via
    // [security.policies.wasm.<name>] + YAML `security_policy: wasm: <name>`.
    // See crates/components/camel-component-wasm/README.md for details.
    let wasm_policy = WasmSecurityPolicy::new(
        &wasm_path,
        WasmConfig::default(),
        Arc::new(camel_core::RegistryComponentContext::new(registry)),
        HashMap::new(),
    )
    .await?;

    let policy = AuthenticatedWasmPolicy::new(authenticator, alice_token.to_string(), wasm_policy);
    // ANCHOR_END: wasm-policy-setup

    println!("\n=== WASM Security Policy Example ===");
    println!("Plugin:    role-check.wasm (authorization-policy world)");
    println!("Auth:      StaticTokenAuthenticator + NativeCredentialStore");
    println!("Alice:     admin,user roles -> static auth");
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
    println!("Flow: policy authenticates Alice, populates camel.auth.roles,");
    println!("      WASM plugin reads property, grants access (admin role present).");
    println!("Running for ~6s...\n");

    tokio::time::sleep(tokio::time::Duration::from_secs(7)).await;

    ctx.stop().await?;
    println!("\nDone.");
    Ok(())
}
