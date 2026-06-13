//! Integration tests for LLM embed and error handling.
//!
//! These tests exercise the full LlmComponent → Endpoint → Producer
//! pipeline for the embed operation and various error scenarios.

use std::sync::Arc;

use camel_api::{Body, Exchange, Message};
use camel_component_api::{
    Component, NoOpComponentContext, NoopRuntimeObservability, RuntimeObservability,
};
use camel_component_llm::headers::*;
use tower::Service;

mod common;

#[tokio::test]
async fn embed_returns_json_vectors() {
    let component = common::make_test_component_with_response("echo");
    let endpoint = component
        .create_endpoint("llm:embed?provider=test", &NoOpComponentContext)
        .expect("endpoint");

    let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
    let mut producer = endpoint
        .create_producer(rt, &common::make_producer_ctx())
        .expect("producer");

    let exchange = Exchange::new(Message::new(Body::Text("hello world".into())));
    let result = producer.call(exchange).await.expect("producer ok");

    // Embed returns Body::Json with embedding vectors
    assert!(matches!(result.input.body, Body::Json(_)));

    // Usage is available for embed (non-streaming operation)
    assert_eq!(
        result
            .input
            .headers
            .get(CAMEL_LLM_USAGE_AVAILABLE)
            .and_then(|v| v.as_bool()),
        Some(true)
    );
}

#[tokio::test]
async fn embed_sets_model_header() {
    let component = common::make_test_component_with_response("echo");
    let endpoint = component
        .create_endpoint("llm:embed?provider=test", &NoOpComponentContext)
        .expect("endpoint");

    let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
    let mut producer = endpoint
        .create_producer(rt, &common::make_producer_ctx())
        .expect("producer");

    let exchange = Exchange::new(Message::new(Body::Text("hello".into())));
    let result = producer.call(exchange).await.expect("producer ok");

    // Model header should be set from the embed response
    assert_eq!(
        result
            .input
            .headers
            .get(CAMEL_LLM_MODEL)
            .and_then(|v| v.as_str()),
        Some("mock-model")
    );
}

#[tokio::test]
async fn unknown_operation_fails() {
    let component = common::make_test_component_with_response("echo");
    let result = component.create_endpoint("llm:invalid?provider=test", &NoOpComponentContext);
    assert!(result.is_err());
}

#[tokio::test]
async fn unknown_provider_fails_at_endpoint_creation() {
    let component = common::make_test_component_with_response("echo");
    let result = component.create_endpoint("llm:chat?provider=nonexistent", &NoOpComponentContext);
    assert!(result.is_err());
}

#[tokio::test]
async fn no_provider_fails() {
    let component = common::make_test_component_with_response("echo");
    let result = component.create_endpoint("llm:chat", &NoOpComponentContext);
    assert!(result.is_err());
}

#[tokio::test]
async fn empty_body_fails() {
    let component = common::make_test_component_with_response("echo");
    let endpoint = component
        .create_endpoint("llm:chat?provider=test&stream=false", &NoOpComponentContext)
        .expect("endpoint");

    let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
    let mut producer = endpoint
        .create_producer(rt, &common::make_producer_ctx())
        .expect("producer");

    let exchange = Exchange::new(Message::new(Body::Empty));
    let result = producer.call(exchange).await;
    assert!(result.is_err());
}
