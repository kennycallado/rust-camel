use std::time::Duration;

use async_trait::async_trait;
use camel_api::security_policy::{
    AuthorizationDecision, Principal, SecurityPolicy, SecurityPolicyConfig,
};
use camel_api::{CamelError, Exchange};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_test::CamelTestContext;

struct GrantAllPolicy;

#[async_trait]
impl SecurityPolicy for GrantAllPolicy {
    async fn evaluate(
        &self,
        _exchange: &mut Exchange,
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
    ) -> Result<AuthorizationDecision, CamelError> {
        Err(CamelError::Unauthenticated("invalid token".into()))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_security_policy_granted_stores_properties() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .route_id("security-granted")
        .to("mock:result")
        .build()
        .unwrap()
        .with_security_policy(SecurityPolicyConfig::new(GrantAllPolicy));

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    h.stop().await;

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
        .with_timer()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=2")
        .route_id("security-denied")
        .to("mock:result")
        .build()
        .unwrap()
        .with_security_policy(SecurityPolicyConfig::new(DenyAllPolicy));

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    h.stop().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(0).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_security_policy_error_propagates() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=2")
        .route_id("security-error")
        .to("mock:result")
        .build()
        .unwrap()
        .with_security_policy(SecurityPolicyConfig::new(FailPolicy));

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    h.stop().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(0).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_security_policy_granted_exact_roles_scopes_json() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .route_id("security-json-props")
        .to("mock:result")
        .build()
        .unwrap()
        .with_security_policy(SecurityPolicyConfig::new(GrantAllPolicy));

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    h.stop().await;

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
        .with_timer()
        .with_mock()
        .build()
        .await;

    struct PropertySettingPolicy;
    #[async_trait]
    impl SecurityPolicy for PropertySettingPolicy {
        async fn evaluate(
            &self,
            exchange: &mut Exchange,
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

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .route_id("security-existing-props")
        .to("mock:result")
        .build()
        .unwrap()
        .with_security_policy(SecurityPolicyConfig::new(PropertySettingPolicy));

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    h.stop().await;

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
