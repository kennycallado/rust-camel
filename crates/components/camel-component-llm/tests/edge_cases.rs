//! Edge-case tests for the LLM component.
//!
//! Tests:
//! - Mid-stream error propagation
//! - Drop safety for unconsumed streams
//! - Input secret headers not echoed in output
//! - Provider error display truncation

use std::sync::Arc;

use camel_api::{Body, Exchange, Message, Value};
use camel_component_api::{
    Component, NoOpComponentContext, NoopRuntimeObservability, RuntimeObservability,
};
use camel_component_llm::error::LlmError;
use camel_component_llm::headers::*;
use futures::StreamExt;
use tower::Service;

mod common;

// ---------------------------------------------------------------------------
// Test 1: Mid-stream error propagation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mid_stream_error_propagates() {
    let component = common::make_test_component_with_error("simulated provider failure");
    let endpoint = component
        .create_endpoint("llm:chat?provider=err&stream=true", &NoOpComponentContext)
        .expect("endpoint");

    let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);

    let mut producer = endpoint
        .create_producer(rt, &common::make_producer_ctx())
        .expect("producer");

    let exchange = Exchange::new(Message::new(Body::Text("hello".into())));
    let result = producer
        .call(exchange)
        .await
        .expect("producer call should succeed");

    // Body must be Stream for streaming mode
    assert!(
        matches!(result.input.body, Body::Stream(_)),
        "expected Stream body"
    );

    // Consume the stream and verify error appears.
    // The MockMode::Error emits: Ok(Delta("partial")), then Err(error).
    // The producer maps the error to CamelError.
    if let Body::Stream(ref sb) = result.input.body {
        let mut guard = sb.stream.lock().await;
        if let Some(mut inner) = guard.take() {
            let mut found_error = false;

            while let Some(item) = inner.next().await {
                match item {
                    Ok(bytes) => {
                        // The Error mode sends one Delta with "partial" text.
                        assert_eq!(
                            bytes.as_ref(),
                            b"partial",
                            "expected partial content from error mode"
                        );
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        assert!(
                            msg.contains("provider error") || msg.contains("simulated"),
                            "error message should describe provider failure, got: {msg}"
                        );
                        found_error = true;
                    }
                }
            }

            assert!(found_error, "expected at least one error item in stream");
        } else {
            panic!("stream was already consumed before test could read it");
        }
    }
}

// ---------------------------------------------------------------------------
// Test 2: Drop safety for unconsumed streams
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stream_drop_does_not_panic() {
    let component = common::make_test_component();
    let endpoint = component
        .create_endpoint("llm:chat?provider=test&stream=true", &NoOpComponentContext)
        .expect("endpoint");

    let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);

    let mut producer = endpoint
        .create_producer(rt, &common::make_producer_ctx())
        .expect("producer");

    let exchange = Exchange::new(Message::new(Body::Text("hello".into())));
    let result = producer.call(exchange).await.expect("producer ok");

    // Verify it is a stream body
    assert!(matches!(result.input.body, Body::Stream(_)));

    // Drop the result without consuming the stream.
    // If this doesn't panic, the test passes.
    drop(result);
}

// ---------------------------------------------------------------------------
// Test 3: Producer-set headers don't leak secrets from input
// ---------------------------------------------------------------------------

#[tokio::test]
async fn producer_headers_never_contain_secrets() {
    let component = common::make_test_component();
    let endpoint = component
        .create_endpoint("llm:chat?provider=test&stream=false", &NoOpComponentContext)
        .expect("endpoint");

    let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);

    let mut producer = endpoint
        .create_producer(rt, &common::make_producer_ctx())
        .expect("producer");

    // Inject fake secrets into the INPUT exchange headers.
    // The Camel exchange model keeps input headers in place — we verify
    // that the producer does NOT accidentally copy secret-like values
    // into its own well-known output headers.
    let mut exchange = Exchange::new(Message::new(Body::Text("hello".into())));
    exchange.input.headers.insert(
        "Authorization".to_string(),
        Value::String("Bearer sk-test-secret-12345".to_string()),
    );
    exchange.input.headers.insert(
        "X-Api-Key".to_string(),
        Value::String("sk-another-secret".to_string()),
    );

    let result = producer.call(exchange).await.expect("producer ok");

    // Collect the known output headers the producer sets
    // (non-streaming chat with mock Fixed mode)
    let known_llm_headers: [&str; 7] = [
        CAMEL_LLM_PROVIDER,
        CAMEL_LLM_STREAM,
        CAMEL_LLM_USAGE_AVAILABLE,
        CAMEL_LLM_TOKENS_IN,
        CAMEL_LLM_TOKENS_OUT,
        CAMEL_LLM_FINISH_REASON,
        CAMEL_LLM_MODEL,
    ];

    // Every known header must be present (for non-streaming chat)
    for hdr in &known_llm_headers {
        assert!(
            result.input.headers.contains_key(*hdr),
            "expected header {hdr} to be set by producer"
        );
    }

    // None of the known output headers should contain secret-like values
    for hdr in &known_llm_headers {
        if let Some(val) = result.input.headers.get(*hdr) {
            let display = val.to_string();
            assert!(
                !display.contains("sk-"),
                "secret-like value in LLM header {hdr}: {display}"
            );
            assert!(
                !display.contains("secret"),
                "secret-like value in LLM header {hdr}: {display}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Test 4: Provider error display truncation
// ---------------------------------------------------------------------------

#[test]
fn provider_error_display_truncated() {
    let long = "x".repeat(20_000);
    let err = LlmError::provider(long);
    let display = err.to_string();
    assert!(
        display.contains("[truncated]"),
        "truncated marker should appear"
    );
    assert!(
        display.len() <= 240,
        "truncated display should be bounded, got {} bytes",
        display.len()
    );
}
