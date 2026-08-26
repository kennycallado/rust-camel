use super::*;
use camel_api::error_handler::{BoundaryKind, PolicyId, RetryOutcome, StepDisposition};
use camel_api::{Body, BoxProcessorExt, CircuitBreakerConfig, Message, Value};
use camel_processor::RouteErrorHandler;
use camel_processor::error_handler::DefaultRouteErrorHandler;
use serde_json::json;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tower::ServiceExt;

fn make_test_exchange() -> Exchange {
    Exchange::new(Message::new("test"))
}

/// Test double for RouteErrorHandler that returns Continued disposition.
struct ContinuedHandler;

#[async_trait::async_trait]
impl RouteErrorHandler for ContinuedHandler {
    fn match_policy(&self, _: &CamelError) -> Option<PolicyId> {
        Some(PolicyId(0))
    }

    async fn retry_step(
        &self,
        _: Option<PolicyId>,
        _: &mut dyn camel_api::error_handler::RetryableStep,
        original: Exchange,
        error: CamelError,
    ) -> RetryOutcome {
        RetryOutcome::Exhausted {
            exchange: original,
            error,
            policy: Some(PolicyId(0)),
        }
    }

    async fn handle_step(
        &self,
        _: Option<PolicyId>,
        mut ex: Exchange,
        _: CamelError,
    ) -> Result<StepDisposition, CamelError> {
        ex.clear_error();
        Ok(StepDisposition::Continued(ex))
    }

    async fn handle_boundary(
        &self,
        _: BoundaryKind,
        ex: Exchange,
        _: CamelError,
    ) -> Result<Exchange, CamelError> {
        Ok(ex)
    }
}

/// Test double for RouteErrorHandler that returns Propagate disposition.
struct PropagateHandler;

#[async_trait::async_trait]
impl RouteErrorHandler for PropagateHandler {
    fn match_policy(&self, _: &CamelError) -> Option<PolicyId> {
        None
    }

    async fn retry_step(
        &self,
        _: Option<PolicyId>,
        _: &mut dyn camel_api::error_handler::RetryableStep,
        original: Exchange,
        error: CamelError,
    ) -> RetryOutcome {
        RetryOutcome::Exhausted {
            exchange: original,
            error,
            policy: None,
        }
    }

    async fn handle_step(
        &self,
        _: Option<PolicyId>,
        _ex: Exchange,
        error: CamelError,
    ) -> Result<StepDisposition, CamelError> {
        Ok(StepDisposition::Propagate(error))
    }

    async fn handle_boundary(
        &self,
        _: BoundaryKind,
        ex: Exchange,
        _: CamelError,
    ) -> Result<Exchange, CamelError> {
        Ok(ex)
    }
}

/// A service that returns `Pending` on the first `poll_ready`, then `Ready`.
#[derive(Clone)]
struct DelayedReadyService {
    ready: Arc<AtomicBool>,
}

impl Service<Exchange> for DelayedReadyService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if self.ready.fetch_or(true, Ordering::SeqCst) {
            Poll::Ready(Ok(()))
        } else {
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }

    fn call(&mut self, ex: Exchange) -> Self::Future {
        Box::pin(async move { Ok(ex) })
    }
}

#[test]
fn test_pipeline_poll_ready_delegates_to_first_step() {
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);

    let inner = DelayedReadyService {
        ready: Arc::new(AtomicBool::new(false)),
    };
    let boxed = BoxProcessor::new(inner);
    let mut pipeline = SequentialPipeline {
        steps: SharedSnapshot(Arc::from(vec![CompiledStep::Process {
            processor: boxed,
            body_contract: None,
            lifecycle: None,
        }])),
        handler: None,
        ctx: PipelineRuntimeCtx::compile_time(),
    };

    let first = pipeline.poll_ready(&mut cx);
    assert!(first.is_pending(), "expected Pending on first poll_ready");

    let second = pipeline.poll_ready(&mut cx);
    assert!(second.is_ready(), "expected Ready on second poll_ready");
}

#[test]
fn test_pipeline_poll_ready_with_empty_steps() {
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);

    let mut pipeline = SequentialPipeline {
        steps: SharedSnapshot(Arc::from(vec![])),
        handler: None,
        ctx: PipelineRuntimeCtx::compile_time(),
    };
    let result = pipeline.poll_ready(&mut cx);
    assert!(result.is_ready(), "expected Ready for empty pipeline");
}

#[tokio::test]
async fn test_pipeline_stop_returns_ok_with_exchange() {
    let stop_step = CompiledStep::Stop;
    let after_called = Arc::new(AtomicBool::new(false));
    let after_called_clone = after_called.clone();
    let after_step = CompiledStep::Process {
        processor: BoxProcessor::from_fn(move |ex| {
            after_called_clone.store(true, Ordering::SeqCst);
            Box::pin(async move { Ok(ex) })
        }),
        body_contract: None,
        lifecycle: None,
    };

    let mut pipeline = SequentialPipeline {
        steps: SharedSnapshot(Arc::from(vec![stop_step, after_step])),
        handler: None,
        ctx: PipelineRuntimeCtx::compile_time(),
    };

    let ex = Exchange::new(camel_api::Message::new("hello"));
    let result = pipeline.call(ex).await;
    // Pipeline-level result is Ok(ex) — Stop arrives as success (ADR-0024).
    assert!(result.is_ok(), "expected Ok, got: {:?}", result);
    assert_eq!(result.unwrap().input.body.as_text(), Some("hello"));
    assert!(
        !after_called.load(Ordering::SeqCst),
        "step after stop should not be called"
    );
}

