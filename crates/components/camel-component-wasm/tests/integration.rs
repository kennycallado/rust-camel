use camel_component_api::test_support::PanicRuntimeObservability;
fn test_rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
    std::sync::Arc::new(PanicRuntimeObservability)
}
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use camel_api::{Body, CamelError, Exchange, ExchangePattern, Message, ProducerContext, Value};
use camel_component_api::{
    Component, ComponentBundle, ComponentRegistrar, Endpoint, NoOpComponentContext,
};
use camel_component_wasm::WasmComponent;
use camel_component_wasm::WasmConfig;
use camel_component_wasm::bindings::camel::plugin::types::WasmBody;
use camel_component_wasm::bundle::WasmBundle;
use camel_component_wasm::endpoint::WasmEndpoint;
use camel_component_wasm::producer::WasmProducer;
use camel_component_wasm::serde_bridge::{exchange_to_wasm, wasm_to_exchange};
use camel_core::Registry;
use serde_json::json;
use tempfile::tempdir;
use tower::ServiceExt;

// TODO(WASM-010): Integration tests require a compiled WASM fixture not in repo.
// Run: cargo build --target wasm32-wasip2 -p <fixture-crate> first.

fn make_registry() -> Arc<Mutex<Registry>> {
    Arc::new(Mutex::new(Registry::new()))
}

struct TestRegistrar {
    schemes: Vec<String>,
}

impl ComponentRegistrar for TestRegistrar {
    fn register_component_dyn(&mut self, component: Arc<dyn Component>) {
        self.schemes.push(component.scheme().to_string());
    }
}

#[tokio::test]
async fn wasm_component_rejects_absolute_paths() {
    let base = tempdir().unwrap();
    let component = WasmComponent::new(make_registry(), base.path().to_path_buf());
    let result = component.create_endpoint("wasm:/tmp/plugin.wasm", &NoOpComponentContext);
    assert!(matches!(result, Err(CamelError::InvalidUri(msg)) if msg.contains("relative")));
}

#[tokio::test]
async fn wasm_component_rejects_parent_dir_paths() {
    let base = tempdir().unwrap();
    let component = WasmComponent::new(make_registry(), base.path().to_path_buf());
    let result = component.create_endpoint("wasm:../plugin.wasm", &NoOpComponentContext);
    assert!(
        matches!(result, Err(CamelError::InvalidUri(msg)) if msg.contains("must not contain '..'"))
    );
}

#[tokio::test]
async fn wasm_component_rejects_nonexistent_paths() {
    let base = tempdir().unwrap();
    let component = WasmComponent::new(make_registry(), base.path().to_path_buf());
    let result = component.create_endpoint("wasm:missing/plugin.wasm", &NoOpComponentContext);
    assert!(
        matches!(result, Err(CamelError::ComponentNotFound(msg)) if msg.contains("WASM module not found"))
    );
}

#[tokio::test]
async fn wasm_component_rejects_symlink_escape_paths() {
    let base = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let outside_file = outside.path().join("plugin.wasm");
    std::fs::write(&outside_file, b"x").unwrap();
    let escape = base.path().join("escape.wasm");
    std::os::unix::fs::symlink(&outside_file, &escape).unwrap();
    let component = WasmComponent::new(make_registry(), base.path().to_path_buf());
    let result = component.create_endpoint("wasm:escape.wasm", &NoOpComponentContext);
    assert!(
        matches!(result, Err(CamelError::InvalidUri(msg)) if msg.contains("escapes project root"))
    );
}

#[tokio::test]
async fn wasm_component_accepts_valid_relative_path() {
    let base = tempdir().unwrap();
    let module = base.path().join("plugins").join("ok.wasm");
    std::fs::create_dir_all(module.parent().unwrap()).unwrap();
    std::fs::write(&module, b"not-real-wasm").unwrap();
    let component = WasmComponent::new(make_registry(), base.path().to_path_buf());
    let endpoint = component
        .create_endpoint("wasm:plugins/ok.wasm", &NoOpComponentContext)
        .unwrap();
    assert_eq!(endpoint.uri(), "wasm:plugins/ok.wasm");
}

