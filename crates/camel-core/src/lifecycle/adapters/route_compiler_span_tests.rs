use super::*;
use crate::shared::observability::adapters::span_test_util::{finish, test_spans};
use camel_api::fragment_exchange;
use camel_api::{ExceptionPolicy, OutcomePipeline, OutcomeSegment, RedeliveryPolicy};
use opentelemetry::baggage::BaggageExt;
use opentelemetry::trace::{Span, SpanId, Status, TraceContextExt, Tracer};
use opentelemetry::{Context as OtelContext, KeyValue, global};
use std::sync::atomic::AtomicUsize;

/// A pass-through step: IdentityProcessor wrapped as a CompiledStep.
fn identity_step() -> CompiledStep {
    CompiledStep::Process {
        processor: BoxProcessor::new(IdentityProcessor),
        body_contract: None,
        lifecycle: None,
    }
}

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

#[tokio::test]
async fn traced_pipeline_opens_root_span_with_step_children() {
    let spans = test_spans().await;
    let mut pipeline = compose_traced_pipeline(
        vec![identity_step(), identity_step()],
        "rt",
        true,
        DetailLevel::Minimal,
        None,
        None,
        PipelineRuntimeCtx::compile_time(),
    );
    let exchange = Exchange::new(Message::default());
    pipeline
        .ready()
        .await
        .expect("pipeline ready")
        .call(exchange)
        .await
        .expect("pipeline call succeeds");

    let all = finish(spans);
    let root_trace_id = all
        .iter()
        .find(|s| s.name == "rt")
        .map(|s| s.span_context.trace_id())
        .expect("root span exported");
    let in_trace: Vec<_> = all
        .iter()
        .filter(|s| s.span_context.trace_id() == root_trace_id)
        .collect();
    let root = in_trace
        .iter()
        .find(|s| s.name == "rt")
        .expect("root span exported");
    assert_eq!(
        root.parent_span_id,
        SpanId::INVALID,
        "route root span must have no parent"
    );
    let trace_id = root.span_context.trace_id();
    let root_span_id = root.span_context.span_id();
    let steps: Vec<_> = all
        .iter()
        .filter(|s| s.span_context.trace_id() == trace_id && s.name.starts_with("rt:step-"))
        .collect();
    assert_eq!(steps.len(), 2, "both step spans exported");
    for step in steps {
        assert_eq!(step.parent_span_id, root_span_id, "step parented by root");
        assert!(
            step.start_time > root.start_time,
            "step must start after the root span starts"
        );
        assert!(
            step.end_time < root.end_time,
            "step must end before the root span ends"
        );
    }
}

#[tokio::test]
async fn traced_pipeline_nested_entry_roots_under_caller() {
    let spans = test_spans().await;
    let tracer = global::tracer("camel-core-test");
    let caller = tracer.span_builder("caller").start(&tracer);
    let trace_id = caller.span_context().trace_id();
    let caller_span_id = caller.span_context().span_id();
    let mut exchange = Exchange::new(Message::default());
    exchange.otel_context = OtelContext::current_with_span(caller);

    let mut pipeline = compose_traced_pipeline(
        vec![identity_step()],
        "rt",
        true,
        DetailLevel::Minimal,
        None,
        None,
        PipelineRuntimeCtx::compile_time(),
    );
    pipeline
        .ready()
        .await
        .expect("pipeline ready")
        .call(exchange)
        .await
        .expect("pipeline call succeeds");

    let all = finish(spans);
    let root = all
        .into_iter()
        .filter(|s| s.span_context.trace_id() == trace_id)
        .find(|s| s.name == "rt")
        .expect("root span exported under caller trace");
    assert_eq!(
        root.parent_span_id, caller_span_id,
        "route root must nest under the caller span"
    );
}

