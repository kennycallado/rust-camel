//! Integration tests for camel-otel using in-memory OpenTelemetry exporters.
//!
//! These tests verify:
//! - Span hierarchy (parent-child relationships)
//! - Propagation roundtrip (trace ID preservation)
//! - TracingProcessor behavior
//! - No-crash guarantees when no provider is configured
//!
//! Note: Tests that use `global::set_tracer_provider` must be serialized
//! because OTel SDK's global state is shared across all tests.

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::{Mutex, MutexGuard};

use camel_api::exchange::Exchange;
use camel_api::message::Message;
use camel_api::{BoxProcessor, IdentityProcessor};
use camel_core::DetailLevel;
use camel_core::TracingProcessor;
use camel_otel::propagation::{
    extract_context, extract_into_exchange, inject_context, inject_from_exchange,
};
use opentelemetry::Context;
use opentelemetry::global;
use opentelemetry::trace::{
    SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState, Tracer, TracerProvider,
};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SimpleSpanProcessor};
use tower::{Service, ServiceExt};

/// Mutex to serialize tests that modify global OTel state.
/// Tests using `global::set_tracer_provider` must lock this mutex first.
static GLOBAL_TRACER_LOCK: Mutex<()> = Mutex::new(());

/// Acquire the global tracer lock for the duration of the test.
fn lock_global_tracer() -> MutexGuard<'static, ()> {
    GLOBAL_TRACER_LOCK.lock().unwrap()
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Set up an in-memory tracer provider for testing.
/// Returns the provider and exporter for verification.
fn setup_test_tracer() -> (SdkTracerProvider, InMemorySpanExporter) {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_span_processor(SimpleSpanProcessor::new(exporter.clone()))
        .build();
    (provider, exporter)
}

/// Create a valid traceparent header value for testing.
fn make_traceparent(trace_id_hex: &str, span_id_hex: &str, sampled: bool) -> String {
    let flags = if sampled { "01" } else { "00" };
    format!("00-{}-{}-{}", trace_id_hex, span_id_hex, flags)
}

// ---------------------------------------------------------------------------
// Test 1: Propagation roundtrip preserves trace ID
// ---------------------------------------------------------------------------

#[test]
fn test_propagation_roundtrip_preserves_trace_id() {
    // Create a context with a valid span using with_remote_span_context
    let trace_id_hex = "4bf92f3577b34da6a3ce929d0e0e4736";
    let span_id_hex = "00f067aa0ba902b7";

    let trace_id = TraceId::from_hex(trace_id_hex).unwrap();
    let span_id = SpanId::from_hex(span_id_hex).unwrap();

    let span_context = SpanContext::new(
        trace_id,
        span_id,
        TraceFlags::SAMPLED,
        true, // is_remote
        TraceState::default(),
    );

    // Create context with remote span context
    let cx = Context::new().with_remote_span_context(span_context);

    // Create an exchange with this context
    let mut exchange = Exchange::new(Message::new("test"));
    exchange.otel_context = cx;

    // Inject from exchange into headers
    let mut headers = HashMap::new();
    inject_from_exchange(&exchange, &mut headers);

    // Verify traceparent was injected
    assert!(
        headers.contains_key("traceparent"),
        "Should have traceparent header"
    );

    // Create a new exchange and extract into it
    let mut new_exchange = Exchange::new(Message::new("test"));
    extract_into_exchange(&mut new_exchange, &headers);

    // Verify the trace ID is preserved in the new exchange's otel_context
    let extracted_span = new_exchange.otel_context.span();
    let extracted_span_context = extracted_span.span_context();
    assert!(
        extracted_span_context.is_valid(),
        "Extracted context should have valid span context"
    );
    assert_eq!(
        extracted_span_context.trace_id(),
        trace_id,
        "Trace ID should be preserved"
    );
    assert_eq!(
        extracted_span_context.span_id(),
        span_id,
        "Span ID should be preserved"
    );
}

// ---------------------------------------------------------------------------
// Test 2: Span hierarchy parent-child
// ---------------------------------------------------------------------------

#[test]
fn test_span_hierarchy_parent_child() {
    let (provider, exporter) = setup_test_tracer();

    // Create parent span context (simulating incoming remote context)
    let parent_trace_id = TraceId::from_hex("12345678901234567890123456789012").unwrap();
    let parent_span_id = SpanId::from_hex("1234567890123456").unwrap();

    let parent_span_context = SpanContext::new(
        parent_trace_id,
        parent_span_id,
        TraceFlags::SAMPLED,
        true, // is_remote
        TraceState::default(),
    );

    let parent_cx = Context::new().with_remote_span_context(parent_span_context);

    // Create a child span using the provider
    let tracer = provider.tracer("test-tracer");

    let child_span = tracer
        .span_builder("child-span")
        .start_with_context(&tracer, &parent_cx);

    // Create context with child span and end it
    let child_cx = Context::current_with_span(child_span);
    child_cx.span().end();

    // Get exported spans
    let spans = exporter.get_finished_spans().expect("Should get spans");

    assert_eq!(spans.len(), 1, "Should have one exported span");

    let exported_span = &spans[0];

    // Verify the span has the correct parent span ID
    assert_eq!(
        exported_span.parent_span_id, parent_span_id,
        "Child span should have correct parent span ID"
    );

    // Verify the trace ID is inherited from parent
    assert_eq!(
        exported_span.span_context.trace_id(),
        parent_trace_id,
        "Child span should inherit trace ID from parent"
    );

    // The child span should have a different span ID
    assert_ne!(
        exported_span.span_context.span_id(),
        parent_span_id,
        "Child span should have different span ID than parent"
    );
}