#[tokio::test]
async fn test_run_steps_stop_produces_pipeline_outcome_stopped() {
    use camel_api::PipelineOutcome;
    // A two-step pipeline where the first step is a Stop marker.
    let steps = vec![
        CompiledStep::Stop,
        CompiledStep::Process {
            processor: BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) })),
            body_contract: None,
            lifecycle: None,
        },
    ];
    let ex = Exchange::new(camel_api::Message::new("payload"));
    let outcome = run_steps(
        SharedSnapshot(Arc::from(steps)),
        ex,
        None,
        false,
        "",
        &PipelineRuntimeCtx::compile_time(),
    )
    .await;
    match outcome {
        PipelineOutcome::Stopped(returned) => {
            assert_eq!(returned.input.body.as_text(), Some("payload"));
        }
        other => panic!(
            "expected PipelineOutcome::Stopped, got {:?}",
            other.is_success()
        ),
    }
}

#[tokio::test]
async fn test_run_steps_stop_bypasses_error_handler() {
    use camel_api::PipelineOutcome;
    use camel_api::error_handler::{BoundaryKind, PolicyId, RetryOutcome, StepDisposition};
    use camel_processor::RouteErrorHandler;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let handler_invocations = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&handler_invocations);

    // Handler that records every call. NONE of its methods should be invoked for Stop.
    struct RecordingHandler {
        counter: Arc<AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl RouteErrorHandler for RecordingHandler {
        fn match_policy(&self, _err: &CamelError) -> Option<PolicyId> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            None
        }
        async fn retry_step(
            &self,
            _policy: Option<PolicyId>,
            _step: &mut dyn camel_api::error_handler::RetryableStep,
            _original: Exchange,
            _error: CamelError,
        ) -> RetryOutcome {
            self.counter.fetch_add(1, Ordering::SeqCst);
            unreachable!("retry_step must not be called for CompiledStep::Stop")
        }
        async fn handle_step(
            &self,
            _policy: Option<PolicyId>,
            _exchange: Exchange,
            _error: CamelError,
        ) -> Result<StepDisposition, CamelError> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            unreachable!("handle_step must not be called for CompiledStep::Stop")
        }
        async fn handle_boundary(
            &self,
            _kind: BoundaryKind,
            _exchange: Exchange,
            _error: CamelError,
        ) -> Result<Exchange, CamelError> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            unreachable!("handle_boundary must not be called for CompiledStep::Stop")
        }
    }

    let steps = vec![CompiledStep::Stop];
    let ex = Exchange::new(camel_api::Message::new("payload"));
    let outcome = run_steps(
        SharedSnapshot(Arc::from(steps)),
        ex,
        Some(Arc::new(RecordingHandler { counter })),
        false,
        "",
        &PipelineRuntimeCtx::compile_time(),
    )
    .await;

    assert!(matches!(outcome, PipelineOutcome::Stopped(_)));
    assert_eq!(
        handler_invocations.load(Ordering::SeqCst),
        0,
        "error handler MUST NOT be invoked for CompiledStep::Stop"
    );
}

#[tokio::test]
async fn test_compose_traced_pipeline_disabled() {
    let pipeline = compose_traced_pipeline(
        vec![],
        "test-route",
        false,
        DetailLevel::Minimal,
        None,
        None,
        PipelineRuntimeCtx::compile_time(),
    );
    let ex = Exchange::new(camel_api::Message::new("hello"));
    let result = tower::ServiceExt::oneshot(pipeline, ex).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_compose_traced_pipeline_enabled() {
    let step = BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }));
    let pipeline = compose_traced_pipeline(
        vec![CompiledStep::Process {
            processor: step,
            body_contract: None,
            lifecycle: None,
        }],
        "test-route",
        true,
        DetailLevel::Minimal,
        None,
        None,
        PipelineRuntimeCtx::compile_time(),
    );
    let ex = Exchange::new(camel_api::Message::new("hello"));
    let result = tower::ServiceExt::oneshot(pipeline, ex).await;
    assert!(result.is_ok());
}

/// TracedPipeline route-root-span tests (trace-model-tree T1.3).
///
/// Span-exporting tests use `span_test_util`: ONE global in-memory provider
/// per test binary, test bodies serialized by a process-wide mutex, spans
/// filtered by the trace id each test set up itself.
mod traced_pipeline_span_tests {
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
            Box::pin(
                async move { PipelineOutcome::Failed(CamelError::ProcessorError("boom".into())) },
            )
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
}

#[tokio::test]
async fn test_compose_pipeline_with_contracts_coerces_before_inner_processor() {
    let seen_body = Arc::new(Mutex::new(None::<Body>));
    let seen_body_clone = Arc::clone(&seen_body);

    let inner = BoxProcessor::from_fn(move |ex: Exchange| {
        let seen_body_clone = Arc::clone(&seen_body_clone);
        Box::pin(async move {
            *seen_body_clone.lock().expect("lock seen body") = Some(ex.input.body.clone());
            Ok(ex)
        })
    });

    let pipeline = compose_pipeline_with_contracts(
        vec![CompiledStep::Process {
            processor: inner,
            body_contract: Some(camel_api::BodyType::Text),
            lifecycle: None,
        }],
        None,
        PipelineRuntimeCtx::compile_time(),
    );

    let mut ex = Exchange::new(Message::default());
    ex.input.body = Body::Json(json!("hello"));

    let result = tower::ServiceExt::oneshot(pipeline, ex).await;
    assert!(result.is_ok());

    let observed = seen_body.lock().expect("lock seen body").clone();
    assert_eq!(observed, Some(Body::Text("hello".to_string())));
}

