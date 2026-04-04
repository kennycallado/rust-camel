//! Integration tests for the JMS component.
//!
//! **Requires Docker to be running.**
//! **Requires `integration-tests` feature:** `cargo test -p camel-test --features integration-tests`
//! **Requires the JMS bridge binary.** Set `CAMEL_JMS_BRIDGE_RELEASE_URL` or pre-build.

#![cfg(feature = "integration-tests")]

mod support;

use std::time::Duration;

use camel_api::Value;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_jms::{BrokerType, JmsComponent, JmsConfig};
use camel_test::CamelTestContext;
use support::activemq::shared_activemq;
use support::artemis::shared_artemis;
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

fn test_jms_config(broker_url: &str, broker_type: BrokerType) -> JmsConfig {
    JmsConfig {
        broker_url: broker_url.to_string(),
        broker_type,
        bridge_start_timeout_ms: 30_000,
        ..JmsConfig::default()
    }
}

#[tokio::test]
async fn jms_producer_sends_to_activemq() {
    init_tracing();
    let (_, broker_url) = shared_activemq().await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(JmsComponent::new(test_jms_config(
            broker_url,
            BrokerType::ActiveMq,
        )))
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(Value::String("hello-jms".to_string()))
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
        Duration::from_secs(20),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(endpoint.get_received_exchanges().await.len() >= 1) }
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
    let (_, broker_url) = shared_activemq().await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(JmsComponent::new(test_jms_config(
            broker_url,
            BrokerType::ActiveMq,
        )))
        .build()
        .await;

    let consumer_route = RouteBuilder::from("jms:queue:test-consume-activemq")
        .to("mock:consumed")
        .route_id("jms-consumer-activemq")
        .build()
        .unwrap();

    let inject_route =
        RouteBuilder::from("timer:inject-activemq?period=1000&delay=1500&repeatCount=1")
            .set_body(Value::String("consume-from-activemq".to_string()))
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
        Duration::from_secs(20),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    assert!(!endpoint.get_received_exchanges().await.is_empty());
}

#[tokio::test]
async fn jms_consumer_receives_from_artemis() {
    init_tracing();
    let (_, broker_url) = shared_artemis().await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(JmsComponent::new(JmsConfig {
            username: Some("artemis".to_string()),
            password: Some("artemis".to_string()),
            ..test_jms_config(broker_url, BrokerType::Artemis)
        }))
        .build()
        .await;

    let consumer_route = RouteBuilder::from("jms:queue:test-consume-artemis")
        .to("mock:consumed")
        .route_id("jms-consumer-artemis")
        .build()
        .unwrap();

    let inject_route =
        RouteBuilder::from("timer:inject-artemis?period=1000&delay=1500&repeatCount=1")
            .set_body(Value::String("consume-from-artemis".to_string()))
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
        Duration::from_secs(20),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    assert!(!endpoint.get_received_exchanges().await.is_empty());
}

#[tokio::test]
async fn jms_headers_propagated() {
    init_tracing();
    let (_, broker_url) = shared_activemq().await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(JmsComponent::new(test_jms_config(
            broker_url,
            BrokerType::ActiveMq,
        )))
        .build()
        .await;

    let consumer_route = RouteBuilder::from("jms:queue:test-headers")
        .to("mock:headers")
        .route_id("jms-headers-consumer")
        .build()
        .unwrap();

    let producer_route =
        RouteBuilder::from("timer:headers-inject?period=1000&delay=1500&repeatCount=1")
            .set_body(Value::String("header-check".to_string()))
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
        Duration::from_secs(20),
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

    let mut cfg = test_jms_config("tcp://127.0.0.1:1", BrokerType::ActiveMq);
    cfg.bridge_start_timeout_ms = 500;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(JmsComponent::new(cfg))
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(Value::String("this-should-fail".to_string()))
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
    let (_, broker_url) = shared_activemq().await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(JmsComponent::new(test_jms_config(
            broker_url,
            BrokerType::ActiveMq,
        )))
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
        RouteBuilder::from("timer:multi-inject?period=1000&delay=1500&repeatCount=2")
            .set_body(Value::String("multi-msg".to_string()))
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
        Duration::from_secs(25),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(endpoint.get_received_exchanges().await.len() >= 2) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    assert!(endpoint.get_received_exchanges().await.len() >= 2);
}