#[tokio::test]
async fn traced_pipeline_restores_entry_context() {
    let _spans = test_spans().await;
    let tracer = global::tracer("camel-core-test");
    let caller = tracer.span_builder("caller").start(&tracer);
    let caller_span_id = caller.span_context().span_id();
    let mut exchange = Exchange::new(Message::default());
    exchange.otel_context =
        OtelContext::current_with_span(caller).with_baggage([KeyValue::new("outer", "1")]);

    let mut pipeline = compose_traced_pipeline(
        vec![identity_step()],
        "rt",
        true,
        DetailLevel::Minimal,
        None,
        None,
        PipelineRuntimeCtx::compile_time(),
    );
    let ex = pipeline
        .ready()
        .await
        .expect("pipeline ready")
        .call(exchange)
        .await
        .expect("pipeline call succeeds");

    assert_eq!(
        ex.otel_context.span().span_context().span_id(),
        caller_span_id,
        "returned exchange must restore the caller's active span"
    );
    assert!(
        ex.otel_context.baggage().get("outer").is_some(),
        "entry-context baggage must survive the route"
    );
}

#[tokio::test]
async fn empty_traced_route_opens_and_closes_root() {
    let spans = test_spans().await;
    let tracer = global::tracer("camel-core-test");
    let entry = tracer.span_builder("e").start(&tracer);
    let trace_id = entry.span_context().trace_id();
    let entry_span_id = entry.span_context().span_id();
    let mut exchange = Exchange::new(Message::default());
    exchange.otel_context =
        OtelContext::current_with_span(entry).with_baggage([KeyValue::new("keep", "1")]);

    let mut pipeline = compose_traced_pipeline(
        vec![],
        "empty",
        true,
        DetailLevel::Minimal,
        None,
        None,
        PipelineRuntimeCtx::compile_time(),
    );
    let ex = pipeline
        .ready()
        .await
        .expect("pipeline ready")
        .call(exchange)
        .await
        .expect("empty traced route completes");

    assert_eq!(
        ex.otel_context.span().span_context().span_id(),
        entry_span_id,
        "returned exchange must restore the entry span"
    );
    assert!(
        ex.otel_context.baggage().get("keep").is_some(),
        "entry-context baggage must survive the empty route"
    );

    let all = finish(spans);
    let in_trace: Vec<_> = all
        .into_iter()
        .filter(|s| s.span_context.trace_id() == trace_id)
        .collect();
    let root = in_trace
        .iter()
        .find(|s| s.name == "empty")
        .expect("root span exported for empty traced route");
    assert_eq!(
        root.parent_span_id, entry_span_id,
        "empty route root nests under the entry span"
    );
    assert!(
        matches!(root.status, Status::Ok),
        "empty route root status must be Ok"
    );
    assert_eq!(
        in_trace.len(),
        1,
        "empty route root must have zero child spans"
    );
}

#[tokio::test]
async fn traced_pipeline_failed_root_records_exception() {
    let spans = test_spans().await;
    let mut pipeline = compose_traced_pipeline(
        vec![CompiledStep::Process {
            processor: BoxProcessor::new(ErrProcessor),
            body_contract: None,
            lifecycle: None,
        }],
        "rt",
        true,
        DetailLevel::Minimal,
        None,
        None,
        PipelineRuntimeCtx::compile_time(),
    );
    let exchange = Exchange::new(Message::default());
    let result = pipeline
        .ready()
        .await
        .expect("pipeline ready")
        .call(exchange)
        .await;
    assert!(result.is_err(), "error must propagate to the caller");

    let all = finish(spans);
    let root_trace_id = all
        .iter()
        .find(|s| s.name == "rt")
        .map(|s| s.span_context.trace_id())
        .expect("root span exported");
    let in_trace: Vec<_> = all
        .iter()
        .filter(|s| s.span_context.trace_id() == root_trace_id)
        .collect();
    let root = in_trace
        .iter()
        .find(|s| s.name == "rt")
        .expect("root span exported");
    assert!(
        matches!(root.status, Status::Error { .. }),
        "root span status must be Error"
    );
    assert_eq!(root.events.len(), 1, "exactly one event on the root span");
    let event = &root.events[0];
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
}

// ── Segment step spans (trace-model-tree T1.4) ──

/// A `CompiledStep::Segment` wrapping the given segment double.
fn segment_step(segment: OutcomeSegment) -> CompiledStep {
    CompiledStep::Segment {
        segment,
        body_contract: None,
        lifecycle: None,
    }
}

