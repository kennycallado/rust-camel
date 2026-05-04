mod support;

use std::time::Duration;

use camel_component_api::{Body, Exchange, Message, Value};
use camel_component_cxf::config::CxfServiceConfig;
use camel_component_cxf::producer::CxfProducer;
use support::mock_bridge::{MockState, spawn_mock_bridge};
use tonic::transport::{Channel, Endpoint};
use tower::Service;

async fn make_producer() -> (CxfProducer, MockState) {
    let (port, state) = spawn_mock_bridge().await.expect("mock bridge");
    let channel = Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("endpoint")
        .connect()
        .await
        .expect("connect");

    let config = CxfServiceConfig {
        address: Some("http://localhost:8080/ws".to_string()),
        wsdl_path: "service.wsdl".to_string(),
        service_name: "TestService".to_string(),
        port_name: "TestPort".to_string(),
        security: Default::default(),
    };

    let producer = CxfProducer::from_channel(channel, config, "defaultOperation".to_string());
    (producer, state)
}

#[tokio::test]
async fn test_invoke_success_sets_response_body() {
    let (mut producer, state) = make_producer().await;
    *state.invoke_response_payload.lock().await = Some(b"ok-response".to_vec());
    let exchange = Exchange::new(Message::new(Body::Text("request".to_string())));

    let out = producer.call(exchange).await.expect("success");
    assert_eq!(
        out.input.body,
        Body::Bytes(bytes::Bytes::from_static(b"ok-response"))
    );
}

#[tokio::test]
async fn test_invoke_fault_returns_error() {
    let (mut producer, state) = make_producer().await;
    *state.invoke_fault.lock().await = Some(("Server".to_string(), "Boom".to_string()));
    let exchange = Exchange::new(Message::new(Body::Text("request".to_string())));

    let err = producer.call(exchange).await.expect_err("fault error");
    let msg = err.to_string();
    assert!(msg.contains("fault"));
    assert!(msg.contains("Server"));
    assert!(msg.contains("Boom"));
}

#[tokio::test]
async fn test_invoke_timeout_returns_transport_error() {
    let channel: Channel = Endpoint::from_static("http://127.0.0.1:9")
        .connect_timeout(Duration::from_millis(100))
        .timeout(Duration::from_millis(100))
        .connect_lazy();
    let config = CxfServiceConfig {
        address: Some("http://localhost:8080/ws".to_string()),
        wsdl_path: "service.wsdl".to_string(),
        service_name: "TestService".to_string(),
        port_name: "TestPort".to_string(),
        security: Default::default(),
    };
    let mut producer = CxfProducer::from_channel(channel, config, "defaultOperation".to_string());

    let exchange = Exchange::new(Message::new(Body::Text("request".to_string())));
    let err = producer.call(exchange).await.expect_err("transport error");
    assert!(err.to_string().contains("CXF gRPC invoke"));
}

#[tokio::test]
async fn test_operation_from_header_overrides_default() {
    let (mut producer, state) = make_producer().await;
    let mut msg = Message::new(Body::Text("request".to_string()));
    msg.set_header(
        "CamelCxfOperation",
        Value::String("headerOperation".to_string()),
    );
    let exchange = Exchange::new(msg);

    let _ = producer.call(exchange).await.expect("success");
    let req = state.last_invoke.lock().await.clone().expect("last invoke");
    assert_eq!(req.operation, "headerOperation");
}

#[tokio::test]
async fn test_operation_from_uri_when_no_header() {
    let (mut producer, state) = make_producer().await;
    let exchange = Exchange::new(Message::new(Body::Text("request".to_string())));

    let _ = producer.call(exchange).await.expect("success");
    let req = state.last_invoke.lock().await.clone().expect("last invoke");
    assert_eq!(req.operation, "defaultOperation");
}

