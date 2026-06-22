// adapters/route_compiler.rs
// Pipeline compilation functions: compose BuilderSteps into a Tower BoxProcessor.
// Tower types live here as this is the adapter layer responsible for
// translating declarative route definitions into executable pipelines.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::metrics::MetricsCollector;
use camel_api::{BoxProcessor, CamelError, Exchange, IdentityProcessor, PipelineOutcome};

use camel_api::error_handler::{BoundaryKind, RetryOutcome, StepDisposition};
use camel_processor::{
    CircuitBreakerDecision, CircuitBreakerGate, RouteErrorHandler, invoke_processor,
};
use tracing::Instrument;

use crate::lifecycle::adapters::body_coercing::wrap_if_needed;
use crate::lifecycle::adapters::step_compilers::CompiledStep;
use crate::shared::observability::adapters::TracingProcessor;
use crate::shared::observability::domain::DetailLevel;

/// Compose a list of CompiledSteps into a sub-pipeline (EIP internal).
///
/// Sub-pipelines preserve `Err(CamelError::Stopped)` so nested Stop
/// propagates to the outer `run_steps` bypass. Use [`compose_pipeline_with_handler`]
/// for the top-level consumer-facing pipeline.
pub fn compose_pipeline(processors: Vec<CompiledStep>) -> BoxProcessor {
    if processors.is_empty() {
        return BoxProcessor::new(IdentityProcessor);
    }
    BoxProcessor::new(SequentialPipeline {
        steps: processors,
        handler: None,
        flatten_stop: false,
    })
}

/// Compose a list of CompiledSteps with an optional route error handler.
///
/// When a handler is present, step readiness errors are swallowed (poll_ready
/// returns Ready) and the handler's retry/recovery logic is invoked on step
/// failures. Otherwise, step readiness errors propagate immediately.
pub fn compose_pipeline_with_handler(
    processors: Vec<CompiledStep>,
    handler: Option<Arc<dyn RouteErrorHandler>>,
) -> BoxProcessor {
    if processors.is_empty() {
        return BoxProcessor::new(IdentityProcessor);
    }
    BoxProcessor::new(SequentialPipeline {
        steps: processors,
        handler,
        flatten_stop: true, // top-level: flatten Stop to Ok(ex) — Bug B fix
    })
}

/// Compose a list of CompiledSteps into a traced pipeline.
///
/// Each processor is wrapped with TracingProcessor to emit spans for observability.
/// When tracing is disabled, falls back to plain compose_pipeline with zero overhead.
pub fn compose_traced_pipeline(
    processors: Vec<CompiledStep>,
    route_id: &str,
    trace_enabled: bool,
    detail_level: DetailLevel,
    metrics: Option<Arc<dyn MetricsCollector>>,
    handler: Option<Arc<dyn RouteErrorHandler>>,
) -> BoxProcessor {
    if !trace_enabled {
        return compose_pipeline_with_handler(processors, handler);
    }

    if processors.is_empty() {
        return BoxProcessor::new(IdentityProcessor);
    }

    let wrapped: Vec<CompiledStep> = processors
        .into_iter()
        .enumerate()
        .map(|(idx, step)| {
            let (p, c) = match step {
                CompiledStep::Process {
                    processor,
                    body_contract,
                } => (processor, body_contract),
                CompiledStep::Stop => return CompiledStep::Stop,
            };
            let traced = BoxProcessor::new(TracingProcessor::new(
                p,
                route_id.to_string(),
                idx,
                detail_level.clone(),
                metrics.clone(),
            ));
            CompiledStep::Process {
                processor: traced,
                body_contract: c,
            }
        })
        .collect();

    BoxProcessor::new(TracedPipeline {
        steps: wrapped,
        handler,
        flatten_stop: false, // sub-pipeline: preserve Err(Stopped) for EIP nesting
    })
}