#[tokio::test]
async fn test_run_steps_continued_skips_failed_step() {
    let step1 = CompiledStep::Process {
        processor: BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) })),
        body_contract: None,
        lifecycle: None,
    };
    let step2 = CompiledStep::Process {
        processor: BoxProcessor::from_fn(|_ex| {
            Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
        }),
        body_contract: None,
        lifecycle: None,
    };
    let step3_hit = Arc::new(AtomicBool::new(false));
    let hit = step3_hit.clone();
    let step3 = CompiledStep::Process {
        processor: BoxProcessor::from_fn(move |ex| {
            let hit = hit.clone();
            Box::pin(async move {
                hit.store(true, Ordering::SeqCst);
                Ok(ex)
            })
        }),
        body_contract: None,
        lifecycle: None,
    };

    let handler: Arc<dyn RouteErrorHandler> = Arc::new(ContinuedHandler);
    let outcome = run_steps(
        SharedSnapshot(Arc::from([step1, step2, step3])),
        make_test_exchange(),
        Some(handler),
        false,
        "",
        &PipelineRuntimeCtx::compile_time(),
    )
    .await;
    assert!(
        matches!(outcome, PipelineOutcome::Completed(_)),
        "expected Completed, got: {:?}",
        outcome.is_success()
    );
    assert!(
        step3_hit.load(Ordering::SeqCst),
        "step 3 should have executed after continued"
    );
}

#[tokio::test]
async fn test_run_steps_failed_without_handler_returns_failed() {
    // Optimized path: a failing step + handler=None → short-circuit to
    // PipelineOutcome::Failed without attempting retry/recovery.
    let steps = vec![CompiledStep::Process {
        processor: BoxProcessor::from_fn(|_ex| {
            Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
        }),
        body_contract: None,
        lifecycle: None,
    }];
    let ex = Exchange::new(camel_api::Message::new("payload"));
    let outcome = run_steps(
        SharedSnapshot(Arc::from(steps)),
        ex,
        None,
        false,
        "",
        &PipelineRuntimeCtx::compile_time(),
    )
    .await;
    match outcome {
        PipelineOutcome::Failed(err) => {
            assert!(
                matches!(&err, CamelError::ProcessorError(msg) if msg == "boom"),
                "expected ProcessorError('boom'), got {:?}",
                err
            );
        }
        other => panic!(
            "expected PipelineOutcome::Failed, got: {:?}",
            other.is_success()
        ),
    }
}

// ── RouteChannelService tests ─────────────────────────────────────────

#[tokio::test]
async fn test_route_channel_pipeline_propagate_returns_err() {
    let handler: Arc<dyn RouteErrorHandler> = Arc::new(PropagateHandler);
    let failing_step = BoxProcessor::from_fn(|_ex| {
        Box::pin(async { Err(CamelError::ProcessorError("step boom".into())) })
    });
    let pipeline = compose_pipeline_with_handler(
        vec![CompiledStep::Process {
            processor: failing_step,
            body_contract: None,
            lifecycle: None,
        }],
        Some(handler.clone()),
        PipelineRuntimeCtx::compile_time(),
    );
    let channel = RouteChannelService::new(handler.clone(), None, None, pipeline, false);
    let mut svc = BoxProcessor::new(channel);
    let result = svc.ready().await.unwrap().call(make_test_exchange()).await;
    assert!(result.is_err(), "Propagate should return Err");
}

#[tokio::test]
async fn test_route_channel_security_error_calls_boundary() {
    let handler: Arc<dyn RouteErrorHandler> = Arc::new(PropagateHandler);
    let deny_all = BoxProcessor::from_fn(|_ex| {
        Box::pin(async { Err(CamelError::Unauthorized("denied".into())) })
    });
    let pipeline = compose_pipeline_with_handler(
        vec![],
        Some(handler.clone()),
        PipelineRuntimeCtx::compile_time(),
    );
    let channel = RouteChannelService::new(handler.clone(), Some(deny_all), None, pipeline, false);
    let mut svc = BoxProcessor::new(channel);
    let result = svc.ready().await.unwrap().call(make_test_exchange()).await;
    assert!(
        result.is_ok(),
        "boundary errors should be absorbed by handler"
    );
}

#[tokio::test]
async fn test_route_channel_cb_reject_calls_boundary() {
    let handler: Arc<dyn RouteErrorHandler> = Arc::new(PropagateHandler);
    let cb_gate = CircuitBreakerGate::new(CircuitBreakerConfig {
        failure_threshold: 1,
        open_duration: Duration::from_secs(60),
        success_threshold: 1,
        fallback: None,
    });
    cb_gate.after_result(&Err(CamelError::ProcessorError("force open".into())));
    let pipeline = compose_pipeline_with_handler(
        vec![],
        Some(handler.clone()),
        PipelineRuntimeCtx::compile_time(),
    );
    let channel = RouteChannelService::new(handler.clone(), None, Some(cb_gate), pipeline, false);
    let mut svc = BoxProcessor::new(channel);
    let result = svc.ready().await.unwrap().call(make_test_exchange()).await;
    assert!(
        result.is_ok(),
        "CB reject should call handle_boundary and return Ok"
    );
}