#[test]
fn wasm_bundle_registers_wasm_scheme_in_integration() {
    let mut registrar = TestRegistrar { schemes: vec![] };
    let bundle = WasmBundle::new(make_registry(), PathBuf::from("."));
    bundle.register_all(&mut registrar);
    assert_eq!(registrar.schemes, vec!["wasm"]);
}

#[test]
fn wasm_bundle_from_toml_returns_error_in_integration() {
    let value: toml::Value = toml::from_str("").unwrap();
    let result = WasmBundle::from_toml(value);
    assert!(
        matches!(result, Err(CamelError::Config(msg)) if msg.contains("WasmBundle requires registry and base_dir"))
    );
}

#[tokio::test]
async fn wasm_producer_constructor_creates_instance() {
    let producer = WasmProducer::new(
        PathBuf::from("missing-module.wasm"),
        make_registry(),
        WasmConfig::default(),
        test_rt(),
    );
    let exchange = Exchange::new(Message::new("hello"));
    let result = producer.clone().oneshot(exchange).await;
    assert!(matches!(result, Err(CamelError::ComponentNotFound(_))));
}

#[tokio::test]
async fn wasm_producer_returns_component_not_found_for_invalid_module_file() {
    let base = tempdir().unwrap();
    let invalid = base.path().join("invalid.wasm");
    std::fs::write(&invalid, b"this-is-not-a-wasm-component").unwrap();
    let producer = WasmProducer::new(invalid, make_registry(), WasmConfig::default(), test_rt());
    let exchange = Exchange::new(Message::new("hello"));
    let result = producer.clone().oneshot(exchange).await;
    assert!(
        matches!(result, Err(CamelError::ComponentNotFound(msg)) if msg.contains("Failed to load WASM module"))
    );
}

#[test]
fn wasm_endpoint_creation_flow_uri_and_consumer_producer_behavior() {
    let endpoint = WasmEndpoint::new(
        "wasm:plugins/worker.wasm".to_string(),
        PathBuf::from("plugins/worker.wasm"),
        make_registry(),
        WasmConfig::default(),
    );
    assert_eq!(endpoint.uri(), "wasm:plugins/worker.wasm");
    let rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability> =
        std::sync::Arc::new(camel_component_api::NoOpComponentContext);
    let consumer = endpoint.create_consumer(rt);
    assert!(
        consumer.is_ok(),
        "create_consumer should succeed for source world"
    );
    let rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability> =
        std::sync::Arc::new(camel_component_api::NoOpComponentContext);
    let producer = endpoint.create_producer(rt, &ProducerContext::new());
    assert!(producer.is_ok());
}

#[test]
fn serde_bridge_round_trip_empty_exchange() {
    let exchange = Exchange::new(Message::default());
    let wasm = exchange_to_wasm(&exchange).expect("exchange_to_wasm should succeed");
    let mut out = Exchange::new(Message::new("seed"));
    wasm_to_exchange(wasm, &mut out);
    assert!(matches!(out.input.body, Body::Empty));
    assert!(out.output.is_none());
    assert!(out.properties.is_empty());
}

#[test]
fn serde_bridge_round_trip_all_body_types() {
    let variants = vec![
        Body::Empty,
        Body::Text("text".to_string()),
        Body::Bytes(Bytes::from_static(b"bytes")),
        Body::Json(json!({"k":"v"})),
        Body::Xml("<x/>".to_string()),
    ];
    for body in variants {
        let exchange = Exchange::new(Message::new(body));
        let wasm = exchange_to_wasm(&exchange).expect("exchange_to_wasm should succeed");
        let mut out = Exchange::new(Message::default());
        wasm_to_exchange(wasm, &mut out);
        match (&exchange.input.body, &out.input.body) {
            (Body::Empty, Body::Empty)
            | (Body::Text(_), Body::Text(_))
            | (Body::Bytes(_), Body::Bytes(_))
            | (Body::Json(_), Body::Json(_))
            | (Body::Xml(_), Body::Xml(_)) => {}
            _ => panic!("body variant mismatch"),
        }
    }
}