/// Compose a list of `CompiledStep` items into a single pipeline with body coercion.
///
/// Each processor is optionally wrapped with [`BodyCoercingProcessor`] based on its
/// contract. Processors with `None` contract are passed through with zero overhead.
/// `CompiledStep::Stop` passes through without coercion.
pub fn compose_pipeline_with_contracts(
    processors: Vec<CompiledStep>,
    handler: Option<Arc<dyn RouteErrorHandler>>,
) -> BoxProcessor {
    let wrapped: Vec<CompiledStep> = processors
        .into_iter()
        .map(|step| match step {
            CompiledStep::Process {
                processor,
                body_contract,
            } => {
                let coerced = wrap_if_needed(processor, body_contract);
                CompiledStep::Process {
                    processor: coerced,
                    body_contract: None,
                }
            }
            CompiledStep::Stop => CompiledStep::Stop,
        })
        .collect();
    compose_pipeline_with_handler(wrapped, handler)
}

/// Compose a list of `CompiledStep` items into a traced pipeline with body coercion.
///
/// Applies body coercion contracts first, then wraps with `TracingProcessor`.
/// When tracing is disabled, falls back to [`compose_pipeline_with_contracts`].
pub(crate) fn compose_traced_pipeline_with_contracts(
    processors: Vec<CompiledStep>,
    route_id: &str,
    trace_enabled: bool,
    detail_level: DetailLevel,
    metrics: Option<Arc<dyn MetricsCollector>>,
    handler: Option<Arc<dyn RouteErrorHandler>>,
) -> BoxProcessor {
    if !trace_enabled {
        return compose_pipeline_with_contracts(processors, handler);
    }

    if processors.is_empty() {
        return BoxProcessor::new(IdentityProcessor);
    }

    let wrapped: Vec<CompiledStep> = processors
        .into_iter()
        .enumerate()
        .map(|(idx, step)| match step {
            CompiledStep::Process {
                processor,
                body_contract,
            } => {
                let coerced = wrap_if_needed(processor, body_contract);
                let traced = BoxProcessor::new(TracingProcessor::new(
                    coerced,
                    route_id.to_string(),
                    idx,
                    detail_level.clone(),
                    metrics.clone(),
                ));
                CompiledStep::Process {
                    processor: traced,
                    body_contract: None,
                }
            }
            CompiledStep::Stop => CompiledStep::Stop,
        })
        .collect();

    BoxProcessor::new(TracedPipeline {
        steps: wrapped,
        handler,
        flatten_stop: true, // top-level: flatten Stop to Ok(ex) — Bug B fix
    })
}

/// A service that executes a sequence of CompiledSteps in order.
///
/// When `flatten_stop` is true (top-level route pipeline), `Stopped(ex)` is
/// flattened to `Ok(ex)` via `into_tower_result()` — the Bug B fix that makes
/// Stop indistinguishable from Completed at the consumer boundary.
/// When `flatten_stop` is false (EIP sub-pipeline), `Stopped(ex)` remapped to
/// `Err(CamelError::Stopped)` so nested Stop propagates to the outer pipeline's
/// `run_steps` bypass (per e_gpt oracle Option E, 2026-06-22).
#[derive(Clone)]
struct SequentialPipeline {
    steps: Vec<CompiledStep>,
    handler: Option<Arc<dyn RouteErrorHandler>>,
    flatten_stop: bool,
}

impl Service<Exchange> for SequentialPipeline {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.steps.first() {
            Some(CompiledStep::Process { processor, .. }) => {
                let mut proc = processor.clone();
                match proc.poll_ready(cx) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(Err(_)) if self.handler.is_some() => Poll::Ready(Ok(())),
                    Poll::Ready(other) => Poll::Ready(other),
                }
            }
            Some(CompiledStep::Stop) => Poll::Ready(Ok(())),
            None => Poll::Ready(Ok(())),
        }
    }

    // ADR-0024 reply-channel adapter: PipelineOutcome → Result<Exchange, CamelError>.
    // This is the ONLY translation site. Completed(ex) and Stopped(ex) both map to
    // Ok(ex); Failed(err) maps to Err. Downstream consumers (RouteChannelService,
    // ExchangeUoWLayer, HTTP/Kafka reply finalisers) see Result<Exchange, CamelError>
    // and treat Stop as success — the core fix for Bug B.
    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let steps = self.steps.clone();
        let handler = self.handler.clone();
        let flatten = self.flatten_stop;
        Box::pin(async move {
            let outcome = run_steps(steps, exchange, handler, false).await;
            if flatten {
                outcome.into_tower_result()
            } else {
                eip_outcome_to_result(outcome)
            }
        })
    }
}

