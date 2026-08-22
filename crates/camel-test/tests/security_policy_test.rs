//! Security policy layer semantics (carrier-only strict mode, Task 2.9).
//!
//! The layer never authenticates: transports mint the typed kernel carrier at
//! the request boundary and the pre-pipeline dispatch check rejects
//! carrier-less Exchanges on non-Public consumer routes. `direct:` routes
//! have no transport boundary, so these tests mint the carrier themselves
//! (`kernel_authenticate` + `install_carrier`, mirroring the HTTP consumer)
//! before dispatching through `send_to_direct` — the policy layer then
//! evaluates against that carrier: Granted stores principal properties,
//! Denied drops the exchange with `Unauthorized`, and policy errors
//! propagate.
//!
//! Requires `integration-tests` feature to compile and run.

#![cfg(feature = "integration-tests")]

mod support;
use support::send_to_direct;

use async_trait::async_trait;
use camel_api::security_policy::{
    AccessMode, AuthContext, AuthorizationDecision, CredentialSource, Principal, RouteSecurityPlan,
    SecurityPolicy, SecurityPolicyConfig, TransportId,
};
use camel_api::{CamelError, Exchange};
use camel_auth::credential_source::ExtractedToken;
use camel_auth::{install_carrier, kernel_authenticate};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_test::{CamelTestContext, SecurityConfigFixture};

struct GrantAllPolicy;

#[async_trait]
impl SecurityPolicy for GrantAllPolicy {
    async fn evaluate(
        &self,
        _exchange: &mut Exchange,
        _auth: &AuthContext<'_>,
    ) -> Result<AuthorizationDecision, CamelError> {
        Ok(AuthorizationDecision::Granted {
            principal: Principal {
                subject: "test-user".into(),
                issuer: "test-issuer".into(),
                audience: vec!["api".into()],
                scopes: vec!["read".into(), "write".into()],
                roles: vec!["admin".into()],
                claims: serde_json::json!({"sub": "test-user"}),
            },
        })
    }
}

struct DenyAllPolicy;

#[async_trait]
impl SecurityPolicy for DenyAllPolicy {
    async fn evaluate(
        &self,
        _exchange: &mut Exchange,
        _auth: &AuthContext<'_>,
    ) -> Result<AuthorizationDecision, CamelError> {
        Ok(AuthorizationDecision::Denied {
            reason: "no roles assigned".into(),
            required: vec!["admin".into()],
            actual: vec![],
        })
    }
}

struct FailPolicy;

#[async_trait]
impl SecurityPolicy for FailPolicy {
    async fn evaluate(
        &self,
        _exchange: &mut Exchange,
        _auth: &AuthContext<'_>,
    ) -> Result<AuthorizationDecision, CamelError> {
        Err(CamelError::Unauthenticated("invalid token".into()))
    }
}

/// Fixture provider used to mint the kernel carrier (same shape as
/// `kernel_fail_closed_test.rs`): static token `test-token-idp-secpol`
/// resolving to principal `test-user-idp-secpol`.
const FIXTURE_PROVIDER: &str = "idp-secpol";

/// Mint an Exchange carrying the typed kernel carrier, exactly as a
/// transport boundary does: authenticate the fixture token against the
/// fixture provider, then install the sealed principal.
async fn carrier_exchange() -> Exchange {
    let fixture = SecurityConfigFixture::single_static_provider(FIXTURE_PROVIDER);
    let providers = fixture.providers();
    let plan = RouteSecurityPlan {
        access_mode: AccessMode::Authenticated,
        provider_ref: Some(FIXTURE_PROVIDER.to_string()),
        transport: TransportId::Http,
        credential_sources: vec![CredentialSource::AuthorizationHeader],
        audience_binding: None,
    };
    let credentials = ExtractedToken {
        token: format!("test-token-{FIXTURE_PROVIDER}"),
        source: CredentialSource::AuthorizationHeader,
    };
    let principal = kernel_authenticate(&plan, &providers, &credentials)
        .await
        .expect("fixture token must authenticate"); // allow-unwrap
    let mut exchange = Exchange::default();
    install_carrier(&mut exchange, &principal);
    exchange
}

