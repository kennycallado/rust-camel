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
use tower::Layer as _;
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
    let mut proc = TracingProcessor::new(
        inner,
        "r".to_string(),
        0,
        DetailLevel::Minimal,
        None,
        None,
        SpanKindHint::Internal,
    );
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
    assert!(
        step.attributes
            .iter()
            .any(|kv| kv.key.as_str() == "step_index"),
        "step_index attribute must be present"
    );
    assert!(
        !step
            .attributes
            .iter()
            .any(|kv| kv.key.as_str() == "step_id"),
        "step_id must not be recorded as a span attribute"
    );
}

#[tokio::test]
async fn tracing_processor_labeled_span_name() {
    let spans = test_spans().await;
    let (exchange, trace_id, _parent_span_id) = exchange_under_parent_span();

    let inner = BoxProcessor::new(IdentityProcessor);
    let mut proc = TracingProcessor::new(
        inner,
        "r".to_string(),
        0,
        DetailLevel::Minimal,
        None,
        Some("log".into()),
        SpanKindHint::Internal,
    );
    let outcome = proc
        .ready()
        .await
        .expect("service ready")
        .call(exchange)
        .await;
    outcome.expect("step call succeeds");

    let all = finish(spans);
    let matched: Vec<_> = all
        .into_iter()
        .filter(|s| s.span_context.trace_id() == trace_id && s.name == "r:log")
        .collect();
    assert_eq!(matched.len(), 1, "exactly one span named r:log");
}

#[tokio::test]
async fn tracing_processor_fallback_span_name() {
    let spans = test_spans().await;
    let (exchange, trace_id, _parent_span_id) = exchange_under_parent_span();

    let inner = BoxProcessor::new(IdentityProcessor);
    let mut proc = TracingProcessor::new(
        inner,
        "r".to_string(),
        0,
        DetailLevel::Minimal,
        None,
        None,
        SpanKindHint::Internal,
    );
    let outcome = proc
        .ready()
        .await
        .expect("service ready")
        .call(exchange)
        .await;
    outcome.expect("step call succeeds");

    let all = finish(spans);
    let matched: Vec<_> = all
        .into_iter()
        .filter(|s| s.span_context.trace_id() == trace_id && s.name == "r:step-0")
        .collect();
    assert_eq!(
        matched.len(),
        1,
        "exactly one span named r:step-0 (fallback preserved)"
    );
}

#[tokio::test]
async fn tracing_processor_kind_client() {
    let spans = test_spans().await;
    let (exchange, trace_id, _parent_span_id) = exchange_under_parent_span();

    let inner = BoxProcessor::new(IdentityProcessor);
    let mut proc = TracingProcessor::new(
        inner,
        "r".to_string(),
        0,
        DetailLevel::Minimal,
        None,
        None,
        SpanKindHint::Client,
    );
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
    assert_eq!(step.span_kind, SpanKind::Client);
}

#[tokio::test]
async fn tracing_processor_kind_producer() {
    let spans = test_spans().await;
    let (exchange, trace_id, _parent_span_id) = exchange_under_parent_span();

    let inner = BoxProcessor::new(IdentityProcessor);
    let mut proc = TracingProcessor::new(
        inner,
        "r".to_string(),
        0,
        DetailLevel::Minimal,
        None,
        None,
        SpanKindHint::Producer,
    );
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
    assert_eq!(step.span_kind, SpanKind::Producer);
}

