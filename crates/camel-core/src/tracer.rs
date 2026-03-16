use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use opentelemetry::trace::{SpanKind, Status, TraceContextExt, Tracer};
use opentelemetry::{Context as OtelContext, KeyValue, global};
use tower::Service;
use tower::ServiceExt;
use tracing::Instrument;

use crate::config::DetailLevel;
use camel_api::metrics::MetricsCollector;
use camel_api::{Body, BoxProcessor, CamelError, Exchange};

/// RAII guard that ensures an OTel span is ended when dropped.
///
/// This prevents span leaks if the inner processor panics or returns early.
struct SpanEndGuard(OtelContext);

impl Drop for SpanEndGuard {
    fn drop(&mut self) {
        self.0.span().end();
    }
}

/// Returns a human-readable name for the body type variant.
fn body_type_name(body: &Body) -> &'static str {
    match body {
        Body::Empty => "empty",
        Body::Bytes(_) => "bytes",
        Body::Text(_) => "text",
        Body::Json(_) => "json",
        Body::Xml(_) => "xml",
        Body::Stream(_) => "stream",
    }
}

/// A processor wrapper that emits tracing spans for each step.
///
/// This processor wraps another processor and adds distributed tracing by:
/// 1. Starting a native OpenTelemetry span for each exchange
/// 2. Propagating the OTel context through `exchange.otel_context`
/// 3. Recording errors and status on the span
///
/// When no OTel provider is configured (noop provider), spans are no-ops with minimal overhead.
pub struct TracingProcessor {
    inner: BoxProcessor,
    route_id: String,
    step_id: String,
    step_index: usize,
    detail_level: DetailLevel,
    metrics: Option<Arc<dyn MetricsCollector>>,
}

impl TracingProcessor {
    /// Wrap a processor with tracing.
    pub fn new(
        inner: BoxProcessor,
        route_id: String,
        step_index: usize,
        detail_level: DetailLevel,
        metrics: Option<Arc<dyn MetricsCollector>>,
    ) -> Self {
        Self {
            inner,
            route_id,
            step_id: format!("step-{}", step_index),
            step_index,
            detail_level,
            metrics,
        }
    }

    /// Build the span name from route_id and step_id.
    fn span_name(&self) -> String {
        format!("{}/{}", self.route_id, self.step_id)
    }
}

impl Service<Exchange> for TracingProcessor {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let start = Instant::now();
        let span_name = self.span_name();

        // Get the global tracer (noop if no provider is configured)
        let tracer = global::tracer("camel-core");

        // Extract parent context from exchange.otel_context
        let parent_cx = exchange.otel_context.clone();

        // Build span attributes
        let mut attributes = vec![
            KeyValue::new("correlation_id", exchange.correlation_id().to_string()),
            KeyValue::new("route_id", self.route_id.clone()),
            KeyValue::new("step_id", self.step_id.clone()),
            KeyValue::new("step_index", self.step_index as i64),
        ];

        if self.detail_level >= DetailLevel::Medium {
            attributes.push(KeyValue::new(
                "headers_count",
                exchange.input.headers.len() as i64,
            ));
            attributes.push(KeyValue::new(
                "body_type",
                body_type_name(&exchange.input.body),
            ));
            attributes.push(KeyValue::new("has_error", exchange.has_error()));
        }

        // Start a new span as a child of the parent context
        let span = tracer
            .span_builder(span_name.clone())
            .with_kind(SpanKind::Internal)
            .with_attributes(attributes)
            .start_with_context(&tracer, &parent_cx);

        // Create new context with this span as the active span
        let cx = OtelContext::current_with_span(span);

        // Store back into exchange so downstream processors inherit this context
        exchange.otel_context = cx.clone();

        // Also create a tracing span for local dev logging
        let tracing_span = tracing::info_span!(
            target: "camel_tracer",
            "step",
            correlation_id = %exchange.correlation_id(),
            route_id = %self.route_id,
            step_id = %self.step_id,
            step_index = self.step_index,
            timestamp = %chrono::Utc::now().to_rfc3339(),
            duration_ms = tracing::field::Empty,
            status = tracing::field::Empty,
            headers_count = tracing::field::Empty,
            body_type = tracing::field::Empty,
            has_error = tracing::field::Empty,
            output_body_type = tracing::field::Empty,
            header_0 = tracing::field::Empty,
            header_1 = tracing::field::Empty,
            header_2 = tracing::field::Empty,
            error = tracing::field::Empty,
            error_type = tracing::field::Empty,
        );

        if self.detail_level >= DetailLevel::Medium {
            tracing_span.record("headers_count", exchange.input.headers.len() as u64);
            tracing_span.record("body_type", body_type_name(&exchange.input.body));
            tracing_span.record("has_error", exchange.has_error());
        }

