// adapters/route_compiler.rs
// Pipeline compilation functions: compose BuilderSteps into a Tower BoxProcessor.
// Tower types live here as this is the adapter layer responsible for
// translating declarative route definitions into executable pipelines.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::BodyType;
use camel_api::metrics::MetricsCollector;
use camel_api::{BoxProcessor, CamelError, Exchange, IdentityProcessor};

use camel_api::error_handler::{BoundaryKind, RetryOutcome, StepDisposition};
use camel_processor::{
    CircuitBreakerDecision, CircuitBreakerGate, RouteErrorHandler, invoke_processor,
};
use tracing::Instrument;

use crate::lifecycle::adapters::body_coercing::wrap_if_needed;
use crate::shared::observability::adapters::TracingProcessor;
use crate::shared::observability::domain::DetailLevel;

/// Compose a list of BoxProcessors into a single pipeline that runs them sequentially.
pub fn compose_pipeline(processors: Vec<BoxProcessor>) -> BoxProcessor {
    compose_pipeline_with_handler(processors, None)
}

/// Compose a list of BoxProcessors with an optional route error handler.
///
/// When a handler is present, step readiness errors are swallowed (poll_ready
/// returns Ready) and the handler's retry/recovery logic is invoked on step
/// failures. Otherwise, step readiness errors propagate immediately.
pub fn compose_pipeline_with_handler(
    processors: Vec<BoxProcessor>,
    handler: Option<Arc<dyn RouteErrorHandler>>,
) -> BoxProcessor {
    if processors.is_empty() {
        return BoxProcessor::new(IdentityProcessor);
    }
    BoxProcessor::new(SequentialPipeline {
        steps: processors,
        handler,
    })
}

/// Compose a list of BoxProcessors into a traced pipeline.
///
/// Each processor is wrapped with TracingProcessor to emit spans for observability.
/// When tracing is disabled, falls back to plain compose_pipeline with zero overhead.
pub fn compose_traced_pipeline(
    processors: Vec<BoxProcessor>,
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

    let wrapped: Vec<BoxProcessor> = processors
        .into_iter()
        .enumerate()
        .map(|(idx, processor)| {
            BoxProcessor::new(TracingProcessor::new(
                processor,
                route_id.to_string(),
                idx,
                detail_level.clone(),
                metrics.clone(),
            ))
        })
        .collect();

    BoxProcessor::new(TracedPipeline {
        steps: wrapped,
        handler,
    })
}

/// Compose a list of `(BoxProcessor, Option<BodyType>)` pairs into a single pipeline.
///
/// Each processor is optionally wrapped with [`BodyCoercingProcessor`] based on its
/// contract. Processors with `None` contract are passed through with zero overhead.
///
/// This is the internal variant used by `route_controller` for top-level route steps.
/// Sub-pipelines (filter, choice, split branches) always use [`compose_pipeline`] because
/// their steps are `.process()` closures or EIP processors — never raw component producers.
pub fn compose_pipeline_with_contracts(
    processors: Vec<(BoxProcessor, Option<BodyType>)>,
    handler: Option<Arc<dyn RouteErrorHandler>>,
) -> BoxProcessor {
    let wrapped: Vec<BoxProcessor> = processors
        .into_iter()
        .map(|(p, c)| wrap_if_needed(p, c))
        .collect();
    compose_pipeline_with_handler(wrapped, handler)
}

/// Compose a list of `(BoxProcessor, Option<BodyType>)` pairs into a traced pipeline.
///
/// Applies body coercion contracts first, then wraps with `TracingProcessor`.
/// When tracing is disabled, falls back to [`compose_pipeline_with_contracts`].
pub(crate) fn compose_traced_pipeline_with_contracts(
    processors: Vec<(BoxProcessor, Option<BodyType>)>,
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

    let wrapped: Vec<BoxProcessor> = processors
        .into_iter()
        .enumerate()
        .map(|(idx, (p, c))| {
            let coerced = wrap_if_needed(p, c);
            BoxProcessor::new(TracingProcessor::new(
                coerced,
                route_id.to_string(),
                idx,
                detail_level.clone(),
                metrics.clone(),
            ))
        })
        .collect();

    BoxProcessor::new(TracedPipeline {
        steps: wrapped,
        handler,
    })
}

/// A service that executes a sequence of BoxProcessors in order.
#[derive(Clone)]
struct SequentialPipeline {
    steps: Vec<BoxProcessor>,
    handler: Option<Arc<dyn RouteErrorHandler>>,
}

impl Service<Exchange> for SequentialPipeline {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.steps.first() {
            Some(first) => {
                let mut first = first.clone();
                match first.poll_ready(cx) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(Err(_)) if self.handler.is_some() => Poll::Ready(Ok(())),
                    Poll::Ready(other) => Poll::Ready(other),
                }
            }
            None => Poll::Ready(Ok(())),
        }
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let steps = self.steps.clone();
        let handler = self.handler.clone();
        Box::pin(async move { run_steps(steps, exchange, handler, false).await })
    }
}

