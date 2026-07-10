//! Integration tests for the MQTT component (rumqttc).
//!
//! Uses testcontainers to spin up an Eclipse Mosquitto broker.
//! **Requires Docker** and the `integration-tests` feature.

#![cfg(feature = "integration-tests")]

mod support;
use support::install_crypto_provider;

use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_mqtt::MqttComponent;
use camel_component_mqtt::config::{MqttBrokerConfig, MqttConfig};
use camel_component_mqtt::headers::CAMEL_MQTT_TOPIC;
use camel_test::CamelTestContext;
use support::mosquitto::shared_mosquitto;
use support::wait::wait_until;

fn mqtt_component_for(host_port: &str) -> MqttComponent {
    let config = MqttConfig {
        brokers: [(
            "test".to_string(),
            MqttBrokerConfig {
                url: format!("mqtt://{host_port}"),
                username: None,
                password: None,
                tls_ca_cert: None,
            },
        )]
        .into_iter()
        .collect(),
        ..Default::default()
    };
    MqttComponent::with_config(config).expect("mqtt config")
}

// ===========================================================================
// Producer test — timer → mqtt, assert the exchange reaches mock:result
// ===========================================================================
#[tokio::test(flavor = "multi_thread")]
async fn mqtt_producer_publishes_without_error() {
    install_crypto_provider();
    let (_container, host_port) = shared_mosquitto().await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(mqtt_component_for(host_port))
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(camel_api::Value::String("hello-mqtt".to_string()))
        .to("mqtt://test/sensors/temp")
        .to("mock:result")
        .route_id("mqtt-producer-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    wait_until(
        "mqtt producer route delivery",
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(100),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;

    // Check no errors reached the error endpoint
    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Producer route had errors: {:?}", errors[0].error);
        }
    }

    endpoint.assert_exchange_count(1).await;
}

// ===========================================================================
// Consumer test — consumer subscribes; producer publishes; consumer receives
// ===========================================================================
#[tokio::test(flavor = "multi_thread")]
async fn mqtt_consumer_receives_published_message() {
    install_crypto_provider();
    let (_container, host_port) = shared_mosquitto().await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(mqtt_component_for(host_port))
        .build()
        .await;

    // Consumer: subscribe to sensors/# (wildcard) → mock:consumed
    let consumer_route = RouteBuilder::from("mqtt://test/sensors/#")
        .to("mock:consumed")
        .route_id("mqtt-consumer-test")
        .build()
        .unwrap();

    // Producer: delayed timer so the consumer connects + subscribes first.
    let producer_route = RouteBuilder::from("timer:produce?period=3000&delay=3000&repeatCount=1")
        .set_body(camel_api::Value::String("hello-mqtt".to_string()))
        .to("mqtt://test/sensors/temp")
        .route_id("mqtt-producer-for-consumer-test")
        .build()
        .unwrap();

    h.add_route(consumer_route).await.unwrap();
    h.add_route(producer_route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("consumed").unwrap();
    wait_until(
        "mqtt consumer receives message",
        std::time::Duration::from_secs(20),
        std::time::Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;

    // The received exchange carries the MQTT topic header
    let ex = &endpoint.get_received_exchanges().await[0];
    assert_eq!(
        ex.input.header(CAMEL_MQTT_TOPIC),
        Some(&camel_api::Value::String("sensors/temp".to_string())),
    );
}