/// A traced service pipeline for wrapped CompiledSteps.
#[derive(Clone)]
struct TracedPipeline {
    steps: Vec<CompiledStep>,
    handler: Option<Arc<dyn RouteErrorHandler>>,
    flatten_stop: bool,
}

impl Service<Exchange> for TracedPipeline {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.steps.first() {
            Some(CompiledStep::Process { processor, .. }) => {
                let mut proc = processor.clone();
                match proc.poll_ready(cx) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(Err(_)) if self.handler.is_some() => Poll::Ready(Ok(())),
                    Poll::Ready(other) => Poll::Ready(other),
                }
            }
            Some(CompiledStep::Stop) => Poll::Ready(Ok(())),
            None => Poll::Ready(Ok(())),
        }
    }

    // ADR-0024 reply-channel adapter (same as SequentialPipeline::call):
    // Completed(ex) and Stopped(ex) both map to Ok(ex). Bug B fix.
    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let steps = self.steps.clone();
        let handler = self.handler.clone();
        let flatten = self.flatten_stop;
        Box::pin(async move {
            let outcome = run_steps(steps, exchange, handler, true).await;
            if flatten {
                outcome.into_tower_result()
            } else {
                eip_outcome_to_result(outcome)
            }
        })
    }
}

/// Translate a `PipelineOutcome` to `Result<Exchange, CamelError>` for the Tower
/// boundary of an EIP sub-pipeline. Unlike `into_tower_result` (which maps
/// `Stopped(ex)` → `Ok(ex)` for the top-level consumer reply), this preserves
/// `Err(CamelError::Stopped)` so nested Stop propagates to the outer `run_steps`
/// bypass in `stop_inside_filter_prevents_outer_pipeline` and similar EIP tests.
/// This is the internal sub-pipeline adapter — removed once EIPs become
/// outcome-aware (per e_gpt oracle Option E, 2026-06-22).
fn eip_outcome_to_result(outcome: PipelineOutcome) -> Result<Exchange, CamelError> {
    match outcome {
        PipelineOutcome::Completed(ex) => Ok(ex),
        PipelineOutcome::Stopped(_ex) => Err(CamelError::Stopped),
        PipelineOutcome::Failed(err) => Err(err),
    }
}