#[tokio::test]
async fn test_route_channel_cb_fallback_executes_fallback() {
    let handler: Arc<dyn RouteErrorHandler> = Arc::new(PropagateHandler);
    let fallback = BoxProcessor::from_fn(|mut ex| {
        Box::pin(async move {
            ex.input.set_header("from_fallback", Value::Bool(true));
            Ok(ex)
        })
    });
    let cb_gate = CircuitBreakerGate::new(CircuitBreakerConfig {
        failure_threshold: 1,
        open_duration: Duration::from_secs(60),
        success_threshold: 1,
        fallback: Some(fallback),
    });
    cb_gate.after_result(&Err(CamelError::ProcessorError("force open".into())));
    let pipeline = compose_pipeline_with_handler(
        vec![],
        Some(handler.clone()),
        PipelineRuntimeCtx::compile_time(),
    );
    let channel = RouteChannelService::new(handler.clone(), None, Some(cb_gate), pipeline, false);
    let mut svc = BoxProcessor::new(channel);
    let result = svc.ready().await.unwrap().call(make_test_exchange()).await;
    assert!(result.is_ok(), "fallback should succeed");
    assert_eq!(
        result.unwrap().input.header("from_fallback"),
        Some(&Value::Bool(true)),
        "should have executed fallback processor",
    );
}

#[tokio::test]
async fn test_route_channel_cb_fallback_failure_calls_boundary() {
    // CRITICAL: fallback failure must go through handle_boundary, NOT raw Err to upstream.
    let handler: Arc<dyn RouteErrorHandler> = Arc::new(PropagateHandler);
    let failing_fallback = BoxProcessor::from_fn(|_ex| {
        Box::pin(async { Err(CamelError::ProcessorError("fallback broken".into())) })
    });
    let cb_gate = CircuitBreakerGate::new(CircuitBreakerConfig {
        failure_threshold: 1,
        open_duration: Duration::from_secs(60),
        success_threshold: 1,
        fallback: Some(failing_fallback),
    });
    cb_gate.after_result(&Err(CamelError::ProcessorError("force open".into())));

    let pipeline = compose_pipeline_with_handler(
        vec![],
        Some(handler.clone()),
        PipelineRuntimeCtx::compile_time(),
    );
    let channel = RouteChannelService::new(handler.clone(), None, Some(cb_gate), pipeline, false);

    let mut svc = BoxProcessor::new(channel);
    let result = svc.ready().await.unwrap().call(make_test_exchange()).await;
    // PropagateHandler.handle_boundary returns Ok(ex) — so fallback failure is absorbed
    assert!(
        result.is_ok(),
        "fallback failure should go through handle_boundary, not raw Err"
    );
}

#[tokio::test]
async fn test_route_channel_cb_counts_stopped_as_success() {
    // ADR-0024 §3.5: PipelineOutcome::Stopped translates to Ok(ex) at the
    // Tower boundary. RouteChannelService::call invokes cb.after_result(&result)
    // where result = Ok(ex) for Stop. The CB must NOT trip.
    let handler: Arc<dyn RouteErrorHandler> = Arc::new(PropagateHandler);
    let cb_gate = CircuitBreakerGate::new(CircuitBreakerConfig {
        failure_threshold: 2,
        open_duration: Duration::from_secs(60),
        success_threshold: 1,
        fallback: None,
    });
    let cb_clone = cb_gate.clone();

    // Pipeline emits Stop as the only step — top-level maps Stop to Ok(ex).
    let pipeline = compose_pipeline_with_handler(
        vec![CompiledStep::Stop],
        None,
        PipelineRuntimeCtx::compile_time(),
    );

    let channel = RouteChannelService::new(handler, None, Some(cb_gate), pipeline, false);

    // Two Stop invocations — would trip a 2-failure CB if Stop counted as failure.
    let ex1 = Exchange::new(camel_api::Message::new("a"));
    let ex2 = Exchange::new(camel_api::Message::new("b"));
    let r1 = tower::ServiceExt::oneshot(channel.clone(), ex1).await;
    let r2 = tower::ServiceExt::oneshot(channel, ex2).await;
    assert!(r1.is_ok(), "Stop must arrive as Ok via RouteChannelService");
    assert!(r2.is_ok(), "Stop must arrive as Ok via RouteChannelService");

    // CB must still be in Allow state — Stop counted as success.
    assert!(
        matches!(cb_clone.before_call(), CircuitBreakerDecision::Allow),
        "CB should count Stop as success"
    );
}

// ── use_original_message full-path integration test (N5) ──