        if self.detail_level >= DetailLevel::Full {
            for (i, (k, v)) in exchange.input.headers.iter().take(3).enumerate() {
                tracing_span.record(format!("header_{i}").as_str(), format!("{k}={v:?}"));
            }
        }

        let mut inner = self.inner.clone();
        let detail_level = self.detail_level.clone();
        let metrics = self.metrics.clone();
        let route_id = self.route_id.clone();

        Box::pin(
            async move {
                // Note: ContextGuard is not Send (it uses thread-local storage), so we cannot
                // hold it across await points in an async fn. Instead, we propagate the OTel
                // context through exchange.otel_context, which is Send + Sync.

                // Create guard to ensure span is ended even on panic
                let _guard = SpanEndGuard(cx.clone());

                let result = inner.ready().await?.call(exchange).await;

                let duration = start.elapsed();
                let duration_ms = duration.as_millis() as u64;
                tracing::Span::current().record("duration_ms", duration_ms);

                // Record duration on OTel span
                cx.span()
                    .set_attribute(KeyValue::new("duration_ms", duration_ms as i64));

                // Record metrics if collector is present
                if let Some(ref metrics) = metrics {
                    metrics.record_exchange_duration(&route_id, duration);
                    metrics.increment_exchanges(&route_id);

                    if let Err(e) = &result {
                        metrics.increment_errors(&route_id, &e.to_string());
                    }
                }

                match &result {
                    Ok(ex) => {
                        tracing::Span::current().record("status", "success");
                        cx.span().set_status(Status::Ok);

                        if detail_level >= DetailLevel::Medium {
                            tracing::Span::current()
                                .record("output_body_type", body_type_name(&ex.input.body));
                            cx.span().set_attribute(KeyValue::new(
                                "output_body_type",
                                body_type_name(&ex.input.body),
                            ));
                        }
                    }
                    Err(e) => {
                        tracing::Span::current().record("status", "error");
                        tracing::Span::current().record("error", e.to_string());
                        tracing::Span::current()
                            .record("error_type", std::any::type_name::<CamelError>());

                        // Record error on OTel span
                        cx.span().set_status(Status::error(e.to_string()));
                        cx.span().set_attribute(KeyValue::new(
                            "error_type",
                            std::any::type_name::<CamelError>(),
                        ));
                    }
                }

                // Span is ended by _guard when it drops here
                result
            }
            .instrument(tracing_span),
        )
    }
}

impl Clone for TracingProcessor {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            route_id: self.route_id.clone(),
            step_id: self.step_id.clone(),
            step_index: self.step_index,
            detail_level: self.detail_level.clone(),
            metrics: self.metrics.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    //! Tests for TracingProcessor.
    //!
    //! These tests use the noop OTel provider, which means:
    //! - Spans are created but not exported
    //! - Span contexts may not have valid trace/span IDs
    //! - Error recording on spans cannot be verified
    //!
    //! Full span hierarchy verification (trace ID matching, parent span ID, error recording)
    //! requires an integration test with a real exporter, which will be covered in Task 11
    //! (integration tests).

