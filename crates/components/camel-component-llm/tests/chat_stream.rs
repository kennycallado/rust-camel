//! Integration tests for LLM chat streaming.
//!
//! These tests exercise the full LlmComponent → Endpoint → Producer
//! pipeline with a mock provider, verifying that streaming responses
//! produce the expected body type and header values.

use std::sync::Arc;

use camel_api::{Body, Exchange, Message};
use camel_component_api::{
    Component, NoOpComponentContext, NoopRuntimeObservability, RuntimeObservability,
};
use camel_component_llm::headers::*;
use tower::Service;

mod common;

#[tokio::test]
async fn chat_streaming_produces_stream_body() {
    let component = common::make_test_component();
    let endpoint = component
        .create_endpoint("llm:chat?provider=test&stream=true", &NoOpComponentContext)
        .expect("endpoint");

    let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);

    let mut producer = endpoint
        .create_producer(rt, &common::make_producer_ctx())
        .expect("producer");

    let exchange = Exchange::new(Message::new(Body::Text("hi".into())));
    let result = producer.call(exchange).await.expect("producer ok");

    // Streaming mode → body is Stream
    assert!(matches!(result.input.body, Body::Stream(_)));

    // Headers set by set_start_headers
    assert_eq!(
        result
            .input
            .headers
            .get(CAMEL_LLM_PROVIDER)
            .and_then(|v| v.as_str()),
        Some("test")
    );
    assert_eq!(
        result
            .input
            .headers
            .get(CAMEL_LLM_STREAM)
            .and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        result
            .input
            .headers
            .get(CAMEL_LLM_USAGE_AVAILABLE)
            .and_then(|v| v.as_bool()),
        Some(false)
    );

    // Token headers should NOT be present in streaming mode
    assert!(!result.input.headers.contains_key(CAMEL_LLM_TOKENS_IN));
    assert!(!result.input.headers.contains_key(CAMEL_LLM_TOKENS_OUT));
}