#[tokio::test]
async fn test_use_original_message_stash_survives_full_route_channel() {
    // Full path: RouteChannelService stashes original message, pipeline mutates
    // then fails, DefaultRouteErrorHandler restores before DLC.
    use std::sync::Mutex;

    let dlc_received = Arc::new(Mutex::new(None::<Exchange>));
    let dlc_received_clone = Arc::clone(&dlc_received);
    let dlc = BoxProcessor::from_fn(move |ex: Exchange| {
        let r = Arc::clone(&dlc_received_clone);
        Box::pin(async move {
            *r.lock().unwrap() = Some(ex.clone());
            Ok(ex)
        })
    });

    let mut handler = DefaultRouteErrorHandler::new(Some(dlc), vec![]);
    handler.use_original_message = true;
    let handler: Arc<dyn RouteErrorHandler> = Arc::new(handler);

    // Pipeline: step that mutates body, then step that fails.
    let mutating_step = BoxProcessor::from_fn(|mut ex: Exchange| {
        Box::pin(async move {
            ex.input.body = Body::Bytes("mutated".into());
            Ok(ex)
        })
    });
    let failing_step = BoxProcessor::from_fn(|_ex: Exchange| {
        Box::pin(async { Err::<Exchange, CamelError>(CamelError::ProcessorError("boom".into())) })
    });

    let pipeline = compose_pipeline_with_handler(
        vec![
            CompiledStep::Process {
                processor: mutating_step,
                body_contract: None,
                lifecycle: None,
            },
            CompiledStep::Process {
                processor: failing_step,
                body_contract: None,
                lifecycle: None,
            },
        ],
        Some(handler.clone()),
        PipelineRuntimeCtx::compile_time(),
    );

    let channel = RouteChannelService::new(handler, None, None, pipeline, true);

    let ex = Exchange::new(Message::new("original-body"));
    let result = tower::ServiceExt::oneshot(channel, ex).await;

    // Pipeline error should propagate (no policy configured, no on_steps, disposition=Propagate).
    assert!(
        result.is_err(),
        "RouteChannelService should propagate error when no policy matches"
    );

    // But the DLC must have been called with the ORIGINAL body (pre-mutation).
    let received = dlc_received
        .lock()
        .unwrap()
        .take()
        .expect("DLC should have been called");
    let received_text = match &received.input.body {
        camel_api::Body::Text(s) => s.clone(),
        camel_api::Body::Bytes(b) => String::from_utf8_lossy(b).to_string(),
        camel_api::Body::Json(v) => v.to_string(),
        _ => String::new(),
    };
    assert_eq!(
        received_text, "original-body",
        "DLC should receive the pre-route original body, not the mutated version"
    );
}

#[tokio::test]
async fn test_use_original_message_wholesale_exchange_replacement() {
    // When a step returns a brand-new Exchange (not the original one), the stash
    // extension is LOST because extensions live on the original Exchange object.
    // This test documents the limitation: use_original_message only works when
    // steps mutate the existing Exchange in place.
    use std::sync::Mutex;

    let dlc_received = Arc::new(Mutex::new(None::<Exchange>));
    let dlc_received_clone = Arc::clone(&dlc_received);
    let dlc = BoxProcessor::from_fn(move |ex: Exchange| {
        let r = Arc::clone(&dlc_received_clone);
        Box::pin(async move {
            *r.lock().unwrap() = Some(ex.clone());
            Ok(ex)
        })
    });

    let mut handler = DefaultRouteErrorHandler::new(Some(dlc), vec![]);
    handler.use_original_message = true;
    let handler: Arc<dyn RouteErrorHandler> = Arc::new(handler);

    // Step that returns a BRAND-NEW Exchange (wholesale replacement).
    let replace_step = BoxProcessor::from_fn(|_ex: Exchange| {
        Box::pin(async { Ok(Exchange::new(Message::new("new-body"))) })
    });
    let failing_step = BoxProcessor::from_fn(|_ex: Exchange| {
        Box::pin(async { Err::<Exchange, CamelError>(CamelError::ProcessorError("boom".into())) })
    });

    let pipeline = compose_pipeline_with_handler(
        vec![
            CompiledStep::Process {
                processor: replace_step,
                body_contract: None,
                lifecycle: None,
            },
            CompiledStep::Process {
                processor: failing_step,
                body_contract: None,
                lifecycle: None,
            },
        ],
        Some(handler.clone()),
        PipelineRuntimeCtx::compile_time(),
    );

    let channel = RouteChannelService::new(handler, None, None, pipeline, true);

    let ex = Exchange::new(Message::new("original-body"));
    let result = tower::ServiceExt::oneshot(channel, ex).await;

    // Error propagates (no policy configured).
    assert!(
        result.is_err(),
        "RouteChannelService should propagate error when no policy matches"
    );

    // Because the stash was on the original Exchange (now gone), the DLC gets
    // the new Exchange's body — not the original.
    let received = dlc_received
        .lock()
        .unwrap()
        .take()
        .expect("DLC should have been called");
    let received_text = match &received.input.body {
        camel_api::Body::Text(s) => s.clone(),
        camel_api::Body::Bytes(b) => String::from_utf8_lossy(b).to_string(),
        camel_api::Body::Json(v) => v.to_string(),
        _ => String::new(),
    };
    assert_eq!(
        received_text, "new-body",
        "When a step replaces the Exchange wholesale, use_original_message cannot \
         restore the pre-route Message — the stash lives on the original Exchange"
    );
}

// ── CamelStop drop signal tests ──
//
// Verifies that the CamelStop property is honored at all executor boundaries:
// - run_steps (Process mode: SamplingService, ThrottlerService)
// - BoxProcessorSegment (legacy Tower processor wrapped as OutcomePipeline)
// - SequentialOutcomeSegment (defensive check after child Completed)