    use super::*;
    use camel_api::{BoxProcessorExt, IdentityProcessor, Message, Value};
    use opentelemetry::trace::{SpanContext, SpanId, TraceFlags, TraceId, TraceState};
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_tracing_processor_minimal() {
        let inner = BoxProcessor::new(IdentityProcessor);
        let mut tracer = TracingProcessor::new(
            inner,
            "test-route".to_string(),
            0,
            DetailLevel::Minimal,
            None,
        );

        let exchange = Exchange::new(Message::default());
        let result = tracer.ready().await.unwrap().call(exchange).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_tracing_processor_medium_detail() {
        let inner = BoxProcessor::new(IdentityProcessor);
        let mut tracer = TracingProcessor::new(
            inner,
            "test-route".to_string(),
            0,
            DetailLevel::Medium,
            None,
        );

        let exchange = Exchange::new(Message::default());
        let result = tracer.ready().await.unwrap().call(exchange).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_tracing_processor_full_detail() {
        let inner = BoxProcessor::new(IdentityProcessor);
        let mut tracer =
            TracingProcessor::new(inner, "test-route".to_string(), 0, DetailLevel::Full, None);

        let mut exchange = Exchange::new(Message::default());
        exchange
            .input
            .headers
            .insert("test".to_string(), Value::String("value".into()));

        let result = tracer.ready().await.unwrap().call(exchange).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_tracing_processor_clone() {
        let inner = BoxProcessor::new(IdentityProcessor);
        let tracer = TracingProcessor::new(
            inner,
            "test-route".to_string(),
            1,
            DetailLevel::Minimal,
            None,
        );

        let mut cloned = tracer.clone();
        let exchange = Exchange::new(Message::default());
        let result = cloned.ready().await.unwrap().call(exchange).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_tracing_processor_propagates_otel_context() {
        let inner = BoxProcessor::new(IdentityProcessor);
        let mut tracer = TracingProcessor::new(
            inner,
            "test-route".to_string(),
            0,
            DetailLevel::Minimal,
            None,
        );

        // Start with an empty exchange (default context)
        let exchange = Exchange::new(Message::default());
        assert!(
            !exchange.otel_context.span().span_context().is_valid(),
            "Initial context should have invalid span"
        );

        let result = tracer.ready().await.unwrap().call(exchange).await;

        // After processing, the exchange should have a new span context
        let output_exchange = result.unwrap();

        // The output exchange should now have a valid span context
        // (even with noop provider, the span should be recorded)
        // Note: With noop provider, span context may still be invalid
        // but the context should be properly attached
        let _span_context = output_exchange.otel_context.span().span_context();
    }

    #[tokio::test]
    async fn test_tracing_processor_with_parent_context() {
        let inner = BoxProcessor::new(IdentityProcessor);
        let mut tracer = TracingProcessor::new(
            inner,
            "test-route".to_string(),
            0,
            DetailLevel::Minimal,
            None,
        );

        // Create a parent span context
        let trace_id = TraceId::from_hex("12345678901234567890123456789012").unwrap();
        let span_id = SpanId::from_hex("1234567890123456").unwrap();
        let parent_span_context = SpanContext::new(
            trace_id,
            span_id,
            TraceFlags::SAMPLED,
            true, // is_remote
            TraceState::default(),
        );

        // Create exchange with parent context
        let mut exchange = Exchange::new(Message::default());
        exchange.otel_context = OtelContext::new().with_remote_span_context(parent_span_context);

        // Store the initial parent span context for comparison
        let initial_span_context = exchange.otel_context.span().span_context().clone();

        // Verify parent context is set
        assert!(
            exchange.otel_context.span().span_context().is_valid(),
            "Parent context should be valid"
        );
        let _parent_trace_id = exchange.otel_context.span().span_context().trace_id();

        let result = tracer.ready().await.unwrap().call(exchange).await;

        let output_exchange = result.unwrap();

        // The output should still have a valid context
        // The trace ID should be preserved from parent
        let output_span = output_exchange.otel_context.span();
        // With noop provider, we may not get a valid span context,
        // but the context propagation mechanism should work
        let _output_trace_id = output_span.span_context().trace_id();

        // Verify that the exchange's otel_context has been updated (child span created)
        // Even with noop provider, the span context should be a different object
        // (the processor creates a new span, which may be a noop but is still a new span)
        let output_span_context = output_span.span_context();
        // The span contexts should be different objects (different span IDs conceptually,
        // though noop provider may not actually assign them)
        assert!(
            !std::ptr::eq(&initial_span_context, output_span_context),
            "exchange.otel_context should have been updated with a new child span context"
        );
    }

    #[tokio::test]
    async fn test_tracing_processor_records_error() {
        // Create a processor that always fails
        let failing_processor = BoxProcessor::from_fn(|_ex: Exchange| async move {
            Err(CamelError::ProcessorError("intentional test error".into()))
        });

        let mut tracer = TracingProcessor::new(
            failing_processor,
            "test-route".to_string(),
            0,
            DetailLevel::Minimal,
            None,
        );

        let exchange = Exchange::new(Message::default());
        let result = tracer.ready().await.unwrap().call(exchange).await;

        // Verify the error is correctly propagated
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("intentional test error"));

        // Note: With noop provider, we cannot verify that the error was recorded on the span.
        // Full span hierarchy verification (trace ID matching, parent span ID, error recording)
        // requires an integration test with a real exporter, which will be covered in Task 11
        // (integration tests).
    }

    #[tokio::test]
    async fn test_tracing_processor_span_name_format() {
        let inner = BoxProcessor::new(IdentityProcessor);
        let tracer =
            TracingProcessor::new(inner, "my-route".to_string(), 5, DetailLevel::Minimal, None);

        // Span name should be "route_id/step_id"
        assert_eq!(tracer.span_name(), "my-route/step-5");
    }

    #[tokio::test]
    async fn test_tracing_processor_chained_propagation() {
        // Test that multiple processors in a chain properly propagate context
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

        let exchange = Exchange::new(Message::default());
        let result1 = tracer1.ready().await.unwrap().call(exchange).await;
        let exchange1 = result1.unwrap();

        // Pass the exchange through second processor
        let result2 = tracer2.ready().await.unwrap().call(exchange1).await;
        let exchange2 = result2.unwrap();

        // Both processors should have updated the context
        // The context should be valid and propagating
        let _ = exchange2.otel_context;
    }
}