/// Segment double that completes with the incoming exchange untouched.
#[derive(Clone)]
struct CompleteSegment;

impl OutcomePipeline for CompleteSegment {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(CompleteSegment)
    }

    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        Box::pin(async move { PipelineOutcome::Completed(exchange) })
    }
}

/// Segment double that always fails with a processor error.
#[derive(Clone)]
struct FailSegment;

impl OutcomePipeline for FailSegment {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(FailSegment)
    }

    fn run<'a>(
        &'a mut self,
        _exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        Box::pin(async move { PipelineOutcome::Failed(CamelError::ProcessorError("boom".into())) })
    }
}

/// Segment double that fails the first attempt; on the second attempt it
/// records the active span id, runs a fragment through a locally composed
/// traced sub-pipeline (`srt-sub`), then completes.
#[derive(Clone)]
struct RetrySubrouteSegment {
    calls: Arc<AtomicUsize>,
    second_attempt_span: Arc<Mutex<Option<SpanId>>>,
}

impl OutcomePipeline for RetrySubrouteSegment {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        Box::pin(async move {
            let attempt = self.calls.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                return PipelineOutcome::Failed(CamelError::ProcessorError("attempt-1".into()));
            }
            *self.second_attempt_span.lock().expect("span id lock") =
                Some(exchange.otel_context.span().span_context().span_id());
            let fragment = fragment_exchange(&exchange, Body::Text("frag".into()));
            let mut sub = compose_traced_pipeline(
                vec![identity_step()],
                "srt-sub",
                true,
                DetailLevel::Minimal,
                None,
                None,
                PipelineRuntimeCtx::compile_time(),
            );
            sub.ready()
                .await
                .expect("sub-pipeline ready")
                .call(fragment)
                .await
                .expect("sub-pipeline completes");
            PipelineOutcome::Completed(exchange)
        })
    }
}

#[tokio::test]
async fn segment_step_opens_span_and_restores_root() {
    let spans = test_spans().await;
    let mut pipeline = compose_traced_pipeline(
        vec![
            segment_step(OutcomeSegment::new(Box::new(CompleteSegment))),
            identity_step(),
        ],
        "srt",
        true,
        DetailLevel::Minimal,
        None,
        None,
        PipelineRuntimeCtx::compile_time(),
    );
    let exchange = Exchange::new(Message::default());
    pipeline
        .ready()
        .await
        .expect("pipeline ready")
        .call(exchange)
        .await
        .expect("pipeline call succeeds");

    let all = finish(spans);
    let root_trace_id = all
        .iter()
        .find(|s| s.name == "srt")
        .map(|s| s.span_context.trace_id())
        .expect("root span exported");
    let in_trace: Vec<_> = all
        .into_iter()
        .filter(|s| s.span_context.trace_id() == root_trace_id)
        .collect();
    let root = in_trace
        .iter()
        .find(|s| s.name == "srt")
        .expect("root span exported");
    let root_span_id = root.span_context.span_id();
    let segment_span = in_trace
        .iter()
        .find(|s| s.name == "srt:step-0")
        .expect("segment attempt span exported");
    assert_eq!(
        segment_span.parent_span_id, root_span_id,
        "segment step span must be parented by the route root"
    );
    let next_step = in_trace
        .iter()
        .find(|s| s.name == "srt:step-1")
        .expect("following step span exported");
    assert_eq!(
        next_step.parent_span_id, root_span_id,
        "step after the segment must chain onto the route root, not the segment span"
    );
}

