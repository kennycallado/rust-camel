use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use opentelemetry::trace::{SpanKind, SpanRef, Status, TraceContextExt, Tracer};
use opentelemetry::{Context as OtelContext, KeyValue, global};
use tower::Service;
use tracing::Instrument;

use crate::shared::observability::domain::DetailLevel;
use camel_api::metrics::MetricsCollector;
use camel_api::{Body, BoxProcessor, CamelError, Exchange};

/// RAII guard that ensures an OTel span is ended when dropped.
///
/// This prevents span leaks if the inner processor panics or returns early.
/// `pub(crate)` so the route compiler can reuse it for the route root span.
pub(crate) struct SpanEndGuard(pub(crate) OtelContext);

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
        _ => "unknown",
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
    span_name: String,
    step_index: usize,
    detail_level: DetailLevel,
    metrics: Option<Arc<dyn MetricsCollector>>,
}

fn step_id_for(index: usize) -> String {
    format!("step-{index}")
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
        let step_id = step_id_for(step_index);
        let span_name = format!("{route_id}:{step_id}");
        Self {
            inner,
            route_id,
            step_id,
            span_name,
            step_index,
            detail_level,
            metrics,
        }
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
        let span_name = self.span_name.clone();

        // Get the global tracer (noop if no provider is configured)
        let tracer = global::tracer("camel-core");

        // Extract parent context from exchange.otel_context
        let parent_cx = exchange.otel_context.clone();

        // Build span attributes (Minimal set; Medium/Full extras appended below)
        let mut attributes =
            step_span_attributes(&self.route_id, self.step_index, exchange.correlation_id());

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
            .span_builder(span_name)
            .with_kind(SpanKind::Internal)
            .with_attributes(attributes.iter().cloned())
            .start_with_context(&tracer, &parent_cx);

        // Derive the step context from the parent (not from the ambient
        // current context): parent entries such as baggage stay attached, and
        // the parent context is restored on the result exchange after the step.
        let cx = parent_cx.with_span(span);

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
            let headers: Vec<_> = exchange.input.headers.iter().take(3).collect();
            if let Some((k, v)) = headers.first() {
                tracing_span.record("header_0", format!("{k}={v:?}"));
            }
            if let Some((k, v)) = headers.get(1) {
                tracing_span.record("header_1", format!("{k}={v:?}"));
            }
            if let Some((k, v)) = headers.get(2) {
                tracing_span.record("header_2", format!("{k}={v:?}"));
            }
        }

        // Consume the ORIGINAL inner that `poll_ready` readied: its
        // reservations (e.g. DirectProducer's pending semaphore permit)
        // belong to that instance, so re-readying a clone would drop them.
        // The fresh clone stays in `self.inner` as the unreadied placeholder
        // for the next ready/call cycle.
        let fresh = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, fresh);
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

                let result = inner.call(exchange).await;

                let duration = start.elapsed();
                let duration_ms = duration.as_millis() as u64;
                tracing::Span::current().record("duration_ms", duration_ms);

                // Record metrics if collector is present
                if let Some(ref metrics) = metrics {
                    metrics.record_exchange_duration(&route_id, duration);
                    metrics.increment_exchanges(&route_id);

                    if let Err(e) = &result {
                        metrics.increment_errors(&route_id, e.classify());
                    }
                }

                match result {
                    Ok(mut ex) => {
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

                        // Restore the caller's context: the step span ends with
                        // this future, so downstream steps must not chain onto it.
                        ex.otel_context = parent_cx.clone();
                        Ok(ex)
                    }
                    Err(e) => {
                        record_exception(&cx.span(), &e);
                        let error_class = e.classify();
                        tracing::Span::current().record("status", "error");
                        tracing::Span::current().record("error", e.to_string());
                        tracing::Span::current().record("error_type", error_class);
                        Err(e)
                    }
                }
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
            span_name: self.span_name.clone(),
            step_index: self.step_index,
            detail_level: self.detail_level.clone(),
            metrics: self.metrics.clone(),
        }
    }
}

