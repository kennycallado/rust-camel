//! Keycloak-style security example for rust-camel.
//!
//! Demonstrates role-based access control using the native auth pipeline:
//!   - Issues JWTs with roles using NativeTokenIssuer (simulates Keycloak)
//!   - Validates tokens via LocalJwtValidator + NativeJwksProvider
//!   - Applies RolePolicy that checks for required roles
//!   - Shows Granted for admin user, Denied for viewer user
//!
//! No Docker or external Keycloak required — uses the same auth
//! pipeline that works with a real Keycloak in production.
//!
//! Note: SecurityPolicyLayer evaluates BEFORE route steps (set_header, etc).
//! In production, the Bearer token arrives from HTTP consumers. This example
//! uses a timer consumer, so the token is injected via a wrapper policy.
//!
//! Run:
//!
//!   cargo run -p security-keycloak

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use camel_api::security_policy::{AuthorizationDecision, SecurityPolicy, SecurityPolicyConfig};
use camel_api::{CamelError, Exchange, Value};
use camel_auth::{
    ClaimPaths, JsonPointerClaimsMapper, LocalJwtValidator, M2mClient, M2mClientSecret,
    M2mClientStore, NativeJwksProvider, NativeSigningKey, NativeTokenIssuer, RolePolicy,
    TokenAuthenticator,
};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

const RSA_PRIVATE_PEM: &str = include_str!("../fixtures/test_key.pem");

struct BearerInjectingPolicy {
    token: String,
    inner: RolePolicy,
}

impl BearerInjectingPolicy {
    fn new(token: String, inner: RolePolicy) -> Self {
        Self { token, inner }
    }
}