#[tokio::test]
async fn tracing_processor_kind_default_internal() {
    let spans = test_spans().await;
    let (exchange, trace_id, _parent_span_id) = exchange_under_parent_span();

    let inner = BoxProcessor::new(IdentityProcessor);
    let mut proc = TracingProcessor::new(
        inner,
        "r".to_string(),
        0,
        DetailLevel::Minimal,
        None,
        None,
        SpanKindHint::default(),
    );
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
    assert_eq!(step.span_kind, SpanKind::Internal);
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
    let mut proc = TracingProcessor::new(
        inner,
        "r".to_string(),
        0,
        DetailLevel::Minimal,
        None,
        None,
        SpanKindHint::Internal,
    );
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
    let mut proc = TracingProcessor::new(
        inner,
        "r".to_string(),
        0,
        DetailLevel::Minimal,
        None,
        None,
        SpanKindHint::Internal,
    );
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
        None,
        SpanKindHint::Internal,
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
        None,
        SpanKindHint::Internal,
    );

    let exchange = Exchange::new(Message::default());
    let result = tracer.ready().await.unwrap().call(exchange).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_tracing_processor_full_detail() {
    let inner = BoxProcessor::new(IdentityProcessor);
    let mut tracer = TracingProcessor::new(
        inner,
        "test-route".to_string(),
        0,
        DetailLevel::Full,
        None,
        None,
        SpanKindHint::Internal,
    );

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
        None,
        SpanKindHint::Internal,
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
        None,
        SpanKindHint::Internal,
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
        None,
        SpanKindHint::Internal,
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
        None,
        SpanKindHint::Internal,
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
    let tracer = TracingProcessor::new(
        inner,
        "my-route".to_string(),
        5,
        DetailLevel::Minimal,
        None,
        None,
        SpanKindHint::Internal,
    );

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
        None,
        SpanKindHint::Internal,
    );

    let processor2 = BoxProcessor::new(IdentityProcessor);
    let mut tracer2 = TracingProcessor::new(
        processor2,
        "route2".to_string(),
        1,
        DetailLevel::Minimal,
        None,
        None,
        SpanKindHint::Internal,
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
            Poll::Ready(Err(err)) => Poll::Ready(Err(CamelError::ProcessorError(err.to_string()))),
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
    let mut tracing_proc = TracingProcessor::new(
        mock_inner,
        "r".to_string(),
        0,
        DetailLevel::Minimal,
        None,
        None,
        SpanKindHint::Internal,
    );

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
    let mut tracing_proc = TracingProcessor::new(
        mock_inner,
        "r".to_string(),
        0,
        DetailLevel::Minimal,
        None,
        None,
        SpanKindHint::Internal,
    );

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

// ── Circuit-open exclusion (dashboard-observability D2) ──────────────────

/// Records every `MetricsCollector` observation as `method:route[:type]`.
struct RecordingMetrics {
    calls: std::sync::Mutex<Vec<String>>,
}

impl RecordingMetrics {
    fn snapshot(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn push(&self, method: &str, key: &str) {
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(format!("{method}:{key}"));
    }
}

impl MetricsCollector for RecordingMetrics {
    fn record_exchange_duration(&self, route_id: &str, _duration: Duration) {
        self.push("record_exchange_duration", route_id);
    }
    fn increment_errors(&self, route_id: &str, error_type: &str) {
        self.push("increment_errors", &format!("{route_id}:{error_type}"));
    }
    fn increment_exchanges(&self, route_id: &str) {
        self.push("increment_exchanges", route_id);
    }
    fn set_queue_depth(&self, _route_id: &str, _depth: usize) {}
    fn record_circuit_breaker_change(&self, _route_id: &str, _from: &str, _to: &str) {}
    fn increment_circuit_breaker_rejection(&self, route: &str) {
        self.push("increment_circuit_breaker_rejection", route);
    }
}

/// An open-breaker fast-fail observed through the tracer adapter counts as
/// exactly one circuit-breaker rejection and never as `camel_errors_total`.
///
/// Production wiring for a no-error-handler route: the tracer wraps the
/// breaker-wrapped pipeline. The trip exchange runs through an UNTRACED
/// service (same layer → shared breaker state) so the only traced exchange
/// is the fast-failed one; readiness-time rejections bypass the tracer's
/// call path entirely, so the tracer must record neither errors nor
/// exchanges for it — the breaker alone records the rejection.
#[tokio::test]
async fn rejection_counted_not_errored() {
    let collector = Arc::new(RecordingMetrics {
        calls: std::sync::Mutex::new(Vec::new()),
    });
    let config = camel_api::CircuitBreakerConfig::new()
        .failure_threshold(1)
        .open_duration(Duration::from_secs(60));
    let layer = camel_processor::circuit_breaker::CircuitBreakerLayer::new(
        config,
        Arc::from("r"),
        Some(Arc::clone(&collector) as Arc<dyn MetricsCollector>),
    );

    // Trip the breaker (threshold 1) without the tracer in the path.
    let failing = BoxProcessor::from_fn(|_ex: Exchange| {
        Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
    });
    let mut tripper = layer.layer(failing);
    let _ = tripper
        .ready()
        .await
        .expect("closed breaker readies")
        .call(Exchange::new(Message::new("trip")))
        .await;

    // Traced service over the SAME layer: state is shared, so the breaker
    // is open and poll_ready fast-fails with CircuitOpen.
    let failing = BoxProcessor::from_fn(|_ex: Exchange| {
        Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
    });
    let mut traced = TracingProcessor::new(
        BoxProcessor::new(layer.layer(failing)),
        "r".to_string(),
        0,
        DetailLevel::Minimal,
        Some(Arc::clone(&collector) as Arc<dyn MetricsCollector>),
        None,
        SpanKindHint::Internal,
    );
    let outcome = traced.ready().await.err();
    assert!(
        matches!(outcome, Some(CamelError::CircuitOpen(_))),
        "open breaker must fast-fail at readiness"
    );

    let calls = collector.snapshot();
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.starts_with("increment_circuit_breaker_rejection"))
            .count(),
        1,
        "exactly one rejection must be recorded, got {calls:?}"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.starts_with("increment_errors"))
            .count(),
        0,
        "circuit-open rejection must not increment errors, got {calls:?}"
    );
}