#[tokio::test(flavor = "multi_thread")]
async fn test_security_policy_granted_stores_properties() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("direct:sec-granted")
        .route_id("security-granted")
        .to("mock:result")
        .build()
        .unwrap()
        .with_security_policy(SecurityPolicyConfig::new(GrantAllPolicy));

    h.add_route(route).await.unwrap();
    h.start().await;

    let exchange = carrier_exchange().await;
    send_to_direct(&h, "direct:sec-granted", exchange)
        .await
        .expect("granted exchange must flow through"); // allow-unwrap

    let endpoint = h.mock().get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    let ex = &exchanges[0];
    assert_eq!(
        ex.property("camel.auth.subject"),
        Some(&serde_json::Value::String("test-user".into()))
    );
    assert_eq!(
        ex.property("camel.auth.issuer"),
        Some(&serde_json::Value::String("test-issuer".into()))
    );
    assert!(ex.property("camel.auth.roles").is_some());
    assert!(ex.property("camel.auth.scopes").is_some());
    assert!(ex.property("camel.auth.audience").is_some());
    assert!(ex.property("camel.auth.claims").is_some());
    assert!(ex.property("camel.auth.principal").is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_security_policy_denied_no_exchanges_reach_consumer() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("direct:sec-denied")
        .route_id("security-denied")
        .to("mock:result")
        .build()
        .unwrap()
        .with_security_policy(SecurityPolicyConfig::new(DenyAllPolicy));

    h.add_route(route).await.unwrap();
    h.start().await;

    let exchange = carrier_exchange().await;
    let result = send_to_direct(&h, "direct:sec-denied", exchange).await;
    match result {
        Err(CamelError::Unauthorized(msg)) => {
            assert!(msg.contains("no roles assigned"));
        }
        other => panic!("expected Unauthorized denial, got {other:?}"),
    }

    let endpoint = h.mock().get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(0).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_security_policy_error_propagates() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("direct:sec-error")
        .route_id("security-error")
        .to("mock:result")
        .build()
        .unwrap()
        .with_security_policy(SecurityPolicyConfig::new(FailPolicy));

    h.add_route(route).await.unwrap();
    h.start().await;

    let exchange = carrier_exchange().await;
    let result = send_to_direct(&h, "direct:sec-error", exchange).await;
    match result {
        Err(CamelError::Unauthenticated(msg)) => {
            assert!(msg.contains("invalid token"));
        }
        other => panic!("expected Unauthenticated error, got {other:?}"),
    }

    let endpoint = h.mock().get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(0).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_security_policy_granted_exact_roles_scopes_json() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("direct:sec-json-props")
        .route_id("security-json-props")
        .to("mock:result")
        .build()
        .unwrap()
        .with_security_policy(SecurityPolicyConfig::new(GrantAllPolicy));

    h.add_route(route).await.unwrap();
    h.start().await;

    let exchange = carrier_exchange().await;
    send_to_direct(&h, "direct:sec-json-props", exchange)
        .await
        .expect("granted exchange must flow through"); // allow-unwrap

    let endpoint = h.mock().get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    let ex = &exchanges[0];

    let roles: Vec<String> =
        serde_json::from_str(ex.property("camel.auth.roles").unwrap().as_str().unwrap()).unwrap();
    assert_eq!(roles, vec!["admin"]);

    let scopes: Vec<String> =
        serde_json::from_str(ex.property("camel.auth.scopes").unwrap().as_str().unwrap()).unwrap();
    assert_eq!(scopes, vec!["read", "write"]);

    let audience: Vec<String> = serde_json::from_str(
        ex.property("camel.auth.audience")
            .unwrap()
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(audience, vec!["api"]);

    let principal: serde_json::Value = serde_json::from_str(
        ex.property("camel.auth.principal")
            .unwrap()
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(principal["subject"], "test-user");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_security_policy_preserves_exchange_properties() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    struct PropertySettingPolicy;
    #[async_trait]
    impl SecurityPolicy for PropertySettingPolicy {
        async fn evaluate(
            &self,
            exchange: &mut Exchange,
            _auth: &AuthContext<'_>,
        ) -> Result<AuthorizationDecision, CamelError> {
            exchange.set_property("pre.auth.key", "before-value");
            Ok(AuthorizationDecision::Granted {
                principal: Principal {
                    subject: "u".into(),
                    issuer: "i".into(),
                    audience: vec![],
                    scopes: vec![],
                    roles: vec![],
                    claims: serde_json::Value::Null,
                },
            })
        }
    }

    let route = RouteBuilder::from("direct:sec-existing-props")
        .route_id("security-existing-props")
        .to("mock:result")
        .build()
        .unwrap()
        .with_security_policy(SecurityPolicyConfig::new(PropertySettingPolicy));

    h.add_route(route).await.unwrap();
    h.start().await;

    let exchange = carrier_exchange().await;
    send_to_direct(&h, "direct:sec-existing-props", exchange)
        .await
        .expect("granted exchange must flow through"); // allow-unwrap

    let endpoint = h.mock().get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    let ex = &exchanges[0];
    assert_eq!(
        ex.property("pre.auth.key"),
        Some(&serde_json::Value::String("before-value".into()))
    );
    assert!(ex.property("camel.auth.subject").is_some());
}
