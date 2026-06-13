//! Integration tests for LLM chat materialized (non-streaming).
//!
//! These tests verify that non-streaming chat responses produce a materialized
//! body type (Text) with token usage headers, and that header overrides
//! correctly influence model selection.

use std::sync::Arc;

use camel_api::{Body, Exchange, Message, Value};
use camel_component_api::{
    Component, NoOpComponentContext, NoopRuntimeObservability, RuntimeObservability,
};
use camel_component_llm::headers::*;
use tower::Service;

mod common;

#[tokio::test]
async fn chat_materialized_has_token_headers() {
    let component = common::make_test_component();
    let endpoint = component
        .create_endpoint("llm:chat?provider=test&stream=false", &NoOpComponentContext)
        .expect("endpoint");

    let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);

    let mut producer = endpoint
        .create_producer(rt, &common::make_producer_ctx())
        .expect("producer");

    let exchange = Exchange::new(Message::new(Body::Text("hello".into())));
    let result = producer.call(exchange).await.expect("producer ok");

    // Non-streaming → body is Text
    assert!(matches!(result.input.body, Body::Text(_)));

    // Usage headers should be present
    assert_eq!(
        result
            .input
            .headers
            .get(CAMEL_LLM_USAGE_AVAILABLE)
            .and_then(|v| v.as_bool()),
        Some(true)
    );
    assert!(result.input.headers.contains_key(CAMEL_LLM_TOKENS_IN));
    assert!(result.input.headers.contains_key(CAMEL_LLM_TOKENS_OUT));
    assert!(result.input.headers.contains_key(CAMEL_LLM_FINISH_REASON));
}

#[tokio::test]
async fn header_override_changes_model() {
    let component = common::make_test_component();
    let endpoint = component
        .create_endpoint("llm:chat?provider=test&stream=false", &NoOpComponentContext)
        .expect("endpoint");

    let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);

    let mut producer = endpoint
        .create_producer(rt, &common::make_producer_ctx())
        .expect("producer");

    let mut exchange = Exchange::new(Message::new(Body::Text("hello".into())));
    exchange
        .input
        .headers
        .insert(CAMEL_LLM_MODEL.into(), Value::String("custom-model".into()));
    let result = producer.call(exchange).await.expect("producer ok");

    // Should still succeed and produce a text body
    assert!(matches!(result.input.body, Body::Text(_)));

    // Verify the model override was used
    assert_eq!(
        result
            .input
            .headers
            .get(CAMEL_LLM_MODEL)
            .and_then(|v| v.as_str()),
        Some("custom-model")
    );
}