#[tokio::test]
async fn test_sampling_drop_stops_following_process_step() {
    // Route: sampling(period=2) → process(set captured=true)
    // Exchange 1 (counter=1, 1%2≠0): CamelStop=true → executor stops → "captured" NOT set
    // Exchange 2 (counter=2, 2%2=0): passes through → "captured" IS set
    use camel_api::PipelineOutcome;
    use camel_processor::SamplingService;

    let captured = Arc::new(AtomicBool::new(false));
    let captured1 = captured.clone();

    let steps = vec![
        CompiledStep::Process {
            processor: BoxProcessor::new(SamplingService::new(2)),
            body_contract: None,
            lifecycle: None,
        },
        CompiledStep::Process {
            processor: BoxProcessor::from_fn(move |mut ex: Exchange| {
                let cap = captured1.clone();
                Box::pin(async move {
                    cap.store(true, Ordering::SeqCst);
                    ex.set_property("captured", Value::Bool(true));
                    Ok(ex)
                })
            }),
            body_contract: None,
            lifecycle: None,
        },
    ];

    // Exchange 1 — should be stopped by sampling (counter=1, 1%2≠0)
    captured.store(false, Ordering::SeqCst);
    let ex1 = Exchange::new(Message::new("first"));
    let outcome1 = run_steps(
        SharedSnapshot(Arc::from(steps.clone())),
        ex1,
        None,
        false,
        "",
        &PipelineRuntimeCtx::compile_time(),
    )
    .await;
    match &outcome1 {
        PipelineOutcome::Stopped(returned) => {
            assert!(
                camel_api::is_camel_stop(returned),
                "dropped exchange must have CamelStop property set"
            );
            assert!(
                !captured.load(Ordering::SeqCst),
                "dropped exchange must NOT reach the step after sampling"
            );
        }
        other => panic!("exchange 1 should be Stopped, got {:?}", other),
    }

    // Exchange 2 — should pass through (counter=2, 2%2=0)
    captured.store(false, Ordering::SeqCst);
    let ex2 = Exchange::new(Message::new("second"));
    let outcome2 = run_steps(
        SharedSnapshot(Arc::from(steps.clone())),
        ex2,
        None,
        false,
        "",
        &PipelineRuntimeCtx::compile_time(),
    )
    .await;
    match &outcome2 {
        PipelineOutcome::Completed(returned) => {
            assert!(
                !camel_api::is_camel_stop(returned),
                "passing exchange must NOT have CamelStop"
            );
            assert!(
                captured.load(Ordering::SeqCst),
                "passing exchange MUST reach the step after sampling"
            );
        }
        other => panic!("exchange 2 should be Completed, got {:?}", other),
    }
}

#[tokio::test]
async fn test_sampling_drop_inside_box_processor_segment_stops_sibling() {
    // Verifies BoxProcessorSegment checks is_camel_stop after Tower call.
    // Segment sequence: [BoxProcessorSegment(SamplingService(period=2)), BoxProcessorSegment(marker)]
    use camel_api::{OutcomePipeline, PipelineOutcome};
    use camel_processor::SamplingService;

    let captured = Arc::new(AtomicBool::new(false));
    let captured1 = captured.clone();

    let sampling_seg = BoxProcessorSegment::new(BoxProcessor::new(SamplingService::new(2)));
    let marker_seg = BoxProcessorSegment::new(BoxProcessor::from_fn(move |mut ex: Exchange| {
        let cap = captured1.clone();
        Box::pin(async move {
            cap.store(true, Ordering::SeqCst);
            ex.set_property("captured", Value::Bool(true));
            Ok(ex)
        })
    }));

    let children: Vec<Box<dyn OutcomePipeline>> =
        vec![Box::new(sampling_seg), Box::new(marker_seg)];
    let mut seq =
        crate::lifecycle::adapters::outcome_composition::SequentialOutcomeSegment::new(children);

    // Exchange 1 — should be stopped at sampling
    captured.store(false, Ordering::SeqCst);
    let ex1 = Exchange::new(Message::new("first"));
    let outcome1 = seq.run(ex1).await;
    match &outcome1 {
        PipelineOutcome::Stopped(returned) => {
            assert!(
                camel_api::is_camel_stop(returned),
                "dropped exchange must have CamelStop property set"
            );
            assert!(
                !captured.load(Ordering::SeqCst),
                "marker step must NOT execute after drop"
            );
        }
        other => panic!("exchange 1 should be Stopped, got {:?}", other),
    }
}

#[cfg(test)]
mod compose_outcome_segment_tests {
    use super::*;
    use camel_api::{Exchange, Message, PipelineOutcome};

    #[tokio::test]
    async fn compose_outcome_segment_with_empty_returns_identity_noop() {
        let mut seg = compose_outcome_segment(vec![]);
        let ex = Exchange::new(Message::new("hello"));
        let outcome = seg.run(ex).await;
        assert!(matches!(outcome, PipelineOutcome::Completed(_)));
    }
}

#[cfg(test)]
mod run_steps_segment_tests {
    use super::*;
    use camel_api::error_handler::PolicyId;
    use camel_api::{Exchange, Message, OutcomePipeline, OutcomeSegment, PipelineOutcome};
    use std::future::Future;
    use std::pin::Pin;

    #[derive(Clone)]
    struct StoppedSegment;

    impl OutcomePipeline for StoppedSegment {
        fn clone_box(&self) -> Box<dyn OutcomePipeline> {
            Box::new(StoppedSegment)
        }

