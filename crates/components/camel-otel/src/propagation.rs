//! W3C TraceContext propagation utilities for distributed tracing.
//!
//! This module provides functions to inject and extract OpenTelemetry context
//! using the W3C TraceContext format (`traceparent` and `tracestate` headers).
//!
//! # Example
//!
//! ```
//! use std::collections::HashMap;
//! use opentelemetry::Context;
//! use camel_otel::propagation::{inject_context, extract_context};
//!
//! // Inject context into headers for outgoing requests
//! let cx = Context::current();
//! let mut headers = HashMap::new();
//! inject_context(&cx, &mut headers);
//!
//! // Extract context from headers on incoming requests
//! let extracted = extract_context(&headers);
//! ```

use std::collections::HashMap;

use camel_api::exchange::Exchange;
use opentelemetry::propagation::{Extractor, Injector, TextMapPropagator};
use opentelemetry::Context;
use opentelemetry_sdk::propagation::TraceContextPropagator;

/// Header name for the W3C traceparent header.
pub const TRACE_PARENT_HEADER: &str = "traceparent";

/// Header name for the W3C tracestate header.
pub const TRACE_STATE_HEADER: &str = "tracestate";

// ---------------------------------------------------------------------------
// Internal wrapper types for Injector/Extractor trait implementations
// ---------------------------------------------------------------------------

/// A mutable wrapper around HashMap<String, String> that implements Injector.
///
/// This newtype is needed because OTel v0.31 doesn't have a blanket impl
/// for HashMap, so we provide our own wrapper.
struct HeaderInjector<'a>(&'a mut HashMap<String, String>);

impl Injector for HeaderInjector<'_> {
    /// Set a key-value pair in the headers (lowercase key for HTTP compatibility).
    fn set(&mut self, key: &str, value: String) {
        self.0.insert(key.to_lowercase(), value);
    }
}

/// A read-only wrapper around HashMap<String, String> that implements Extractor.
struct HeaderExtractor<'a>(&'a HashMap<String, String>);

impl Extractor for HeaderExtractor<'_> {
    /// Get a value from the headers by key (case-insensitive lookup).
    fn get(&self, key: &str) -> Option<&str> {
        // HTTP headers are case-insensitive, so we need to search case-insensitively
        self.0
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    /// Collect all keys from the headers.
    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|k| k.as_str()).collect()
    }
}

// ---------------------------------------------------------------------------
// Public API functions
// ---------------------------------------------------------------------------

/// Inject W3C traceparent/tracestate headers into a HashMap.
///
/// This function takes an OpenTelemetry context and injects the trace context
/// information into the provided headers map using the W3C TraceContext format.
///
/// # Arguments
///
/// * `cx` - The OpenTelemetry context to inject
/// * `headers` - The headers map to inject into (will be mutated)
///
/// # Example
///
/// ```
/// use std::collections::HashMap;
/// use opentelemetry::Context;
/// use camel_otel::propagation::inject_context;
///
/// let cx = Context::current();
/// let mut headers = HashMap::new();
/// inject_context(&cx, &mut headers);
/// // headers now contains traceparent (and optionally tracestate)
/// ```
pub fn inject_context(cx: &Context, headers: &mut HashMap<String, String>) {
    let propagator = TraceContextPropagator::new();
    let mut injector = HeaderInjector(headers);
    propagator.inject_context(cx, &mut injector);
}

/// Extract W3C traceparent/tracestate from a HashMap into an OpenTelemetry Context.
///
/// This function reads the W3C TraceContext headers from the provided headers map
/// and constructs an OpenTelemetry context with the extracted span context.
///
/// # Arguments
///
/// * `headers` - The headers map to extract from
///
/// # Returns
///
/// An OpenTelemetry `Context` containing the extracted span context.
/// If no valid trace context is found, returns a default/empty context.
///
/// # Example
///
/// ```
/// use std::collections::HashMap;
/// use camel_otel::propagation::extract_context;
///
/// let mut headers = HashMap::new();
/// headers.insert("traceparent".to_string(), "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_string());
///
/// let cx = extract_context(&headers);
/// // cx now contains the extracted span context
/// ```
pub fn extract_context(headers: &HashMap<String, String>) -> Context {
    let propagator = TraceContextPropagator::new();
    let extractor = HeaderExtractor(headers);
    propagator.extract(&extractor)
}

