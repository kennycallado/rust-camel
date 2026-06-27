//! Integration tests for MqttComponent, MqttEndpoint, and MqttBundle.
//!
//! These tests use only the PUBLIC API surface (no `#[cfg(test)]` internals)
//! to verify that the component, endpoint, and bundle types wire together
//! correctly.

use camel_component_api::{Component, NoOpComponentContext};
use camel_component_mqtt::MqttComponent;

// ---------------------------------------------------------------------------
// Component-level tests
// ---------------------------------------------------------------------------

#[test]
fn mqtt_component_scheme() {
    let comp = MqttComponent::new();
    assert_eq!(comp.scheme(), "mqtt");
}

#[test]
fn create_endpoint_fails_without_broker() {
    let comp = MqttComponent::new();
    let ctx = NoOpComponentContext;
    let result = comp.create_endpoint("mqtt://nonexistent/sensors/temp", &ctx);
    assert!(result.is_err());
}

#[test]
fn create_endpoint_succeeds_with_broker() {
    use camel_component_mqtt::config::{MqttBrokerConfig, MqttConfig};

    let config = MqttConfig {
        brokers: [(
            "local".to_string(),
            MqttBrokerConfig {
                url: "mqtt://localhost:1883".to_string(),
                username: None,
                password: None,
                tls_ca_cert: None,
            },
        )]
        .into_iter()
        .collect(),
        ..Default::default()
    };
    let comp = MqttComponent::with_config(config).unwrap();
    let ctx = NoOpComponentContext;
    let result = comp.create_endpoint("mqtt://local/sensors/temp", &ctx);
    assert!(result.is_ok());
}

#[test]
fn endpoint_uri_roundtrip() {
    use camel_component_mqtt::config::{MqttBrokerConfig, MqttConfig};

    let config = MqttConfig {
        brokers: [(
            "local".to_string(),
            MqttBrokerConfig {
                url: "mqtt://localhost:1883".to_string(),
                username: None,
                password: None,
                tls_ca_cert: None,
            },
        )]
        .into_iter()
        .collect(),
        ..Default::default()
    };
    let comp = MqttComponent::with_config(config).unwrap();
    let ctx = NoOpComponentContext;
    let ep = comp
        .create_endpoint("mqtt://local/sensors/temp", &ctx)
        .unwrap();
    assert_eq!(ep.uri(), "mqtt://local/sensors/temp");
}