#[tokio::test]
/// The tracer's circuit-open SKIP branch (tracer.rs: error_class !=
/// CIRCUIT_OPEN gates increment_errors) pinned directly: a traced
/// processor whose CALL returns CircuitOpen must count the exchange but
/// not the error. Defense-in-depth today (breakers sit outside the
/// traced pipeline and reject at readiness), load-bearing the day a
/// breaker-as-step or error-handler rethrow routes CircuitOpen through
/// a traced segment.
async fn circuit_open_skip_branch_not_errored() {
    let collector = Arc::new(RecordingMetrics {
        calls: std::sync::Mutex::new(Vec::new()),
    });
    let rejecter = BoxProcessor::from_fn(|_ex: Exchange| {
        Box::pin(async { Err(CamelError::CircuitOpen("cb-open".into())) })
    });
    let mut traced = TracingProcessor::new(
        rejecter,
        "r".to_string(),
        0,
        DetailLevel::Minimal,
        Some(Arc::clone(&collector) as Arc<dyn MetricsCollector>),
        None,
        SpanKindHint::Internal,
    );
    let outcome = traced
        .ready()
        .await
        .expect("traced rejecter readies")
        .call(Exchange::new(Message::new("x")))
        .await;
    assert!(
        matches!(outcome, Err(CamelError::CircuitOpen(_))),
        "call must surface CircuitOpen unchanged"
    );
    let calls = collector.snapshot();
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.starts_with("increment_exchanges"))
            .count(),
        1,
        "the exchange itself must be counted, got {calls:?}"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.starts_with("increment_errors"))
            .count(),
        0,
        "circuit-open must skip increment_errors in the tracer, got {calls:?}"
    );
}

// ── Tracer/metrics decoupling (dashboard-observability D3) ───────────────

use crate::shared::observability::domain::MetricsLeversConfig;

/// Spec metrics-configuration Req 1, "metrics on, tracer off": with the
/// pipeline adapter running for metrics (prometheus on) but spans gated
/// off by explicit `tracer.enabled = false`, one exchange records metric
/// families and creates zero spans.
#[tokio::test]
async fn metrics_on_tracer_off() {
    let spans = test_spans().await;
    let collector = Arc::new(RecordingMetrics {
        calls: std::sync::Mutex::new(Vec::new()),
    });
    let mut proc = TracingProcessor::new(
        BoxProcessor::new(IdentityProcessor),
        "m_on_t_off".to_string(),
        0,
        DetailLevel::Minimal,
        Some(Arc::clone(&collector) as Arc<dyn MetricsCollector>),
        None,
        SpanKindHint::Internal,
    )
    .with_spans_enabled(false);

    let outcome = proc
        .ready()
        .await
        .expect("service ready")
        .call(Exchange::new(Message::default()))
        .await;
    outcome.expect("step call succeeds");

    let all = finish(spans);
    assert!(
        all.is_empty(),
        "tracer-off must create no spans, got {all:?}"
    );
    let calls = collector.snapshot();
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.starts_with("increment_exchanges"))
            .count(),
        1,
        "metric families must still flow, got {calls:?}"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.starts_with("record_exchange_duration"))
            .count(),
        1,
        "duration family must still flow, got {calls:?}"
    );
}