#[test]
fn serde_bridge_round_trip_complex_properties_and_headers() {
    let mut input = Message::new(Body::Json(json!({"payload":[1,2,3]})));
    input.set_header("s", Value::String("abc".to_string()));
    input.set_header("n", json!(42));
    input.set_header("b", Value::Bool(true));
    input.set_header("o", json!({"nested":{"x":1}}));
    input.set_header("a", json!([1, {"y":2}, false]));

    let mut exchange = Exchange::new(input);
    exchange.pattern = ExchangePattern::InOut;
    exchange.output = Some(Message::new(Body::Text("out".to_string())));
    exchange.properties = HashMap::from([
        ("p_string".to_string(), Value::String("v".to_string())),
        ("p_num".to_string(), json!(99)),
        ("p_bool".to_string(), Value::Bool(false)),
        ("p_obj".to_string(), json!({"k":"v","n":[1,2]})),
        ("p_arr".to_string(), json!([{"z":1}, null, true])),
    ]);
    let original_correlation_id = exchange.correlation_id.clone();

    let wasm = exchange_to_wasm(&exchange).expect("exchange_to_wasm should succeed");
    let mut out = Exchange::new(Message::default());
    wasm_to_exchange(wasm, &mut out);

    assert!(matches!(out.input.body, Body::Json(_)));
    assert_eq!(
        out.input.headers.get("s"),
        Some(&Value::String("abc".to_string()))
    );
    assert_eq!(out.input.headers.get("n"), Some(&json!(42)));
    assert_eq!(out.input.headers.get("b"), Some(&Value::Bool(true)));
    assert_eq!(out.input.headers.get("o"), Some(&json!({"nested":{"x":1}})));
    assert_eq!(
        out.input.headers.get("a"),
        Some(&json!([1, {"y":2}, false]))
    );
    assert_eq!(
        out.properties.get("p_string"),
        Some(&Value::String("v".to_string()))
    );
    assert_eq!(out.properties.get("p_num"), Some(&json!(99)));
    assert_eq!(out.properties.get("p_bool"), Some(&Value::Bool(false)));
    assert_eq!(
        out.properties.get("p_obj"),
        Some(&json!({"k":"v","n":[1,2]}))
    );
    assert_eq!(
        out.properties.get("p_arr"),
        Some(&json!([{"z":1}, null, true]))
    );
    assert_eq!(out.pattern, ExchangePattern::InOut);
    assert_eq!(out.output.unwrap().body.as_text(), Some("out"));
    assert_ne!(original_correlation_id, String::new());
}

#[test]
fn serde_bridge_json_fallback_to_text_on_invalid_json_string() {
    let wasm = camel_component_wasm::bindings::camel::plugin::types::WasmExchange {
        input: camel_component_wasm::bindings::camel::plugin::types::WasmMessage {
            headers: vec![],
            body: WasmBody::Json("{not-valid-json}".to_string()),
        },
        output: None,
        properties: vec![],
        pattern: camel_component_wasm::bindings::camel::plugin::types::WasmPattern::InOnly,
        correlation_id: "cid".to_string(),
        route_id: None,
        message_id: None,
    };
    let mut out = Exchange::new(Message::default());
    wasm_to_exchange(wasm, &mut out);
    assert_eq!(out.input.body.as_text(), Some("{not-valid-json}"));
}

// TODO(WASM-010): requires compiled WASM fixture — see tests/fixtures/README.md
#[tokio::test]
#[ignore]
async fn wasm_integration_with_compiled_fixture() {
    // This test requires a compiled .wasm module placed in tests/fixtures/.
    // Run: cargo build --target wasm32-wasip2 -p <fixture-crate>
    // Then point the URI at the compiled artifact.
    let _ = "placeholder: see tests/fixtures/README.md for setup instructions";
}