/// Inject context from an Exchange into headers (convenience wrapper).
///
/// This is a convenience function that extracts the `otel_context` from an
/// Exchange and injects it into the provided headers map.
///
/// # Arguments
///
/// * `exchange` - The Exchange containing the OTel context
/// * `headers` - The headers map to inject into (will be mutated)
///
/// # Example
///
/// ```
/// use std::collections::HashMap;
/// use camel_api::exchange::Exchange;
/// use camel_api::message::Message;
/// use camel_otel::propagation::inject_from_exchange;
///
/// let exchange = Exchange::new(Message::new("test"));
/// let mut headers = HashMap::new();
/// inject_from_exchange(&exchange, &mut headers);
/// ```
pub fn inject_from_exchange(exchange: &Exchange, headers: &mut HashMap<String, String>) {
    inject_context(&exchange.otel_context, headers);
}

/// Extract context from headers into an Exchange (updates exchange.otel_context).
///
/// This function extracts the trace context from headers and updates the
/// Exchange's `otel_context` field with the extracted context.
///
/// # Arguments
///
/// * `exchange` - The Exchange to update (will be mutated)
/// * `headers` - The headers map to extract from
///
/// # Example
///
/// ```
/// use std::collections::HashMap;
/// use camel_api::exchange::Exchange;
/// use camel_api::message::Message;
/// use camel_otel::propagation::extract_into_exchange;
///
/// let mut exchange = Exchange::new(Message::new("test"));
/// let mut headers = HashMap::new();
/// headers.insert("traceparent".to_string(), "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_string());
///
/// extract_into_exchange(&mut exchange, &headers);
/// // exchange.otel_context now contains the extracted span context
/// ```
pub fn extract_into_exchange(exchange: &mut Exchange, headers: &HashMap<String, String>) {
    exchange.otel_context = extract_context(headers);
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::message::Message;
    use opentelemetry::trace::{SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState};

    /// Create a valid traceparent header value for testing.
    fn make_traceparent(trace_id_hex: &str, span_id_hex: &str, sampled: bool) -> String {
        let flags = if sampled { "01" } else { "00" };
        format!("00-{}-{}-{}", trace_id_hex, span_id_hex, flags)
    }

    #[test]
    fn test_inject_extract_roundtrip() {
        // Create a context with a valid span context
        let trace_id_hex = "4bf92f3577b34da6a3ce929d0e0e4736";
        let span_id_hex = "00f067aa0ba902b7";
        
        let traceparent = make_traceparent(trace_id_hex, span_id_hex, true);
        
        // Create headers with traceparent
        let mut headers = HashMap::new();
        headers.insert("traceparent".to_string(), traceparent.clone());
        
        // Extract context
        let extracted = extract_context(&headers);
        
        // Inject into new headers
        let mut new_headers = HashMap::new();
        inject_context(&extracted, &mut new_headers);
        
        // Verify roundtrip - the traceparent should be preserved
        assert!(new_headers.contains_key("traceparent"));
        let new_traceparent = new_headers.get("traceparent").unwrap();
        
        // Parse and compare trace IDs (they should match)
        let original_parts: Vec<&str> = traceparent.split('-').collect();
        let new_parts: Vec<&str> = new_traceparent.split('-').collect();
        
        assert_eq!(original_parts[0], new_parts[0], "version should match");
        assert_eq!(original_parts[1], new_parts[1], "trace-id should match");
        assert_eq!(original_parts[2], new_parts[2], "parent-id should match");
        assert_eq!(original_parts[3], new_parts[3], "flags should match");
    }

    #[test]
    fn test_extract_empty_headers_returns_default_context() {
        let headers = HashMap::new();
        let cx = extract_context(&headers);
        
        // The extracted context should have no valid span context
        let span = cx.span();
        let span_context = span.span_context();
        
        assert!(!span_context.is_valid(), "Empty headers should produce invalid span context");
    }

    #[test]
    fn test_inject_produces_traceparent_header() {
        // Create a context with a valid span using with_remote_span_context
        let trace_id = TraceId::from_hex("4bf92f3577b34da6a3ce929d0e0e4736").unwrap();
        let span_id = SpanId::from_hex("00f067aa0ba902b7").unwrap();
        let trace_state = TraceState::default();
        
        let span_context = SpanContext::new(
            trace_id,
            span_id,
            TraceFlags::SAMPLED,
            true, // is_remote - this is a remote span context
            trace_state,
        );
        
        // Create a context with this span context
        let cx = Context::new().with_remote_span_context(span_context);
        
        let mut headers = HashMap::new();
        inject_context(&cx, &mut headers);
        
        // Verify traceparent header exists
        assert!(headers.contains_key("traceparent"), "Should have traceparent header");
        
        let traceparent = headers.get("traceparent").unwrap();
        
        // Verify format: version-traceid-spanid-flags
        let parts: Vec<&str> = traceparent.split('-').collect();
        assert_eq!(parts.len(), 4, "traceparent should have 4 parts");
        assert_eq!(parts[0], "00", "version should be 00");
        assert_eq!(parts[1], "4bf92f3577b34da6a3ce929d0e0e4736", "trace-id should match");
        assert_eq!(parts[2], "00f067aa0ba902b7", "span-id should match");
        assert_eq!(parts[3], "01", "flags should be 01 (sampled)");
    }

    #[test]
    fn test_inject_from_exchange() {
        let mut exchange = Exchange::new(Message::new("test"));
        
        // Create a span context and attach to exchange
        let trace_id = TraceId::from_hex("12345678901234567890123456789012").unwrap();
        let span_id = SpanId::from_hex("1234567890123456").unwrap();
        let span_context = SpanContext::new(
            trace_id,
            span_id,
            TraceFlags::SAMPLED,
            true,
            TraceState::default(),
        );
        
        exchange.otel_context = Context::new().with_remote_span_context(span_context);
        
        let mut headers = HashMap::new();
        inject_from_exchange(&exchange, &mut headers);
        
        assert!(headers.contains_key("traceparent"), "Should inject traceparent from exchange");
    }

    #[test]
    fn test_extract_into_exchange() {
        let mut exchange = Exchange::new(Message::new("test"));
        
        // Verify initial context is empty
        assert!(!exchange.otel_context.span().span_context().is_valid());
        
        // Create headers with traceparent
        let mut headers = HashMap::new();
        let traceparent = make_traceparent("4bf92f3577b34da6a3ce929d0e0e4736", "00f067aa0ba902b7", true);
        headers.insert("traceparent".to_string(), traceparent);
        
        extract_into_exchange(&mut exchange, &headers);
        
        // Verify context was extracted
        assert!(
            exchange.otel_context.span().span_context().is_valid(),
            "Exchange should have valid span context after extraction"
        );
    }

    #[test]
    fn test_header_keys_case_insensitive() {
        let mut headers = HashMap::new();
        // Use uppercase key
        headers.insert("TRACEPARENT".to_string(), make_traceparent("00000000000000000000000000000123", "0000000000000456", true));
        
        let cx = extract_context(&headers);
        
        assert!(
            cx.span().span_context().is_valid(),
            "Should extract from case-insensitive header keys"
        );
    }

    #[test]
    fn test_extract_with_tracestate() {
        let mut headers = HashMap::new();
        headers.insert(
            "traceparent".to_string(),
            make_traceparent("4bf92f3577b34da6a3ce929d0e0e4736", "00f067aa0ba902b7", true),
        );
        headers.insert("tracestate".to_string(), "vendor1=value1,vendor2=value2".to_string());
        
        let cx = extract_context(&headers);
        
        let span = cx.span();
        let span_context = span.span_context();
        assert!(span_context.is_valid());
        
        // TraceState should be extracted - check by getting a header from it
        let trace_state = span_context.trace_state();
        // In OTel v0.31, we can check if tracestate has content by getting a key
        assert!(
            trace_state.get("vendor1").is_some() || trace_state.get("vendor2").is_some(),
            "Should have tracestate entries"
        );
    }
}