#[tokio::test]
async fn segment_failure_records_exception_event() {
    let spans = test_spans().await;
    let mut pipeline = compose_traced_pipeline(
        vec![segment_step(OutcomeSegment::new(Box::new(FailSegment)))],
        "srt",
        true,
        DetailLevel::Minimal,
        None,
        None,
        PipelineRuntimeCtx::compile_time(),
    );
    let result = pipeline
        .ready()
        .await
        .expect("pipeline ready")
        .call(Exchange::new(Message::default()))
        .await;
    assert!(result.is_err(), "segment failure must propagate");

    let all = finish(spans);
    let root_trace_id = all
        .iter()
        .find(|s| s.name == "srt")
        .map(|s| s.span_context.trace_id())
        .expect("root span exported");
    let in_trace: Vec<_> = all
        .into_iter()
        .filter(|s| s.span_context.trace_id() == root_trace_id)
        .collect();
    let step = in_trace
        .iter()
        .find(|s| s.name == "srt:step-0")
        .expect("segment attempt span exported");
    assert!(
        matches!(step.status, Status::Error { .. }),
        "failed attempt span status must be Error"
    );
    assert_eq!(
        step.events.len(),
        1,
        "exactly one event on the attempt span"
    );
    assert_eq!(step.events[0].name, "exception");
    let root = in_trace
        .iter()
        .find(|s| s.name == "srt")
        .expect("root span exported");
    assert!(
        matches!(root.status, Status::Error { .. }),
        "root span status must be Error"
    );
    assert_eq!(root.events.len(), 1, "root records its own exception event");
    assert_eq!(root.events[0].name, "exception");
}

#[tokio::test]
async fn segment_retry_attempts_each_get_span() {
    let spans = test_spans().await;
    let double = RetrySubrouteSegment {
        calls: Arc::new(AtomicUsize::new(0)),
        second_attempt_span: Arc::new(Mutex::new(None)),
    };
    let recorded_span_id = double.second_attempt_span.clone();

    let mut policy = ExceptionPolicy::new(|_| true);
    policy.retry = Some(RedeliveryPolicy::new(2).with_initial_delay(Duration::ZERO));
    let handler: Arc<dyn RouteErrorHandler> =
        Arc::new(DefaultRouteErrorHandler::new(None, vec![(policy, None)]));

    let mut pipeline = compose_traced_pipeline(
        vec![segment_step(OutcomeSegment::new(Box::new(double)))],
        "srt",
        true,
        DetailLevel::Minimal,
        None,
        Some(handler),
        PipelineRuntimeCtx::compile_time(),
    );
    pipeline
        .ready()
        .await
        .expect("pipeline ready")
        .call(Exchange::new(Message::default()))
        .await
        .expect("retry recovers on the second attempt");

    let all = finish(spans);
    let root_trace_id = all
        .iter()
        .find(|s| s.name == "srt")
        .map(|s| s.span_context.trace_id())
        .expect("root span exported");
    let in_trace: Vec<_> = all
        .into_iter()
        .filter(|s| s.span_context.trace_id() == root_trace_id)
        .collect();
    let root = in_trace
        .iter()
        .find(|s| s.name == "srt")
        .expect("root span exported");
    let root_span_id = root.span_context.span_id();

    let mut attempts: Vec<_> = in_trace.iter().filter(|s| s.name == "srt:step-0").collect();
    assert_eq!(
        attempts.len(),
        2,
        "exactly two attempt spans for the retried segment"
    );
    attempts.sort_by_key(|s| s.start_time);
    for attempt in &attempts {
        assert_eq!(
            attempt.parent_span_id, root_span_id,
            "every attempt span is parented by the route root"
        );
    }
    let second = attempts[1];
    let sub = in_trace
        .iter()
        .find(|s| s.name == "srt-sub")
        .expect("sub-route root span exported");
    assert_eq!(
        sub.parent_span_id,
        second.span_context.span_id(),
        "retry sub-route must nest under the retry attempt span"
    );
    assert_eq!(
        *recorded_span_id.lock().expect("span id lock"),
        Some(second.span_context.span_id()),
        "double's second attempt ran inside the second attempt span"
    );
}

#[tokio::test]
async fn untraced_segment_emits_no_span() {
    let spans = test_spans().await;
    let mut pipeline = compose_traced_pipeline(
        vec![segment_step(OutcomeSegment::new(Box::new(CompleteSegment)))],
        "srt",
        false,
        DetailLevel::Minimal,
        None,
        None,
        PipelineRuntimeCtx::compile_time(),
    );
    pipeline
        .ready()
        .await
        .expect("pipeline ready")
        .call(Exchange::new(Message::default()))
        .await
        .expect("pipeline call succeeds");

    let all = finish(spans);
    assert!(
        all.iter().all(|s| s.name != "srt:step-0"),
        "untraced segment step must not emit a span"
    );
}
