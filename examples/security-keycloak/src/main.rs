//! Keycloak-style security example for rust-camel.
//!
//! Demonstrates role-based access control using the native auth pipeline:
//!   - Registers two principals (alice, bob) in a `NativeCredentialStore`
//!   - Authenticates pre-shared credentials via `StaticTokenAuthenticator`
//!   - Applies RolePolicy that checks for required roles
//!   - Shows Granted for admin user, Denied for viewer user
//!
//! No Docker or external Keycloak required — the same RolePolicy /
//! TokenAuthenticator pipeline works with a real Keycloak (OIDC) in production.
//!
//! Note: SecurityPolicyLayer evaluates BEFORE route steps (set_header, etc).
//! In production, the Bearer token arrives from HTTP consumers. This example
//! uses a timer consumer, so the token is injected via a wrapper policy.
//!
//! Run:
//!
//!   cargo run -p security-keycloak

use std::sync::Arc;

use async_trait::async_trait;
use camel_api::security_policy::{
    AuthContext, AuthPrincipal, AuthorizationDecision, Principal, SecurityPolicy,
    SecurityPolicyConfig, TransportId,
};
use camel_api::{CamelError, Exchange, Value};
use camel_auth::native_auth::{
    NativeCredential, NativeCredentialSecret, NativeCredentialStore, StaticTokenAuthenticator,
};
use camel_auth::{RolePolicy, TokenAuthenticator};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
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
    async fn evaluate(
        &self,
        exchange: &mut Exchange,
        auth: &AuthContext<'_>,
    ) -> Result<AuthorizationDecision, CamelError> {
        exchange.input.headers.insert(
            "authorization".to_string(),
            Value::String(format!("Bearer {}", self.token)), // allow-secret
        );
        self.inner.evaluate(exchange, auth).await
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_target(false).init();

    let issuer = "https://keycloak.example.com/realms/test";
    let audience = vec!["camel-api".to_string()];

    let store = NativeCredentialStore::try_new(vec![
        NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: "alice-token".to_string().into(),
            },
            principal: Principal {
                subject: "alice".to_string(),
                issuer: issuer.to_string(),
                audience: audience.clone(),
                scopes: vec!["read".to_string(), "write".to_string()],
                roles: vec!["admin".to_string(), "user".to_string()],
                claims: serde_json::Value::Null,
            },
        },
        NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: "bob-token".to_string().into(),
            },
            principal: Principal {
                subject: "bob".to_string(),
                issuer: issuer.to_string(),
                audience: audience.clone(),
                scopes: vec!["read".to_string()],
                roles: vec!["viewer".to_string()],
                claims: serde_json::Value::Null,
            },
        },
    ])?;

    let authenticator: Arc<dyn TokenAuthenticator> = Arc::new(StaticTokenAuthenticator::new(store));

    // ANCHOR: keycloak-token-issuance
    let alice_token = "alice-token";
    let bob_token = "bob-token";
    // ANCHOR_END: keycloak-token-issuance

    println!("\n=== Keycloak Security Example ===");
    println!("Issuer:  {issuer}");
    println!("Alice:   admin,user roles -> static auth");
    println!("Bob:     viewer role      -> static auth");
    println!();

    // ANCHOR: keycloak-validation
    println!("--- Validation ---");

    let alice_principal = authenticator.authenticate_bearer(alice_token).await;
    match &alice_principal {
        Ok(p) => println!("Alice OK subject={} roles={:?}", p.subject, p.roles), // allow-secret
        Err(e) => println!("Alice invalid ({e})"),                               // allow-secret
    }

    let bob_principal = authenticator.authenticate_bearer(bob_token).await;
    match &bob_principal {
        Ok(p) => println!("Bob: VALID  (subject={}, roles={:?})", p.subject, p.roles),
        Err(e) => println!("Bob: INVALID ({e})"),
    }
    // ANCHOR_END: keycloak-validation
    println!();

    // ANCHOR: keycloak-role-policy
    println!("--- Role-Based Security Policy ---");
    let admin_policy: Arc<dyn SecurityPolicy> =
        Arc::new(RolePolicy::new(vec!["admin".to_string()], true));

    let mut alice_exchange = Exchange::default();
    alice_exchange.input.headers.insert(
        "authorization".to_string(),
        Value::String(format!("Bearer {}", alice_token)), // allow-secret
    );

    let mut bob_exchange = Exchange::default();
    bob_exchange.input.headers.insert(
        "authorization".to_string(),
        Value::String(format!("Bearer {}", bob_token)), // allow-secret
    );

    let alice_principal = authenticator.authenticate_bearer(alice_token).await?;
    let alice_typed = ExamplePrincipal(alice_principal);
    let alice_auth = AuthContext {
        principal: &alice_typed,
        transport: TransportId::Http,
    };

    let bob_principal = authenticator.authenticate_bearer(bob_token).await?;
    let bob_typed = ExamplePrincipal(bob_principal);
    let bob_auth = AuthContext {
        principal: &bob_typed,
        transport: TransportId::Http,
    };

    let alice_decision = admin_policy
        .evaluate(&mut alice_exchange, &alice_auth)
        .await;
    let bob_decision = admin_policy.evaluate(&mut bob_exchange, &bob_auth).await;

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
        _ => println!("Alice vs RolePolicy[admin]: UNKNOWN decision"),
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
        _ => println!("Bob vs RolePolicy[admin]:   UNKNOWN decision"),
    }

    println!();
    println!("--- Route with Security Policy ---");

    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let role_policy = RolePolicy::new(vec!["admin".to_string()], true);
    let wrapped = BearerInjectingPolicy::new(alice_token.to_string(), role_policy);

    let secured_route = RouteBuilder::from("timer:tick?period=2000&repeatCount=2")
        .route_id("admin-only-route")
        .security_policy(SecurityPolicyConfig::new(wrapped))
        .to("log:info?showHeaders=true")
        .build()?;

    ctx.add_route_definition(secured_route).await?;
    // ANCHOR_END: keycloak-role-policy
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