/// Run a sequence of CompiledSteps with optional error recovery.
///
/// Each CompiledStep::Process is executed via `invoke_processor`. On failure:
/// 1. If a handler is present, `match_policy` selects a retry policy.
/// 2. `retry_step` attempts recovery; if exhausted, `handle_step` determines
///    the disposition:
///    - `Propagate` — return the error
///    - `Handled` — return the exchange early (success)
///    - `Continued` — clear the error and continue to the next step
/// 3. If no handler is present, the error is propagated directly.
///
/// CompiledStep::Stop short-circuits to `PipelineOutcome::Stopped(ex)` — the
/// handler is bypassed and no Tower service is invoked (ADR-0024 §3.5).
pub async fn run_steps(
    steps: Vec<CompiledStep>,
    exchange: Exchange,
    handler: Option<Arc<dyn RouteErrorHandler>>,
    trace: bool,
) -> PipelineOutcome {
    use camel_api::PipelineOutcome;
    let mut ex = exchange;
    for (i, step) in steps.into_iter().enumerate() {
        let CompiledStep::Process { mut processor, .. } = step else {
            // CompiledStep::Stop — short-circuit to Stopped outcome WITHOUT
            // invoking a Tower service. The handler is bypassed (ADR-0024 §3.5).
            return PipelineOutcome::Stopped(ex);
        };
        let original = ex.clone();
        let invoke_future = invoke_processor(&mut processor, ex);
        let result = if trace {
            invoke_future
                .instrument(tracing::debug_span!("pipeline_step", index = i))
                .await
        } else {
            invoke_future.await
        };
        match result {
            Ok(next) => {
                ex = next;
            }
            Err(err) => {
                // INTERIM (e_gpt oracle Option E, 2026-06-22): Err(CamelError::Stopped)
                // comes from a nested sub-pipeline (CompiledStep::Stop was mapped to
                // StopService in the sub-pipeline compiler — see control_flow.rs et al).
                // The handler is bypassed; translate to PipelineOutcome::Stopped using
                // the Exchange from BEFORE the step ran. This preserves Apache Camel
                // nested-stop semantics (Stop inside filter/choice/loop/multicast/split
                // halts the entire route). Removed in Task 22 once EIPs are outcome-aware.
                if matches!(err, CamelError::Stopped) {
                    return PipelineOutcome::Stopped(original);
                }
                // NOTE: The CompiledStep::Stop at the top of the loop handles the
                // top-level Stop. The check above handles the DISTINCT case of
                // nested Stop propagating through Tower Response from a sub-pipeline.

                let Some(handler) = handler.as_ref() else {
                    return PipelineOutcome::Failed(err);
                };

                let policy = handler.match_policy(&err);
                match handler
                    .retry_step(policy, &mut processor, original, err)
                    .await
                {
                    RetryOutcome::Recovered(exchange) => {
                        ex = exchange;
                        continue;
                    }
                    RetryOutcome::Exhausted {
                        exchange,
                        error,
                        policy,
                    } => {
                        let handle_future = handler.handle_step(policy, exchange, error);
                        let disposition = if trace {
                            handle_future
                                .instrument(tracing::debug_span!("error_handler", step_index = i))
                                .await
                        } else {
                            handle_future.await
                        };
                        match disposition {
                            Ok(StepDisposition::Propagate(e)) => return PipelineOutcome::Failed(e),
                            Ok(StepDisposition::Handled(done)) => {
                                return PipelineOutcome::Completed(done);
                            }
                            Ok(StepDisposition::Continued(next)) => {
                                ex = next;
                            }
                            Err(e) => return PipelineOutcome::Failed(e),
                        }
                    }
                }
            }
        }
    }
    PipelineOutcome::Completed(ex)
}

/// Route channel with explicit security and circuit-breaker gates.
///
/// Gate order: Security → CB(before_call) → Pipeline → CB(after_result).
/// Errors from Security/CB gates go to `handler.handle_boundary`.
/// Errors from Pipeline go through the injected handler's retry/handle_step.
/// Pipeline Propagate returns Err — passed through to upstream.
#[derive(Clone)]
pub struct RouteChannelService {
    handler: Arc<dyn RouteErrorHandler>,
    security: Option<BoxProcessor>,
    cb_gate: Option<CircuitBreakerGate>,
    pipeline: BoxProcessor,
}

impl RouteChannelService {
    pub fn new(
        handler: Arc<dyn RouteErrorHandler>,
        security: Option<BoxProcessor>,
        cb_gate: Option<CircuitBreakerGate>,
        pipeline: BoxProcessor,
    ) -> Self {
        Self {
            handler,
            security,
            cb_gate,
            pipeline,
        }
    }
}