#[tokio::test]
async fn test_body_conversion_text_to_bytes() {
    let (mut producer, state) = make_producer().await;
    let exchange = Exchange::new(Message::new(Body::Text("hello".to_string())));

    let _ = producer.call(exchange).await.expect("success");
    let req = state.last_invoke.lock().await.clone().expect("last invoke");
    assert_eq!(req.payload, b"hello");
}

#[tokio::test]
async fn test_body_conversion_xml_to_bytes() {
    let (mut producer, state) = make_producer().await;
    let exchange = Exchange::new(Message::new(Body::Xml("<x/>".to_string())));

    let _ = producer.call(exchange).await.expect("success");
    let req = state.last_invoke.lock().await.clone().expect("last invoke");
    assert_eq!(req.payload, b"<x/>");
}

#[tokio::test]
async fn test_body_conversion_bytes_passthrough() {
    let (mut producer, state) = make_producer().await;
    let exchange = Exchange::new(Message::new(Body::Bytes(bytes::Bytes::from_static(b"raw"))));

    let _ = producer.call(exchange).await.expect("success");
    let req = state.last_invoke.lock().await.clone().expect("last invoke");
    assert_eq!(req.payload, b"raw");
}

#[tokio::test]
async fn test_body_empty_sends_zero_length_payload() {
    let (mut producer, state) = make_producer().await;
    let exchange = Exchange::new(Message::new(Body::Empty));

    let _ = producer.call(exchange).await.expect("success");
    let req = state.last_invoke.lock().await.clone().expect("last invoke");
    assert!(req.payload.is_empty());
}

#[tokio::test]
async fn test_invoke_bridge_returns_soap_fault() {
    let (mut producer, state) = make_producer().await;
    *state.invoke_fault.lock().await =
        Some(("soap:Client".to_string(), "Invalid request".to_string()));
    let exchange = Exchange::new(Message::new(Body::Text("<bad/>".to_string())));
    let err = producer.call(exchange).await.expect_err("fault");
    let msg = err.to_string();
    assert!(msg.contains("fault"));
    assert!(msg.contains("soap:Client"));
    assert!(msg.contains("Invalid request"));
}

#[tokio::test]
async fn test_invoke_sends_address_in_request() {
    let (mut producer, state) = make_producer().await;
    let exchange = Exchange::new(Message::new(Body::Text("req".to_string())));
    let _ = producer.call(exchange).await;
    let req = state.last_invoke.lock().await.clone().expect("last invoke");
    assert!(req.address.contains("localhost:8080/ws"));
}

#[tokio::test]
async fn test_invoke_with_custom_timeout_forwarded() {
    let (mut producer, state) = make_producer().await;
    let mut msg = Message::new(Body::Text("req".to_string()));
    msg.set_header("CamelCxfTimeoutMs", Value::String("5000".to_string()));
    let exchange = Exchange::new(msg);
    let _ = producer.call(exchange).await;
    let req = state.last_invoke.lock().await.clone().expect("last invoke");
    assert_eq!(req.timeout_ms, 5000);
}

#[tokio::test]
async fn test_multiple_sequential_invocations() {
    let (mut producer, state) = make_producer().await;
    *state.invoke_response_payload.lock().await = Some(b"response".to_vec());
    for i in 0..5 {
        let exchange = Exchange::new(Message::new(Body::Text(format!("request-{i}"))));
        let out = producer.call(exchange).await.expect("success");
        assert_eq!(
            out.input.body,
            Body::Bytes(bytes::Bytes::from_static(b"response"))
        );
    }
}

#[tokio::test]
async fn test_invoke_json_body_converted_to_bytes() {
    let (mut producer, state) = make_producer().await;
    let json_val = serde_json::json!({"key": "value"});
    let exchange = Exchange::new(Message::new(Body::Json(json_val)));
    let _ = producer.call(exchange).await;
    let req = state.last_invoke.lock().await.clone().expect("last invoke");
    let parsed: serde_json::Value = serde_json::from_slice(&req.payload).unwrap();
    assert_eq!(parsed["key"], "value");
}