/// R4-L8: cap only the span-attr representation. Exchange.correlation_id is untouched.
pub(crate) fn capped_correlation_id(id: &str) -> &str {
    const CAP: usize = 128;
    if id.len() > CAP {
        "<oversized:correlation_id>"
    } else {
        id
    }
}

/// Minimal-level span attributes for a pipeline step span.
///
/// `correlation_id` is the raw exchange correlation id; this is the single
/// capping site for its span-attribute representation (R4-L8). Medium/Full
/// extras (`headers_count`, `body_type`, `has_error`) are appended by the
/// caller, not here.
pub(crate) fn step_span_attributes(
    route_id: &str,
    step_index: usize,
    correlation_id: &str,
) -> Vec<KeyValue> {
    vec![
        KeyValue::new("messaging.system", "camel"),
        KeyValue::new(
            "correlation_id",
            capped_correlation_id(correlation_id).to_string(),
        ),
        KeyValue::new("route_id", route_id.to_string()),
        KeyValue::new("step_id", step_id_for(step_index)),
        KeyValue::new("step_index", step_index as i64),
    ]
}

pub(crate) fn record_exception(span: &SpanRef<'_>, e: &CamelError) {
    let error_class = e.classify();
    span.set_status(Status::error(e.to_string()));
    span.add_event(
        "exception",
        vec![
            KeyValue::new("exception.type", error_class.to_string()),
            KeyValue::new("exception.message", e.to_string()),
        ],
    );
}

#[cfg(test)]
mod tests {
    //! Tests for TracingProcessor.
    //!
    //! Span-exporting tests use `span_test_util`, which installs ONE global
    //! in-memory `SdkTracerProvider` per test binary; test bodies using it are
    //! serialized by a process-wide mutex and filter exported spans by their
    //! own trace id. The remaining tests assert only provider-independent
    //! behavior (error propagation, clone semantics, readiness).

    use super::*;
    use crate::shared::observability::adapters::span_test_util::{finish, test_spans};
    use camel_api::{BoxProcessorExt, IdentityProcessor, Message, Value};
    use opentelemetry::baggage::BaggageExt;
    use opentelemetry::trace::{Span, SpanId, TraceId};
    use std::time::Duration;
    use tokio::sync::{OwnedSemaphorePermit, Semaphore};
    use tower::ServiceExt;

    /// Local double whose `call` always fails with a processor error.
    #[derive(Clone)]
    struct ErrProcessor;