impl Service<Exchange> for RouteChannelService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), CamelError>> {
        // Swallow readiness errors from security gate — deferred to call()
        if let Some(ref mut sec) = self.security {
            match sec.clone().poll_ready(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(_)) | Poll::Ready(Ok(())) => {}
            }
        }
        // Pipeline readiness — swallow errors when handler present
        match self.pipeline.clone().poll_ready(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(_)) | Poll::Ready(Ok(())) => {}
        }
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let handler = self.handler.clone();
        let security = self.security.clone();
        let cb_gate = self.cb_gate.clone();
        let mut pipeline = self.pipeline.clone();

        Box::pin(async move {
            let mut ex = exchange;

            // Gate 1: Security
            if let Some(mut sec) = security {
                let original = ex.clone();
                match invoke_processor(&mut sec, ex).await {
                    Ok(next) => ex = next,
                    Err(err) => {
                        return handler
                            .handle_boundary(BoundaryKind::Security, original, err)
                            .await;
                    }
                }
            }

            // Gate 2: CircuitBreaker — before_call
            if let Some(ref cb) = cb_gate {
                match cb.before_call() {
                    CircuitBreakerDecision::Allow => { /* proceed to pipeline */ }
                    CircuitBreakerDecision::Fallback(mut fb) => {
                        // Circuit open with fallback — call fallback.
                        // Fallback errors go through handle_boundary, not raw to upstream.
                        let original = ex.clone();
                        match invoke_processor(&mut fb, ex).await {
                            Ok(result) => return Ok(result),
                            Err(err) => {
                                return handler
                                    .handle_boundary(BoundaryKind::CircuitBreaker, original, err)
                                    .await;
                            }
                        }
                    }
                    CircuitBreakerDecision::Reject(err) => {
                        let original = ex.clone();
                        return handler
                            .handle_boundary(BoundaryKind::CircuitBreaker, original, err)
                            .await;
                    }
                }
            }

            // Pipeline (handler already injected for step errors)
            let result = invoke_processor(&mut pipeline, ex).await;

            // Gate 2: CircuitBreaker — after_result
            if let Some(ref cb) = cb_gate {
                cb.after_result(&result);
            }

            // Propagate from inner handler — pass through to upstream
            result
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::error_handler::{BoundaryKind, PolicyId, RetryOutcome, StepDisposition};
    use camel_api::{Body, BoxProcessorExt, CircuitBreakerConfig, Message, Value};
    use camel_processor::RouteErrorHandler;
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
            _: &mut BoxProcessor,
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
            _: &mut BoxProcessor,
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
            steps: vec![CompiledStep::Process {
                processor: boxed,
                body_contract: None,
            }],
            handler: None,
            flatten_stop: true,
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
            steps: vec![],
            handler: None,
            flatten_stop: true,
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
        };

        let mut pipeline = SequentialPipeline {
            steps: vec![stop_step, after_step],
            handler: None,
            flatten_stop: true,
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
            },
        ];
        let ex = Exchange::new(camel_api::Message::new("payload"));
        let outcome = run_steps(steps, ex, None, false).await;
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
                _step: &mut camel_api::BoxProcessor,
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
            steps,
            ex,
            Some(Arc::new(RecordingHandler { counter })),
            false,
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
            }],
            "test-route",
            true,
            DetailLevel::Minimal,
            None,
            None,
        );
        let ex = Exchange::new(camel_api::Message::new("hello"));
        let result = tower::ServiceExt::oneshot(pipeline, ex).await;
        assert!(result.is_ok());
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
            }],
            None,
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
        };
        let step2 = CompiledStep::Process {
            processor: BoxProcessor::from_fn(|_ex| {
                Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
            }),
            body_contract: None,
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
        };

        let handler: Arc<dyn RouteErrorHandler> = Arc::new(ContinuedHandler);
        let outcome = run_steps(
            vec![step1, step2, step3],
            make_test_exchange(),
            Some(handler),
            false,
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
            }],
            Some(handler.clone()),
        );
        let channel = RouteChannelService::new(handler.clone(), None, None, pipeline);
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
        let pipeline = compose_pipeline_with_handler(vec![], Some(handler.clone()));
        let channel = RouteChannelService::new(handler.clone(), Some(deny_all), None, pipeline);
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
        let pipeline = compose_pipeline_with_handler(vec![], Some(handler.clone()));
        let channel = RouteChannelService::new(handler.clone(), None, Some(cb_gate), pipeline);
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
        let pipeline = compose_pipeline_with_handler(vec![], Some(handler.clone()));
        let channel = RouteChannelService::new(handler.clone(), None, Some(cb_gate), pipeline);
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

        let pipeline = compose_pipeline_with_handler(vec![], Some(handler.clone()));
        let channel = RouteChannelService::new(handler.clone(), None, Some(cb_gate), pipeline);

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

        // Pipeline emits Stop as the only step — top-level uses flatten_stop: true.
        let pipeline = compose_pipeline_with_handler(vec![CompiledStep::Stop], None);

        let channel = RouteChannelService::new(handler, None, Some(cb_gate), pipeline);

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
}