        fn run<'a>(
            &'a mut self,
            mut exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            Box::pin(async move {
                exchange.input.body =
                    camel_api::Body::Bytes(b"mutated-before-stop".to_vec().into());
                PipelineOutcome::Stopped(exchange)
            })
        }
    }

    #[tokio::test]
    async fn run_steps_segment_stop_preserves_exchange_mutations() {
        let seg = OutcomeSegment::new(Box::new(StoppedSegment));
        let steps = vec![CompiledStep::Segment {
            segment: seg,
            body_contract: None,
            lifecycle: None,
        }];
        let ex = Exchange::new(Message::new("original"));
        let outcome = run_steps(
            SharedSnapshot(Arc::from(steps)),
            ex,
            None,
            false,
            "",
            &PipelineRuntimeCtx::compile_time(),
        )
        .await;
        match outcome {
            PipelineOutcome::Stopped(returned_ex) => {
                if let camel_api::Body::Bytes(b) = &returned_ex.input.body {
                    assert_eq!(
                        b.as_ref(),
                        b"mutated-before-stop",
                        "BUG: Stopped exchange dropped mutations from inside nested block"
                    );
                } else {
                    panic!("expected Bytes body, got {:?}", returned_ex.input.body);
                }
            }
            other => panic!("expected Stopped, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn run_steps_segment_failed_invokes_handler_retry() {
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
                Box::pin(async move {
                    PipelineOutcome::Failed(CamelError::ProcessorError("fail".into()))
                })
            }
        }

        use camel_api::error_handler::BoundaryKind;
        struct FailThroughHandler;
        #[async_trait::async_trait]
        impl RouteErrorHandler for FailThroughHandler {
            fn match_policy(&self, _: &CamelError) -> Option<PolicyId> {
                None
            }
            async fn retry_step(
                &self,
                _: Option<PolicyId>,
                _: &mut dyn camel_api::error_handler::RetryableStep,
                original: Exchange,
                error: CamelError,
            ) -> RetryOutcome {
                RetryOutcome::Exhausted {
                    exchange: original,
                    error,
                    policy: None,
                }
            }
            async fn handle_step(
                &self,
                _: Option<PolicyId>,
                _ex: Exchange,
                error: CamelError,
            ) -> Result<StepDisposition, CamelError> {
                Ok(StepDisposition::Propagate(error))
            }
            async fn handle_boundary(
                &self,
                _: BoundaryKind,
                ex: Exchange,
                _: CamelError,
            ) -> Result<Exchange, CamelError> {
                Ok(ex)
            }
        }

        let handler: Arc<dyn RouteErrorHandler> = Arc::new(FailThroughHandler);
        let seg = OutcomeSegment::new(Box::new(FailSegment));
        let steps = vec![CompiledStep::Segment {
            segment: seg,
            body_contract: None,
            lifecycle: None,
        }];
        let ex = Exchange::new(Message::new("hello"));
        let outcome = run_steps(
            SharedSnapshot(Arc::from(steps)),
            ex,
            Some(handler),
            false,
            "",
            &PipelineRuntimeCtx::compile_time(),
        )
        .await;
        assert!(
            matches!(outcome, PipelineOutcome::Failed(_)),
            "expected Failed, got {:?}",
            outcome
        );
    }
}

#[cfg(test)]
mod body_coercing_segment_tests {
    use super::*;
    use camel_api::{BodyType, Exchange, Message, OutcomePipeline, PipelineOutcome};
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct SourceSegment {
        emitted: Arc<Mutex<Vec<u8>>>,
    }
    impl OutcomePipeline for SourceSegment {
        fn clone_box(&self) -> Box<dyn OutcomePipeline> {
            Box::new(self.clone())
        }
        fn run<'a>(
            &'a mut self,
            mut ex: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            let emitted = self.emitted.clone();
            Box::pin(async move {
                let bytes = match &ex.input.body {
                    camel_api::Body::Bytes(b) => b.as_ref().to_vec(),
                    _ => Vec::new(),
                };
                *emitted.lock().expect("emitted mutex not poisoned") = bytes.clone();
                ex.input.body =
                    camel_api::Body::Bytes([bytes, b"-coerced".to_vec()].concat().into());
                PipelineOutcome::Completed(ex)
            })
        }
    }

    #[tokio::test]
    async fn body_coercing_segment_runs_coercion_before_inner() {
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let inner = SourceSegment {
            emitted: emitted.clone(),
        };
        let contract = BodyType::Bytes;
        let mut seg = BodyCoercingSegment::new(Box::new(inner), contract);
        let ex = Exchange::new(Message::new("payload"));
        let outcome = seg.run(ex).await;
        match outcome {
            PipelineOutcome::Completed(_) => {
                let received = emitted.lock().expect("emitted mutex not poisoned").clone();
                assert!(
                    !received.is_empty(),
                    "inner segment should have seen body bytes"
                );
            }
            other => panic!("expected Completed, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn body_coercing_segment_propagates_stopped() {
        #[derive(Clone)]
        struct StoppingSegment;
        impl OutcomePipeline for StoppingSegment {
            fn clone_box(&self) -> Box<dyn OutcomePipeline> {
                Box::new(StoppingSegment)
            }
            fn run<'a>(
                &'a mut self,
                ex: Exchange,
            ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
                Box::pin(async move { PipelineOutcome::Stopped(ex) })
            }
        }
        let mut seg = BodyCoercingSegment::new(Box::new(StoppingSegment), BodyType::Text);
        let ex = Exchange::new(Message::new("payload"));
        let outcome = seg.run(ex).await;
        assert!(matches!(outcome, PipelineOutcome::Stopped(_)));
    }
}