// ---------------------------------------------------------------------------
// Test 3: TracingProcessor records span
// ---------------------------------------------------------------------------

#[tokio::test]
#[allow(clippy::await_holding_lock)] // intentional: serializes global tracer access across tests
async fn test_tracing_processor_records_span() {
    // Lock to serialize access to global tracer provider
    let _lock = lock_global_tracer();

    let (provider, exporter) = setup_test_tracer();

    // Set as global tracer provider
    global::set_tracer_provider(provider.clone());

    // Create a parent OTel context (simulating incoming trace)
    let parent_trace_id = TraceId::from_hex("abcdef1234567890abcdef1234567890").unwrap();
    let parent_span_id = SpanId::from_hex("abcdef1234567890").unwrap();

    let parent_span_context = SpanContext::new(
        parent_trace_id,
        parent_span_id,
        TraceFlags::SAMPLED,
        true, // is_remote
        TraceState::default(),
    );

    // Create exchange with parent context
    let mut exchange = Exchange::new(Message::new("test message"));
    exchange.otel_context = Context::new().with_remote_span_context(parent_span_context);

    // Create TracingProcessor wrapping IdentityProcessor
    let inner = BoxProcessor::new(IdentityProcessor);
    let mut tracing_processor = TracingProcessor::new(
        inner,
        "test-route".to_string(),
        0,
        DetailLevel::Minimal,
        None,
    );

    // Process the exchange
    let result: Result<Exchange, _> = tracing_processor
        .ready()
        .await
        .unwrap()
        .call(exchange)
        .await;

    assert!(result.is_ok(), "Processing should succeed");

    let output_exchange = result.unwrap();

    // Verify the exchange's otel_context after processing contains a child span
    let output_span = output_exchange.otel_context.span();
    let output_span_context = output_span.span_context();
    assert!(
        output_span_context.is_valid(),
        "Output exchange should have valid span context"
    );

    // The trace ID should be inherited from parent
    assert_eq!(
        output_span_context.trace_id(),
        parent_trace_id,
        "Output span should inherit trace ID from parent"
    );

    // Force flush to ensure spans are exported
    provider.force_flush().expect("Should flush");

    // Get exported spans
    let spans = exporter.get_finished_spans().expect("Should get spans");

    assert!(!spans.is_empty(), "Should have exported spans");

    // Find the span created by TracingProcessor
    let tracing_span = spans.iter().find(|s| s.name == "test-route/step-0");
    assert!(
        tracing_span.is_some(),
        "Should find span created by TracingProcessor"
    );

    let tracing_span = tracing_span.unwrap();

    // Verify the exported span has the correct parent span ID
    assert_eq!(
        tracing_span.parent_span_id, parent_span_id,
        "TracingProcessor span should have correct parent span ID"
    );

    // Verify the span has expected attributes
    let has_route_id_attr = tracing_span
        .attributes
        .iter()
        .any(|kv| kv.key.as_str() == "route_id" && kv.value.as_str() == "test-route");
    assert!(
        has_route_id_attr,
        "Span should have route_id attribute with correct value"
    );

    // Lock is released when _lock goes out of scope
}

// ---------------------------------------------------------------------------
// Test 4: No crash without provider
// ---------------------------------------------------------------------------

#[test]
fn test_no_crash_without_provider() {
    // This test runs without setting up a global tracer provider.
    // The noop provider should be used, and no panics should occur.

    // Create an exchange with default (empty) otel_context
    let exchange = Exchange::new(Message::new("test"));

    // Verify the initial context has no valid span
    let span = exchange.otel_context.span();
    let span_context = span.span_context();
    assert!(
        !span_context.is_valid(),
        "Exchange should start with invalid span context"
    );

    // inject_from_exchange should not panic
    let mut headers = HashMap::new();
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        inject_from_exchange(&exchange, &mut headers);
    }));
    assert!(
        result.is_ok(),
        "inject_from_exchange should not panic without provider"
    );

    // extract_into_exchange should not panic
    let mut new_exchange = Exchange::new(Message::new("test"));
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        extract_into_exchange(&mut new_exchange, &headers);
    }));
    assert!(
        result.is_ok(),
        "extract_into_exchange should not panic without provider"
    );

    // Even with valid traceparent header, extraction should work
    let mut headers_with_trace = HashMap::new();
    headers_with_trace.insert(
        "traceparent".to_string(),
        make_traceparent("4bf92f3577b34da6a3ce929d0e0e4736", "00f067aa0ba902b7", true),
    );

    let mut exchange_with_trace = Exchange::new(Message::new("test"));
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        extract_into_exchange(&mut exchange_with_trace, &headers_with_trace);
    }));
    assert!(
        result.is_ok(),
        "extract_into_exchange should not panic when extracting valid traceparent"
    );

    // Verify the trace context was extracted (even without a provider,
    // the extraction creates a context with the remote span context)
    let span = exchange_with_trace.otel_context.span();
    let span_context = span.span_context();
    assert!(
        span_context.is_valid(),
        "Should extract valid span context from headers"
    );
}