/// A traced service pipeline for wrapped processors.
#[derive(Clone)]
struct TracedPipeline {
    steps: Vec<BoxProcessor>,
    handler: Option<Arc<dyn RouteErrorHandler>>,
}

impl Service<Exchange> for TracedPipeline {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.steps.first() {
            Some(first) => {
                let mut first = first.clone();
                match first.poll_ready(cx) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(Err(_)) if self.handler.is_some() => Poll::Ready(Ok(())),
                    Poll::Ready(other) => Poll::Ready(other),
                }
            }
            None => Poll::Ready(Ok(())),
        }
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let steps = self.steps.clone();
        let handler = self.handler.clone();
        Box::pin(async move { run_steps(steps, exchange, handler, true).await })
    }
}

/// Run a sequence of steps with optional error recovery.
///
/// Each step is executed via `invoke_processor`. On failure:
/// 1. `CamelError::Stopped` is propagated immediately (no handler involved).
/// 2. If a handler is present, `match_policy` selects a retry policy.
/// 3. `retry_step` attempts recovery; if exhausted, `handle_step` determines
///    the disposition:
///    - `Propagate` — return the error
///    - `Handled` — return the exchange early (success)
///    - `Continued` — clear the error and continue to the next step
/// 4. If no handler is present, the error is propagated directly.
pub async fn run_steps(
    steps: Vec<BoxProcessor>,
    exchange: Exchange,
    handler: Option<Arc<dyn RouteErrorHandler>>,
    trace: bool,
) -> Result<Exchange, CamelError> {
    let mut ex = exchange;
    for (i, mut step) in steps.into_iter().enumerate() {
        let original = ex.clone();
        let invoke_future = invoke_processor(&mut step, ex);
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
                if matches!(err, CamelError::Stopped) {
                    return Err(err);
                }

                let Some(handler) = handler.as_ref() else {
                    return Err(err);
                };

                let policy = handler.match_policy(&err);
                match handler.retry_step(policy, &mut step, original, err).await {
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
                                .await?
                        } else {
                            handle_future.await?
                        };
                        match disposition {
                            StepDisposition::Propagate(e) => return Err(e),
                            StepDisposition::Handled(done) => return Ok(done),
                            StepDisposition::Continued(next) => {
                                ex = next;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(ex)
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
            steps: vec![boxed],
            handler: None,
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
        };
        let result = pipeline.poll_ready(&mut cx);
        assert!(result.is_ready(), "expected Ready for empty pipeline");
    }

    #[tokio::test]
    async fn test_pipeline_stops_gracefully_on_stopped_error() {
        let after_called = Arc::new(AtomicBool::new(false));
        let after_called_clone = after_called.clone();

        let stop_step = BoxProcessor::from_fn(|_ex| Box::pin(async { Err(CamelError::Stopped) }));
        let after_step = BoxProcessor::from_fn(move |ex| {
            after_called_clone.store(true, Ordering::SeqCst);
            Box::pin(async move { Ok(ex) })
        });

        let mut pipeline = SequentialPipeline {
            steps: vec![stop_step, after_step],
            handler: None,
        };

        let ex = Exchange::new(camel_api::Message::new("hello"));
        let result = pipeline.call(ex).await;

        assert!(
            matches!(result, Err(CamelError::Stopped)),
            "expected Err(Stopped), got: {:?}",
            result
        );
        assert!(
            !after_called.load(Ordering::SeqCst),
            "step after stop should not be called"
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
            vec![step],
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

        let pipeline = compose_pipeline_with_contracts(vec![(inner, Some(BodyType::Text))], None);

        let mut ex = Exchange::new(Message::default());
        ex.input.body = Body::Json(json!("hello"));

        let result = tower::ServiceExt::oneshot(pipeline, ex).await;
        assert!(result.is_ok());

        let observed = seen_body.lock().expect("lock seen body").clone();
        assert_eq!(observed, Some(Body::Text("hello".to_string())));
    }

    #[tokio::test]
    async fn test_run_steps_continued_skips_failed_step() {
        let step1 = BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }));
        let step2 = BoxProcessor::from_fn(|_ex| {
            Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
        });
        let step3_hit = Arc::new(AtomicBool::new(false));
        let hit = step3_hit.clone();
        let step3 = BoxProcessor::from_fn(move |ex| {
            let hit = hit.clone();
            Box::pin(async move {
                hit.store(true, Ordering::SeqCst);
                Ok(ex)
            })
        });

        let handler: Arc<dyn RouteErrorHandler> = Arc::new(ContinuedHandler);
        let result = run_steps(
            vec![step1, step2, step3],
            make_test_exchange(),
            Some(handler),
            false,
        )
        .await;
        assert!(result.is_ok());
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
        let pipeline = compose_pipeline_with_handler(vec![failing_step], Some(handler.clone()));
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
}