#[cfg(test)]
mod sequential_outcome_segment_tests {
    use super::*;
    use camel_api::{Exchange, Message, OutcomePipeline, PipelineOutcome};
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone)]
    struct Counter {
        n: Arc<AtomicUsize>,
        add: usize,
        order: Arc<Mutex<Vec<usize>>>,
    }

    impl OutcomePipeline for Counter {
        fn clone_box(&self) -> Box<dyn OutcomePipeline> {
            Box::new(self.clone())
        }
        fn run<'a>(
            &'a mut self,
            mut exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            let n = self.n.clone();
            let add = self.add;
            let order = self.order.clone();
            Box::pin(async move {
                n.fetch_add(add, Ordering::SeqCst);
                order.lock().expect("order mutex not poisoned").push(add);
                exchange.input.body = camel_api::Body::Bytes(
                    format!("count={}", n.load(Ordering::SeqCst))
                        .into_bytes()
                        .into(),
                );
                PipelineOutcome::Completed(exchange)
            })
        }
    }

    #[tokio::test]
    async fn sequential_outcome_segment_runs_children_in_order() {
        let n = Arc::new(AtomicUsize::new(0));
        let order = Arc::new(Mutex::new(Vec::new()));
        let children: Vec<Box<dyn OutcomePipeline>> = vec![
            Box::new(Counter {
                n: n.clone(),
                add: 1,
                order: order.clone(),
            }),
            Box::new(Counter {
                n: n.clone(),
                add: 10,
                order: order.clone(),
            }),
            Box::new(Counter {
                n: n.clone(),
                add: 100,
                order: order.clone(),
            }),
        ];
        let mut seg = camel_api::OutcomeSegment::new(Box::new(
            crate::lifecycle::adapters::outcome_composition::SequentialOutcomeSegment::new(
                children,
            ),
        ));
        let ex = Exchange::new(Message::new("start"));
        let outcome = seg.run(ex).await;
        assert!(matches!(outcome, PipelineOutcome::Completed(_)));
        assert_eq!(n.load(Ordering::SeqCst), 111);
        let recorded = order.lock().expect("order mutex not poisoned").clone();
        assert_eq!(
            recorded,
            vec![1, 10, 100],
            "children must execute in forward order; got {:?}",
            recorded
        );
    }
}

// ── B1 cancellation tests ──
//
// These tests were moved from crates/camel-core/tests/cancellation_between_steps.rs
// as part of fixing CANCEL_TOKEN visibility to pub(crate) — integration tests at
// `tests/` cannot access pub(crate) items.

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

    fn pass_through() -> CompiledStep {
        CompiledStep::Process {
            processor: BoxProcessor::from_fn(|ex: Exchange| Box::pin(async move { Ok(ex) })),
            body_contract: None,
            lifecycle: None,
        }
    }

    #[tokio::test]
    async fn cancelled_pipeline_returns_consumer_stopping() {
        let cancel = CancellationToken::new();
        let mut pipeline = compose_pipeline(
            (0..3).map(|_| pass_through()).collect(),
            PipelineRuntimeCtx::compile_time(),
        );
        cancel.cancel();
        // Wrap in CANCEL_TOKEN scope — simulates pipeline task.
        let result = CANCEL_TOKEN
            .scope(cancel, async {
                pipeline.call(Exchange::new(Message::new("hello"))).await
            })
            .await;
        assert!(
            matches!(result, Err(ref e) if matches!(e, CamelError::ConsumerStopping)),
            "expected ConsumerStopping, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn cancel_mid_pipeline_stops_at_next_boundary() {
        let cancel = CancellationToken::new();
        let cancel_in_step = cancel.clone();

        let step1 = CompiledStep::Process {
            processor: BoxProcessor::from_fn(move |ex: Exchange| {
                let c = cancel_in_step.clone();
                Box::pin(async move {
                    c.cancel();
                    Ok(ex)
                })
            }),
            body_contract: None,
            lifecycle: None,
        };

        let mut pipeline = compose_pipeline(
            vec![pass_through(), step1, pass_through()],
            PipelineRuntimeCtx::compile_time(),
        );
        let result = CANCEL_TOKEN
            .scope(cancel, async {
                pipeline.call(Exchange::new(Message::new("hello"))).await
            })
            .await;
        assert!(
            matches!(result, Err(ref e) if matches!(e, CamelError::ConsumerStopping)),
            "expected ConsumerStopping after mid-pipeline cancel, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn no_cancel_token_skips_check() {
        // Without CANCEL_TOKEN scope, run_steps should NOT check cancellation.
        // Exchanges process normally even if a token somewhere is cancelled.
        let mut pipeline = compose_pipeline(
            (0..3).map(|_| pass_through()).collect(),
            PipelineRuntimeCtx::compile_time(),
        );
        // No CANCEL_TOKEN.scope wrapper — task-local absent.
        let result = pipeline.call(Exchange::new(Message::new("hello"))).await;
        assert!(
            result.is_ok(),
            "without task-local, exchange should complete normally, got: {result:?}"
        );
    }
}

/// Documents that SharedSnapshot is Send + Sync.
///
/// `CompiledStep` is `Send + Sync` by construction: `BoxProcessor` is
/// `BoxCloneSyncService`, so the standard `Arc` auto traits hold without
/// unsafe impls. The compile-time guard in the parent module asserts both
/// `CompiledStep: Send` and `CompiledStep: Sync`. This test pins the same
/// fact at the `SharedSnapshot` level.
#[test]
fn shared_snapshot_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SharedSnapshot>();
}