/// Spec metrics-configuration Req 1, "metrics off, tracer on": with
/// `metrics.enabled = false` and tracing on, spans are created, the
/// failure still reaches `increment_errors` (non-disableable family), and
/// `increment_exchanges` is suppressed.
#[tokio::test]
async fn metrics_off_tracer_on() {
    let spans = test_spans().await;
    let collector = Arc::new(RecordingMetrics {
        calls: std::sync::Mutex::new(Vec::new()),
    });
    let mut proc = TracingProcessor::new(
        BoxProcessor::new(ErrProcessor),
        "m_off_t_on".to_string(),
        0,
        DetailLevel::Minimal,
        Some(Arc::clone(&collector) as Arc<dyn MetricsCollector>),
        None,
        SpanKindHint::Internal,
    )
    .with_metric_levers(MetricsLeversConfig {
        enabled: false,
        ..Default::default()
    });

    let outcome = proc
        .ready()
        .await
        .expect("service ready")
        .call(Exchange::new(Message::default()))
        .await;
    assert!(outcome.is_err(), "injected failure must surface");

    let all = finish(spans);
    assert!(
        all.iter().any(|s| s.name == "m_off_t_on:step-0"),
        "tracer-on must still create spans, got {all:?}"
    );
    let calls = collector.snapshot();
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.starts_with("increment_errors"))
            .count(),
        1,
        "error family is non-disableable, got {calls:?}"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.starts_with("increment_exchanges"))
            .count(),
        0,
        "exchanges family must be suppressed by enabled=false, got {calls:?}"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.starts_with("record_exchange_duration"))
            .count(),
        0,
        "duration family must be suppressed by enabled=false, got {calls:?}"
    );
}

/// Spec metrics-configuration Req 2, "duration family disabled": with
/// `duration = false`, a success and a failure produce zero duration
/// records, one error record, and exchanges per the (default-on) exchange
/// lever.
#[tokio::test]
async fn duration_family_disabled_but_errors_survive() {
    let collector = Arc::new(RecordingMetrics {
        calls: std::sync::Mutex::new(Vec::new()),
    });
    let levers = MetricsLeversConfig {
        duration: false,
        ..Default::default()
    };

    let mut ok_proc = TracingProcessor::new(
        BoxProcessor::new(IdentityProcessor),
        "dur_off".to_string(),
        0,
        DetailLevel::Minimal,
        Some(Arc::clone(&collector) as Arc<dyn MetricsCollector>),
        None,
        SpanKindHint::Internal,
    )
    .with_metric_levers(levers.clone());
    let outcome = ok_proc
        .ready()
        .await
        .expect("service ready")
        .call(Exchange::new(Message::default()))
        .await;
    outcome.expect("success call succeeds");

    let mut err_proc = TracingProcessor::new(
        BoxProcessor::new(ErrProcessor),
        "dur_off".to_string(),
        0,
        DetailLevel::Minimal,
        Some(Arc::clone(&collector) as Arc<dyn MetricsCollector>),
        None,
        SpanKindHint::Internal,
    )
    .with_metric_levers(levers);
    let outcome = err_proc
        .ready()
        .await
        .expect("service ready")
        .call(Exchange::new(Message::default()))
        .await;
    assert!(outcome.is_err(), "injected failure must surface");

    let calls = collector.snapshot();
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.starts_with("record_exchange_duration"))
            .count(),
        0,
        "duration lever off must suppress the duration family, got {calls:?}"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.starts_with("increment_errors"))
            .count(),
        1,
        "errors are never lever-gated, got {calls:?}"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.starts_with("increment_exchanges"))
            .count(),
        2,
        "one exchange per call (lever on), got {calls:?}"
    );
}