#[async_trait]
impl SecurityPolicy for BearerInjectingPolicy {
    async fn evaluate(&self, exchange: &mut Exchange) -> Result<AuthorizationDecision, CamelError> {
        exchange.input.headers.insert(
            "authorization".to_string(),
            Value::String(format!("Bearer {}", self.token)), // allow-secret
        );
        self.inner.evaluate(exchange).await
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_target(false).init();

    let issuer = "https://keycloak.example.com/realms/test";
    let audience = vec!["camel-api".to_string()];

    let issuer_signing_key = NativeSigningKey::from_pem(RSA_PRIVATE_PEM, "test-key-1".to_string())?;
    let validator_signing_key =
        NativeSigningKey::from_pem(RSA_PRIVATE_PEM, "test-key-1".to_string())?;

    let client_store = M2mClientStore::try_new(vec![
        M2mClient {
            client_id: "alice".to_string(),
            secret: M2mClientSecret::Plaintext {
                value: "alice-secret".to_string().into(),
            },
            scopes: vec!["read".to_string(), "write".to_string()],
            roles: vec!["admin".to_string(), "user".to_string()],
        },
        M2mClient {
            client_id: "bob".to_string(),
            secret: M2mClientSecret::Plaintext {
                value: "bob-secret".to_string().into(),
            },
            scopes: vec!["read".to_string()],
            roles: vec!["viewer".to_string()],
        },
    ])?;

    let token_issuer = NativeTokenIssuer::try_new(
        issuer.to_string(),
        audience.clone(),
        Duration::from_secs(300),
        issuer_signing_key,
        client_store,
    )?;

    let jwks_provider = Arc::new(NativeJwksProvider::new(validator_signing_key)?);
    let claims_mapper = Arc::new(JsonPointerClaimsMapper::new(ClaimPaths {
        subject: "/sub".to_string(),
        roles: vec!["/roles".to_string()],
        scopes: Some("/scope".to_string()),
    }));
    let validator = Arc::new(LocalJwtValidator::new(
        audience,
        issuer.to_string(),
        jwks_provider,
        claims_mapper,
    )) as Arc<dyn TokenAuthenticator>;

    let alice_token = token_issuer
        .issue_token("alice", "alice-secret", Some("read write"), None)
        .await?;
    let bob_token = token_issuer
        .issue_token("bob", "bob-secret", Some("read"), None)
        .await?;

    println!("\n=== Keycloak Security Example ===");
    println!("Issuer:  {issuer}");
    println!("Alice:   admin,user roles -> JWT issued");
    println!("Bob:     viewer role      -> JWT issued");
    println!();

    println!("--- JWT Validation ---");

    let alice_principal = validator
        .authenticate_bearer(&alice_token.access_token)
        .await;
    match &alice_principal {
        Ok(p) => println!("Alice OK subject={} roles={:?}", p.subject, p.roles), // allow-secret
        Err(e) => println!("Alice invalid ({e})"),                               // allow-secret
    }

    let bob_principal = validator.authenticate_bearer(&bob_token.access_token).await;
    match &bob_principal {
        Ok(p) => println!(
            "Bob JWT:   VALID  (subject={}, roles={:?})",
            p.subject, p.roles
        ),
        Err(e) => println!("Bob JWT:   INVALID ({e})"),
    }
    println!();

    println!("--- Role-Based Security Policy ---");
    let admin_policy: Arc<dyn SecurityPolicy> = Arc::new(RolePolicy::new(
        vec!["admin".to_string()],
        true,
        false,
        validator.clone(),
    ));

    let mut alice_exchange = Exchange::default();
    alice_exchange.input.headers.insert(
        "authorization".to_string(),
        Value::String(format!("Bearer {}", *alice_token.access_token)), // allow-secret
    );

    let mut bob_exchange = Exchange::default();
    bob_exchange.input.headers.insert(
        "authorization".to_string(),
        Value::String(format!("Bearer {}", *bob_token.access_token)), // allow-secret
    );

    let alice_decision = admin_policy.evaluate(&mut alice_exchange).await;
    let bob_decision = admin_policy.evaluate(&mut bob_exchange).await;

    match alice_decision {
        Ok(AuthorizationDecision::Granted { principal }) => {
            println!(
                "Alice vs RolePolicy[admin]: GRANTED (subject={})",
                principal.subject
            );
        }
        Ok(AuthorizationDecision::Denied { reason, .. }) => {
            println!("Alice vs RolePolicy[admin]: DENIED ({reason})");
        }
        Err(e) => println!("Alice vs RolePolicy[admin]: ERROR ({e})"),
    }

    match bob_decision {
        Ok(AuthorizationDecision::Granted { principal }) => {
            println!(
                "Bob vs RolePolicy[admin]:   GRANTED (subject={})",
                principal.subject
            );
        }
        Ok(AuthorizationDecision::Denied { reason, .. }) => {
            println!("Bob vs RolePolicy[admin]:   DENIED ({reason})");
        }
        Err(e) => println!("Bob vs RolePolicy[admin]:   ERROR ({e})"),
    }

    println!();
    println!("--- Route with Security Policy ---");

    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let role_policy = RolePolicy::new(vec!["admin".to_string()], true, false, validator.clone());
    let wrapped = BearerInjectingPolicy::new(alice_token.access_token.to_string(), role_policy);

    let secured_route = RouteBuilder::from("timer:tick?period=2000&repeatCount=2")
        .route_id("admin-only-route")
        .security_policy(SecurityPolicyConfig::new(wrapped))
        .to("log:info?showHeaders=true")
        .build()?;

    ctx.add_route_definition(secured_route).await?;
    ctx.start().await?;

    println!("Route: timer -> [BearerInjectingPolicy -> RolePolicy[admin]] -> log");
    println!("Wrapper injects Authorization header before RolePolicy evaluates.");
    println!("Running for ~5s...\n");

    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

    ctx.stop().await?;

    println!("\n--- Summary ---");
    println!("Alice (admin,user): GRANTED - has admin role");
    println!("Bob (viewer):       DENIED  - missing admin role");
    println!("\nDone.");
    Ok(())
}