    impl Service<Exchange> for ErrProcessor {
        type Response = Exchange;
        type Error = CamelError;
        type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _exchange: Exchange) -> Self::Future {
            Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
        }
    }

    /// Creates a parent span via the global tracer, returns its ids plus an
    /// exchange whose `otel_context` carries that span as the active span.
    fn exchange_under_parent_span() -> (Exchange, TraceId, SpanId) {
        let tracer = global::tracer("camel-core-test");
        let parent = tracer.span_builder("parent").start(&tracer);
        let trace_id = parent.span_context().trace_id();
        let parent_span_id = parent.span_context().span_id();

        let mut exchange = Exchange::new(Message::default());
        exchange.otel_context = OtelContext::current_with_span(parent);
        (exchange, trace_id, parent_span_id)
    }

    #[tokio::test]
    async fn step_span_has_no_duration_ms_attribute() {
        let spans = test_spans().await;
        let (exchange, trace_id, parent_span_id) = exchange_under_parent_span();

        let inner = BoxProcessor::new(IdentityProcessor);
        let mut proc = TracingProcessor::new(inner, "r".to_string(), 0, DetailLevel::Minimal, None);
        let outcome = proc
            .ready()
            .await
            .expect("service ready")
            .call(exchange)
            .await;
        outcome.expect("step call succeeds");

        let all = finish(spans);
        let step = all
            .into_iter()
            .filter(|s| s.span_context.trace_id() == trace_id)
            .find(|s| s.name == "r:step-0")
            .expect("step span exported under parent trace");

        assert_eq!(step.parent_span_id, parent_span_id);
        assert!(
            !step
                .attributes
                .iter()
                .any(|kv| kv.key.as_str() == "duration_ms"),
            "duration_ms must not be recorded as a span attribute"
        );
    }

    #[tokio::test]
    async fn step_restores_parent_context_after_call() {
        let _spans = test_spans().await;
        let (mut exchange, _trace_id, parent_span_id) = exchange_under_parent_span();
        exchange.otel_context = exchange
            .otel_context
            .clone()
            .with_baggage([KeyValue::new("baggage_test", "1")]);

        let inner = BoxProcessor::new(IdentityProcessor);
        let mut proc = TracingProcessor::new(inner, "r".to_string(), 0, DetailLevel::Minimal, None);
        let outcome = proc
            .ready()
            .await
            .expect("service ready")
            .call(exchange)
            .await;
        let ex = outcome.expect("step call succeeds");

        assert_eq!(
            ex.otel_context.span().span_context().span_id(),
            parent_span_id,
            "active span on the returned exchange must be the parent, not the step span"
        );
        assert!(
            ex.otel_context.baggage().get("baggage_test").is_some(),
            "parent context baggage must survive the step"
        );
    }

    #[tokio::test]
    async fn step_error_emits_exception_event() {
        let spans = test_spans().await;
        let (exchange, trace_id, _parent_span_id) = exchange_under_parent_span();

        let inner = BoxProcessor::new(ErrProcessor);
        let mut proc = TracingProcessor::new(inner, "r".to_string(), 0, DetailLevel::Minimal, None);
        let outcome = proc
            .ready()
            .await
            .expect("service ready")
            .call(exchange)
            .await;
        assert!(outcome.is_err(), "error must propagate to the caller");

        let all = finish(spans);
        let step = all
            .into_iter()
            .filter(|s| s.span_context.trace_id() == trace_id)
            .find(|s| s.name == "r:step-0")
            .expect("step span exported under parent trace");

        assert_eq!(step.events.len(), 1, "exactly one event on error");
        let event = &step.events[0];
        assert_eq!(event.name, "exception");
        let attr = |key: &str| {
            event
                .attributes
                .iter()
                .find(|kv| kv.key.as_str() == key)
                .map(|kv| kv.value.as_str().to_string())
        };
        assert!(
            attr("exception.type").is_some_and(|v| !v.is_empty()),
            "exception.type must be non-empty"
        );
        assert!(
            attr("exception.message").is_some_and(|v| !v.is_empty()),
            "exception.message must be non-empty"
        );
        assert!(
            matches!(step.status, Status::Error { .. }),
            "span status must be Error"
        );
    }

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
        let _spans = test_spans().await;
        let inner = BoxProcessor::new(IdentityProcessor);
        let mut tracer = TracingProcessor::new(
            inner,
            "test-route".to_string(),
            0,
            DetailLevel::Minimal,
            None,
        );

        let (exchange, _trace_id, parent_span_id) = exchange_under_parent_span();

        // Verify parent context is set
        assert!(
            exchange.otel_context.span().span_context().is_valid(),
            "Parent context should be valid"
        );
        let _parent_trace_id = exchange.otel_context.span().span_context().trace_id();

        let result = tracer.ready().await.unwrap().call(exchange).await;

        let output_exchange = result.unwrap();

        // The step restores the caller's context so downstream steps continue
        // from the parent span rather than chaining onto the completed step.
        assert_eq!(
            output_exchange.otel_context.span().span_context().span_id(),
            parent_span_id,
            "output exchange must restore the parent span context"
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

        // Full span hierarchy and error-recording coverage lives in this file
        // via `step_error_emits_exception_event` and `span_test_util`.
    }

    #[tokio::test]
    async fn test_tracing_processor_span_name_format() {
        let inner = BoxProcessor::new(IdentityProcessor);
        let tracer =
            TracingProcessor::new(inner, "my-route".to_string(), 5, DetailLevel::Minimal, None);

        assert_eq!(tracer.span_name, "my-route:step-5");
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

    #[test]
    fn capped_correlation_id_uses_sentinel_for_oversized() {
        assert_eq!(
            capped_correlation_id(&"x".repeat(200)),
            "<oversized:correlation_id>"
        );
        assert_eq!(
            capped_correlation_id(&"y".repeat(300)),
            "<oversized:correlation_id>"
        );
        assert_eq!(capped_correlation_id("abc-123"), "abc-123");
    }

    /// Mock inner mirroring `DirectProducer`'s stateful readiness: `poll_ready`
    /// acquires the sole permit of a shared semaphore into `pending_permit`,
    /// and `Clone` shares the semaphore but drops the permit. Calling `call`
    /// without a reserved permit fails, so any readiness state loss is
    /// detected instead of silently proceeding.
    struct PermitGateInner {
        semaphore: Arc<Semaphore>,
        pending_permit: Option<OwnedSemaphorePermit>,
    }

    impl PermitGateInner {
        fn new() -> Self {
            Self {
                semaphore: Arc::new(Semaphore::new(1)),
                pending_permit: None,
            }
        }
    }

    impl Clone for PermitGateInner {
        fn clone(&self) -> Self {
            Self {
                semaphore: Arc::clone(&self.semaphore),
                pending_permit: None,
            }
        }
    }

    impl Service<Exchange> for PermitGateInner {
        type Response = Exchange;
        type Error = CamelError;
        type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

        fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            // Already holding the permit: ready.
            if self.pending_permit.is_some() {
                return Poll::Ready(Ok(()));
            }
            let mut fut = std::pin::pin!(Arc::clone(&self.semaphore).acquire_owned());
            match fut.as_mut().poll(cx) {
                Poll::Ready(Ok(permit)) => {
                    self.pending_permit = Some(permit);
                    Poll::Ready(Ok(()))
                }
                Poll::Pending => Poll::Pending,
                // Unreachable in practice: the semaphore is never closed.
                Poll::Ready(Err(err)) => {
                    Poll::Ready(Err(CamelError::ProcessorError(err.to_string())))
                }
            }
        }

        fn call(&mut self, exchange: Exchange) -> Self::Future {
            let permit = self.pending_permit.take();
            Box::pin(async move {
                match permit {
                    Some(_permit) => Ok(exchange),
                    None => Err(CamelError::ProcessorError(
                        "call() invoked without a reserved permit".into(),
                    )),
                }
            })
        }
    }

    #[tokio::test]
    async fn tracing_processor_does_not_re_ready_clone() {
        let mock_inner = BoxProcessor::new(PermitGateInner::new());
        let mut tracing_proc =
            TracingProcessor::new(mock_inner, "r".to_string(), 0, DetailLevel::Minimal, None);

        let exchange = Exchange::new(Message::default());
        let outcome = tokio::time::timeout(Duration::from_secs(5), async {
            tracing_proc.ready().await.unwrap().call(exchange).await
        })
        .await
        .expect("deadlock: TracingProcessor re-readied a clone whose permit was dropped");

        assert!(outcome.is_ok());
    }

    #[tokio::test]
    async fn tracing_processor_reusable_across_sequential_cycles() {
        let mock_inner = BoxProcessor::new(PermitGateInner::new());
        let mut tracing_proc =
            TracingProcessor::new(mock_inner, "r".to_string(), 0, DetailLevel::Minimal, None);

        let ex_a = Exchange::new(Message::default());
        let outcome_a = tokio::time::timeout(Duration::from_secs(5), async {
            tracing_proc.ready().await.unwrap().call(ex_a).await
        })
        .await
        .expect("first cycle timed out");
        assert!(outcome_a.is_ok());

        let ex_b = Exchange::new(Message::default());
        let outcome_b = tokio::time::timeout(Duration::from_secs(5), async {
            tracing_proc.ready().await.unwrap().call(ex_b).await
        })
        .await
        .expect("second cycle timed out");
        assert!(outcome_b.is_ok());
    }
}
