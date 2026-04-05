//! Integration tests for the JMS component.
//!
//! **Requires Docker to be running.**
//! **Requires `integration-tests` feature:** `cargo test -p camel-test --features integration-tests`
//! **Requires the JMS bridge binary.** Set `CAMEL_JMS_BRIDGE_RELEASE_URL` or pre-build.
//!
//! Tests share a single bridge process per broker type (via `support::jms`) so they can
//! run in parallel without exhausting system resources.

#![cfg(feature = "integration-tests")]

mod support;

use std::time::Duration;

use camel_api::Value;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_jms::{BrokerType, JmsComponent, JmsConfig};
use camel_test::CamelTestContext;
use support::jms::{shared_jms_activemq, shared_jms_artemis, shared_jms_artemis_auth};
use support::wait::wait_until;

fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let _ = fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn,camel=info")),
        )
        .with_test_writer()
        .try_init();
}

#[tokio::test]
async fn jms_producer_sends_to_activemq() {
    init_tracing();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(shared_jms_activemq().await)
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body("hello-jms".to_string())
        .to("jms:queue:test-produce")
        .to("mock:sent")
        .route_id("jms-producer-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("sent").unwrap();
    wait_until(
        "jms producer route delivery",
        Duration::from_secs(35),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    endpoint.assert_exchange_count(1).await;
}

#[tokio::test]
async fn jms_consumer_receives_from_activemq() {
    init_tracing();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(shared_jms_activemq().await)
        .build()
        .await;

    let consumer_route = RouteBuilder::from("jms:queue:test-consume-activemq")
        .to("mock:consumed")
        .route_id("jms-consumer-activemq")
        .build()
        .unwrap();

    let inject_route =
        RouteBuilder::from("timer:inject-activemq?period=300&delay=500&repeatCount=1")
            .set_body("consume-from-activemq".to_string())
            .to("jms:queue:test-consume-activemq")
            .route_id("jms-consumer-activemq-inject")
            .build()
            .unwrap();

    h.add_route(consumer_route).await.unwrap();
    h.add_route(inject_route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("consumed").unwrap();
    wait_until(
        "jms consumer receives message from activemq",
        Duration::from_secs(35),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    let exchanges = endpoint.get_received_exchanges().await;
    assert!(!exchanges.is_empty());
    assert_eq!(
        exchanges[0].input.body.as_text(),
        Some("consume-from-activemq"),
        "Body should survive the ActiveMQ round-trip intact"
    );
}

#[tokio::test]
async fn jms_consumer_receives_from_artemis() {
    init_tracing();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(shared_jms_artemis().await)
        .build()
        .await;

    let consumer_route = RouteBuilder::from("jms:queue:test-consume-artemis")
        .to("mock:consumed")
        .route_id("jms-consumer-artemis")
        .build()
        .unwrap();

    let inject_route =
        RouteBuilder::from("timer:inject-artemis?period=300&delay=500&repeatCount=1")
            .set_body("consume-from-artemis".to_string())
            .to("jms:queue:test-consume-artemis")
            .route_id("jms-consumer-artemis-inject")
            .build()
            .unwrap();

    h.add_route(consumer_route).await.unwrap();
    h.add_route(inject_route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("consumed").unwrap();
    wait_until(
        "jms consumer receives message from artemis",
        Duration::from_secs(35),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    let exchanges = endpoint.get_received_exchanges().await;
    assert!(!exchanges.is_empty());
    assert_eq!(
        exchanges[0].input.body.as_text(),
        Some("consume-from-artemis"),
        "Body should survive the Artemis round-trip intact"
    );
}

#[tokio::test]
async fn jms_producer_sends_to_artemis() {
    init_tracing();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(shared_jms_artemis().await)
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body("hello-artemis".to_string())
        .to("jms:queue:test-produce-artemis")
        .to("mock:sent")
        .route_id("jms-producer-artemis-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("sent").unwrap();
    wait_until(
        "jms producer route delivery (artemis)",
        Duration::from_secs(35),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    endpoint.assert_exchange_count(1).await;
}

#[tokio::test]
async fn jms_headers_propagated() {
    init_tracing();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(shared_jms_activemq().await)
        .build()
        .await;

    let consumer_route = RouteBuilder::from("jms:queue:test-headers")
        .to("mock:headers")
        .route_id("jms-headers-consumer")
        .build()
        .unwrap();

    let producer_route =
        RouteBuilder::from("timer:headers-inject?period=300&delay=500&repeatCount=1")
            .set_body("header-check".to_string())
            .set_header("x-custom-header", Value::String("custom-value".to_string()))
            .to("jms:queue:test-headers")
            .route_id("jms-headers-producer")
            .build()
            .unwrap();

    h.add_route(consumer_route).await.unwrap();
    h.add_route(producer_route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("headers").unwrap();
    wait_until(
        "jms headers consumer receives message",
        Duration::from_secs(35),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;

    let exchanges = endpoint.get_received_exchanges().await;
    assert!(!exchanges.is_empty(), "Expected at least one exchange");
    let received = &exchanges[0];
    assert_eq!(
        received.input.body.as_text(),
        Some("header-check"),
        "Body should survive the round-trip intact"
    );
    assert_eq!(
        received
            .input
            .header("x-custom-header")
            .and_then(|v| v.as_str()),
        Some("custom-value")
    );
}

#[tokio::test]
async fn jms_producer_fails_gracefully_when_broker_unavailable() {
    init_tracing();

    let cfg = JmsConfig {
        broker_url: "tcp://127.0.0.1:1".to_string(),
        broker_type: BrokerType::ActiveMq,
        username: Some("admin".to_string()),
        password: Some("admin".to_string()),
        bridge_start_timeout_ms: 500,
        ..JmsConfig::default()
    };

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(JmsComponent::new(cfg))
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body("this-should-fail".to_string())
        .to("jms:queue:missing-broker")
        .to("mock:success")
        .route_id("jms-unavailable-broker")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("success").unwrap();
    let wait_result = wait_until(
        "unexpected jms success delivery",
        Duration::from_secs(2),
        Duration::from_millis(100),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await;

    h.stop().await;
    assert!(
        wait_result.is_err(),
        "No exchange should reach mock:success when broker is unavailable"
    );
}

#[tokio::test]
async fn jms_producer_sends_multiple_messages() {
    init_tracing();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(shared_jms_activemq().await)
        .build()
        .await;

    // Verifies that the JMS bridge can deliver multiple sequential messages
    // through a producer+consumer round-trip on the same broker.
    let consumer_route = RouteBuilder::from("jms:queue:test-multi")
        .to("mock:consumed")
        .route_id("jms-multi-consumer")
        .build()
        .unwrap();

    let producer_route =
        RouteBuilder::from("timer:multi-inject?period=300&delay=500&repeatCount=2")
            .set_body("multi-msg".to_string())
            .to("jms:queue:test-multi")
            .route_id("jms-multi-producer")
            .build()
            .unwrap();

    h.add_route(consumer_route).await.unwrap();
    h.add_route(producer_route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("consumed").unwrap();
    wait_until(
        "jms multi-message consumer receives two messages",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(endpoint.get_received_exchanges().await.len() >= 2) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    let exchanges = endpoint.get_received_exchanges().await;
    assert!(exchanges.len() >= 2);
    for ex in &exchanges {
        assert_eq!(
            ex.input.body.as_text(),
            Some("multi-msg"),
            "Each message body should survive the round-trip intact"
        );
    }
}

/// Regression test for: bridge crashes ~7s after start when Artemis uses mandatory auth.
///
/// Root cause: `cached_channel_healthy` used a 1s timeout, but the Artemis health check
/// (creating a bare Netty connection in GraalVM native) takes >1s → health check fails →
/// Rust kills and attempts to restart the bridge → subscribers lose their gRPC connection.
///
/// This test runs two concurrent consumers and a producer that sends messages every second
/// for 15 seconds (well past the ~7s crash window). If the bridge restarts mid-test,
/// messages will be lost and the assertion will fail.
#[tokio::test]
async fn jms_artemis_bridge_stable_under_mandatory_auth() {
    init_tracing();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(shared_jms_artemis_auth().await)
        .build()
        .await;

    // Two competing consumers on the same queue — if the bridge restarts, both
    // lose their gRPC streams simultaneously, making the gap obvious.
    let consumer_a = RouteBuilder::from("jms:queue:test-auth-stability")
        .to("mock:stable")
        .route_id("jms-auth-stable-consumer-a")
        .build()
        .unwrap();

    let consumer_b = RouteBuilder::from("jms:queue:test-auth-stability")
        .to("mock:stable")
        .route_id("jms-auth-stable-consumer-b")
        .build()
        .unwrap();

    // Send 1 message/s for 10 iterations starting at 2s (well past the 7s crash).
    let producer = RouteBuilder::from("timer:auth-inject?period=1000&delay=2000&repeatCount=10")
        .set_body("auth-stable-msg".to_string())
        .to("jms:queue:test-auth-stability")
        .route_id("jms-auth-stable-producer")
        .build()
        .unwrap();

    h.add_route(consumer_a).await.unwrap();
    h.add_route(consumer_b).await.unwrap();
    h.add_route(producer).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("stable").unwrap();

    // We expect all 10 messages to be consumed. Allow 30s: bridge is already
    // healthy at this point (h.start() waited), timer fires at +2s, last msg
    // at +12s, plus processing overhead.
    wait_until(
        "jms bridge stable under mandatory auth: 10 messages delivered",
        Duration::from_secs(30),
        Duration::from_millis(500),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(endpoint.get_received_exchanges().await.len() >= 10) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    let exchanges = endpoint.get_received_exchanges().await;
    assert!(
        exchanges.len() >= 10,
        "Expected at least 10 messages; bridge may have restarted under mandatory auth"
    );
    for ex in &exchanges {
        assert_eq!(
            ex.input.body.as_text(),
            Some("auth-stable-msg"),
            "Each message body should survive the Artemis auth round-trip intact"
        );
    }
}