// ---------------------------------------------------------------------------
// Additional tests for completeness
// ---------------------------------------------------------------------------

#[test]
fn test_inject_context_creates_valid_traceparent() {
    // Create a context with a remote span context
    let trace_id = TraceId::from_hex("aaaabbbbccccddddeeeeffffaaaabbbb").unwrap();
    let span_id = SpanId::from_hex("aabbccdd00112233").unwrap();

    let span_context = SpanContext::new(
        trace_id,
        span_id,
        TraceFlags::SAMPLED,
        true,
        TraceState::default(),
    );

    let cx = Context::new().with_remote_span_context(span_context);

    // Inject into headers
    let mut headers = HashMap::new();
    inject_context(&cx, &mut headers);

    // Verify traceparent format
    let traceparent = headers.get("traceparent").expect("Should have traceparent");
    let parts: Vec<&str> = traceparent.split('-').collect();

    assert_eq!(parts.len(), 4, "traceparent should have 4 parts");
    assert_eq!(parts[0], "00", "version should be 00");
    assert_eq!(
        parts[1], "aaaabbbbccccddddeeeeffffaaaabbbb",
        "trace ID should match"
    );
    assert_eq!(parts[2], "aabbccdd00112233", "span ID should match");
    assert_eq!(parts[3], "01", "flags should be 01 (sampled)");
}

#[test]
fn test_extract_context_with_empty_headers() {
    let headers = HashMap::new();
    let cx = extract_context(&headers);

    // Empty headers should produce a context with invalid/no span context
    let span = cx.span();
    let span_context = span.span_context();
    assert!(
        !span_context.is_valid(),
        "Empty headers should produce invalid span context"
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // intentional: serializes global tracer access across tests
async fn test_tracing_processor_chain_preserves_trace_id() {
    // Lock to serialize access to global tracer provider
    let _lock = lock_global_tracer();

    let (provider, exporter) = setup_test_tracer();
    global::set_tracer_provider(provider.clone());

    // Create parent context
    let parent_trace_id = TraceId::from_hex("11111111111111111111111111111111").unwrap();
    let parent_span_id = SpanId::from_hex("1111111111111111").unwrap();

    let parent_span_context = SpanContext::new(
        parent_trace_id,
        parent_span_id,
        TraceFlags::SAMPLED,
        true,
        TraceState::default(),
    );

    let mut exchange = Exchange::new(Message::new("test"));
    exchange.otel_context = Context::new().with_remote_span_context(parent_span_context);

    // Chain two TracingProcessors
    let processor1 = BoxProcessor::new(IdentityProcessor);
    let mut tracer1 = TracingProcessor::new(
        processor1,
        "route1".to_string(),
        0,
        DetailLevel::Minimal,
        None,
    );

    let processor2 = BoxProcessor::new(IdentityProcessor);
    let mut tracer2 = TracingProcessor::new(
        processor2,
        "route2".to_string(),
        1,
        DetailLevel::Minimal,
        None,
    );

    // Process through chain
    let result1: Result<Exchange, _> = tracer1.ready().await.unwrap().call(exchange).await;
    let exchange1 = result1.unwrap();

    let result2: Result<Exchange, _> = tracer2.ready().await.unwrap().call(exchange1).await;
    let exchange2 = result2.unwrap();

    // Verify trace ID is preserved through the chain
    let final_span = exchange2.otel_context.span();
    let final_span_context = final_span.span_context();
    assert!(
        final_span_context.is_valid(),
        "Final exchange should have valid span context"
    );
    assert_eq!(
        final_span_context.trace_id(),
        parent_trace_id,
        "Trace ID should be preserved through processor chain"
    );

    // Force flush and verify both spans were exported
    provider.force_flush().expect("Should flush");
    let spans = exporter.get_finished_spans().expect("Should get spans");

    assert_eq!(spans.len(), 2, "Should have two exported spans");

    // Both spans should have the same trace ID
    for span in &spans {
        assert_eq!(
            span.span_context.trace_id(),
            parent_trace_id,
            "All spans should have the same trace ID"
        );
    }

    // Lock is released when _lock goes out of scope
}
