use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::{Layer, Service, ServiceExt};

use camel_api::error_handler::{
    BoundaryKind, ExceptionDisposition, ExceptionPolicy, HEADER_REDELIVERED,
    HEADER_REDELIVERY_COUNTER, HEADER_REDELIVERY_MAX_COUNTER, PolicyId, RetryOutcome,
    StepDisposition,
};
use camel_api::{BoxProcessor, CamelError, Exchange, PipelineOutcome, SyncBoxProcessor, Value};

async fn execute_on_steps(
    original: Exchange,
    original_err: CamelError,
    on_steps: &SyncBoxProcessor,
    disposition: ExceptionDisposition,
    handler: Option<BoxProcessor>,
) -> Result<Exchange, CamelError> {
    let snapshot = original.clone();
    let mut ex = original;
    ex.set_error(original_err.clone());
    let mut pipeline = on_steps.clone_inner();
    let step_result = async {
        let svc = pipeline.ready().await?;
        svc.call(ex).await
    }
    .await;

    match step_result {
        Ok(mut ex) => {
            if disposition == ExceptionDisposition::Handled {
                ex.handle_error();
                Ok(ex)
            } else {
                // Propagate or Continued — steps execute for side-effects (e.g. logging) but
                // the modified exchange is discarded and the original error propagated.
                Err(original_err)
            }
        }
        Err(_) => {
            tracing::warn!(error = %original_err, "on_steps pipeline failed, falling back to handler/DLC");
            let mut ex = snapshot;
            ex.set_error(original_err);
            send_to_handler(ex, handler).await
        }
    }
}

/// Invoke a processor: readiness check + call, unified into a single Result.
///
/// Readiness errors and call errors are both returned as `Err(CamelError)`,
/// allowing the pipeline's recovery loop to handle them uniformly.
pub async fn invoke_processor(
    svc: &mut BoxProcessor,
    ex: Exchange,
) -> Result<Exchange, CamelError> {
    match svc.ready().await {
        Ok(ready) => ready.call(ex).await,
        Err(err) => Err(err),
    }
}

/// Route-level error handler owning ALL error handling logic.
///
/// Single owner of DLC, retry, onException policies. Called from
/// `RouteChannelService` (boundary errors) and `run_steps` (step errors).
#[async_trait::async_trait]
pub trait RouteErrorHandler: Send + Sync {
    /// Match a policy for the given error. Called once before retry.
    fn match_policy(&self, err: &CamelError) -> Option<PolicyId>;

    /// Phase 1: Retry the failed step.
    async fn retry_step(
        &self,
        policy: Option<PolicyId>,
        step: &mut dyn camel_api::error_handler::RetryableStep,
        original: Exchange,
        error: CamelError,
    ) -> RetryOutcome;

    /// Phase 2: Determine step disposition after retry exhaustion.
    async fn handle_step(
        &self,
        policy: Option<PolicyId>,
        exchange: Exchange,
        error: CamelError,
    ) -> Result<StepDisposition, CamelError>;

    /// Handle boundary (infrastructure) errors.
    async fn handle_boundary(
        &self,
        kind: BoundaryKind,
        exchange: Exchange,
        error: CamelError,
    ) -> Result<Exchange, CamelError>;
}

/// Default implementation of RouteErrorHandler.
/// Owns DLC producer exclusively. Encapsulates retry/onException/DLC logic.
///
/// Uses `SyncBoxProcessor` internally so the handler is `Send + Sync` as required
/// by the `RouteErrorHandler` trait.
pub struct DefaultRouteErrorHandler {
    pub(crate) dlc_producer: Option<SyncBoxProcessor>,
    pub(crate) policies: Vec<(ExceptionPolicy, Option<SyncBoxProcessor>)>,
}

impl DefaultRouteErrorHandler {
    pub fn new(
        dlc_producer: Option<BoxProcessor>,
        policies: Vec<(ExceptionPolicy, Option<BoxProcessor>)>,
    ) -> Self {
        Self {
            dlc_producer: dlc_producer.map(SyncBoxProcessor::new),
            policies: policies
                .into_iter()
                .map(|(p, prod)| (p, prod.map(SyncBoxProcessor::new)))
                .collect(),
        }
    }

    /// Resolve (disposition, producer) for a matched policy.
    /// Shared by handle_step and handle_boundary.
    fn resolve_producer(
        &self,
        policy: Option<PolicyId>,
    ) -> (ExceptionDisposition, Option<BoxProcessor>) {
        match policy {
            Some(PolicyId(idx)) => match self.policies.get(idx) {
                Some((p, prod)) => (
                    p.disposition,
                    prod.as_ref()
                        .map(|p| p.clone_inner())
                        .or_else(|| self.dlc_producer.as_ref().map(|p| p.clone_inner())),
                ),
                None => (
                    ExceptionDisposition::Propagate,
                    self.dlc_producer.as_ref().map(|p| p.clone_inner()),
                ),
            },
            None => (
                ExceptionDisposition::Propagate,
                self.dlc_producer.as_ref().map(|p| p.clone_inner()),
            ),
        }
    }
}

#[async_trait::async_trait]
impl RouteErrorHandler for DefaultRouteErrorHandler {
    fn match_policy(&self, err: &CamelError) -> Option<PolicyId> {
        self.policies
            .iter()
            .position(|(p, _)| (p.matches)(err))
            .map(PolicyId)
    }

    async fn retry_step(
        &self,
        policy: Option<PolicyId>,
        step: &mut dyn camel_api::error_handler::RetryableStep,
        original: Exchange,
        error: CamelError,
    ) -> RetryOutcome {
        let Some(PolicyId(idx)) = policy else {
            return RetryOutcome::Exhausted {
                exchange: original,
                error,
                policy: None,
            };
        };
        let Some((policy_def, _)) = self.policies.get(idx) else {
            return RetryOutcome::Exhausted {
                exchange: original,
                error,
                policy,
            };
        };
        let Some(ref backoff) = policy_def.retry else {
            return RetryOutcome::Exhausted {
                exchange: original,
                error,
                policy,
            };
        };

        for attempt in 0..backoff.max_attempts {
            let delay = backoff.delay_for(attempt);
            tokio::time::sleep(delay).await;

            let mut ex = original.clone();
            ex.input.set_header(HEADER_REDELIVERED, Value::Bool(true));
            ex.input.set_header(
                HEADER_REDELIVERY_COUNTER,
                Value::Number((attempt + 1).into()),
            );
            ex.input.set_header(
                HEADER_REDELIVERY_MAX_COUNTER,
                Value::Number(backoff.max_attempts.into()),
            );

            match step.invoke(ex).await {
                PipelineOutcome::Completed(exchange) => {
                    return RetryOutcome::Recovered(exchange);
                }
                PipelineOutcome::Stopped(stopped_ex) => {
                    return RetryOutcome::Stopped(stopped_ex);
                }
                PipelineOutcome::Failed(retry_err) => {
                    if attempt + 1 == backoff.max_attempts {
                        let mut final_ex = original;
                        final_ex
                            .input
                            .set_header(HEADER_REDELIVERED, Value::Bool(true));
                        final_ex.input.set_header(
                            HEADER_REDELIVERY_COUNTER,
                            Value::Number(backoff.max_attempts.into()),
                        );
                        final_ex.input.set_header(
                            HEADER_REDELIVERY_MAX_COUNTER,
                            Value::Number(backoff.max_attempts.into()),
                        );
                        return RetryOutcome::Exhausted {
                            exchange: final_ex,
                            error: retry_err,
                            policy,
                        };
                    }
                }
            }
        }

        RetryOutcome::Exhausted {
            exchange: original,
            error,
            policy,
        }
    }

    async fn handle_step(
        &self,
        policy: Option<PolicyId>,
        mut exchange: Exchange,
        error: CamelError,
    ) -> Result<StepDisposition, CamelError> {
        let (disposition, producer) = self.resolve_producer(policy);

        // Run on_steps if present (using the SAME policy identified by PolicyId)
        if let Some(PolicyId(idx)) = policy
            && let Some((p, _)) = self.policies.get(idx)
            && let Some(ref steps) = p.on_steps
        {
            let snapshot = exchange.clone();
            exchange.set_error(error.clone());
            let mut step_pipeline = steps.clone_inner();
            let step_result = async {
                let svc = step_pipeline.ready().await?;
                svc.call(exchange).await
            }
            .await;
            match step_result {
                Ok(mut ex) => match disposition {
                    ExceptionDisposition::Handled => {
                        ex.handle_error();
                        return Ok(StepDisposition::Handled(ex));
                    }
                    ExceptionDisposition::Continued => {
                        ex.clear_error();
                        return Ok(StepDisposition::Continued(ex));
                    }
                    ExceptionDisposition::Propagate => {
                        exchange = snapshot;
                    }
                },
                Err(_) => {
                    exchange = snapshot;
                }
            }
        }

        // No on_steps, on_steps failed, or Propagate — forward to DLC/handler.
        // BIND the returned exchange (must use handler output).
        exchange.set_error(error.clone());
        match send_to_handler(exchange, producer).await {
            Ok(handler_ex) => match disposition {
                ExceptionDisposition::Propagate => Ok(StepDisposition::Propagate(error)),
                ExceptionDisposition::Handled => {
                    let mut ex = handler_ex;
                    ex.clear_error();
                    Ok(StepDisposition::Handled(ex))
                }
                ExceptionDisposition::Continued => {
                    let mut ex = handler_ex;
                    ex.clear_error();
                    Ok(StepDisposition::Continued(ex))
                }
            },
            // Dead code by construction: send_to_handler always returns Ok.
            Err(_) => Ok(StepDisposition::Propagate(error)),
        }
    }

    async fn handle_boundary(
        &self,
        _kind: BoundaryKind,
        mut exchange: Exchange,
        error: CamelError,
    ) -> Result<Exchange, CamelError> {
        // Boundary errors: match policy, run on_steps, forward to DLC.
        // Disposition mapping:
        //   Handled → clear error, return Ok(exchange)
        //   Propagate | Continued → forward to DLC, return Ok(exchange_with_error)
        //   (Continued at boundary = Propagate — no next step to continue to)
        let policy = self.match_policy(&error);
        let (disposition, producer) = self.resolve_producer(policy);

        // Run on_steps if present (shared logic with handle_step)
        if let Some(PolicyId(idx)) = policy
            && let Some((p, _)) = self.policies.get(idx)
            && let Some(ref steps) = p.on_steps
        {
            let snapshot = exchange.clone();
            exchange.set_error(error.clone());
            let mut step_pipeline = steps.clone_inner();
            let step_result = async {
                let svc = step_pipeline.ready().await?;
                svc.call(exchange).await
            }
            .await;
            match step_result {
                Ok(mut ex) => match disposition {
                    ExceptionDisposition::Handled => {
                        ex.handle_error();
                        return Ok(ex);
                    }
                    ExceptionDisposition::Propagate | ExceptionDisposition::Continued => {
                        exchange = snapshot;
                    }
                },
                Err(_) => {
                    exchange = snapshot;
                }
            }
        }

        // Forward to DLC/handler — BIND returned exchange
        exchange.set_error(error.clone());
        match send_to_handler(exchange, producer).await {
            Ok(handler_ex) => match disposition {
                ExceptionDisposition::Handled => {
                    let mut ex = handler_ex;
                    ex.clear_error();
                    Ok(ex)
                }
                ExceptionDisposition::Propagate | ExceptionDisposition::Continued => {
                    let mut ex = handler_ex;
                    ex.set_error(error);
                    Ok(ex)
                }
            },
            // Dead code by construction: send_to_handler always returns Ok.
            Err(e) => Err(e),
        }
    }
}

/// Tower Layer that wraps a pipeline with error handling behaviour.
///
/// Constructed with already-resolved producers; URI resolution happens in `camel-core`.
#[deprecated(
    since = "0.16.0",
    note = "Use RouteChannelService + DefaultRouteErrorHandler instead"
)]
pub struct ErrorHandlerLayer {
    /// Resolved DLC producer (None = log only).
    dlc_producer: Option<BoxProcessor>,
    /// Policies with their resolved `handled_by` producers.
    policies: Vec<(ExceptionPolicy, Option<BoxProcessor>)>,
}

#[allow(deprecated)]
impl ErrorHandlerLayer {
    /// Create the layer with pre-resolved producers.
    pub fn new(
        dlc_producer: Option<BoxProcessor>,
        policies: Vec<(ExceptionPolicy, Option<BoxProcessor>)>,
    ) -> Self {
        Self {
            dlc_producer,
            policies,
        }
    }
}

#[allow(deprecated)]
impl<S> Layer<S> for ErrorHandlerLayer
where
    S: Service<Exchange, Response = Exchange, Error = CamelError> + Send + Clone + 'static,
    S::Future: Send + 'static,
{
    type Service = ErrorHandlerService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ErrorHandlerService {
            inner,
            dlc_producer: self.dlc_producer.clone(),
            policies: self
                .policies
                .iter()
                .map(|(p, prod)| (p.clone(), prod.clone()))
                .collect(),
        }
    }
}

/// Tower Service that absorbs pipeline errors by retrying and/or forwarding to a DLC.
///
/// `call` always returns `Ok` — errors are absorbed. The returned exchange will have
/// `has_error() == true` if the pipeline ultimately failed.
#[deprecated(
    since = "0.16.0",
    note = "Use RouteChannelService + DefaultRouteErrorHandler instead"
)]
pub struct ErrorHandlerService<S> {
    inner: S,
    dlc_producer: Option<BoxProcessor>,
    policies: Vec<(ExceptionPolicy, Option<BoxProcessor>)>,
}

#[allow(deprecated)]
impl<S: Clone> Clone for ErrorHandlerService<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            dlc_producer: self.dlc_producer.clone(),
            policies: self
                .policies
                .iter()
                .map(|(p, prod)| (p.clone(), prod.clone()))
                .collect(),
        }
    }
}

#[allow(deprecated)]
impl<S> ErrorHandlerService<S>
where
    S: Service<Exchange, Response = Exchange, Error = CamelError> + Send + Clone + 'static,
    S::Future: Send + 'static,
{
    /// Create the service directly (used in unit tests; in production use the Layer).
    pub fn new(
        inner: S,
        dlc_producer: Option<BoxProcessor>,
        policies: Vec<(ExceptionPolicy, Option<BoxProcessor>)>,
    ) -> Self {
        Self {
            inner,
            dlc_producer,
            policies,
        }
    }
}

#[allow(deprecated)]
impl<S> Service<Exchange> for ErrorHandlerService<S>
where
    S: Service<Exchange, Response = Exchange, Error = CamelError> + Send + Clone + 'static,
    S::Future: Send + 'static,
{
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Preserve backpressure (Pending) but never leak readiness errors upward.
        // Readiness errors are deferred to call(), where they go through the same
        // retry/onException/DLC path as call() errors. This is safe because call()
        // re-checks readiness on a fresh inner clone via inner.ready().await.
        match self.inner.poll_ready(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(_)) => Poll::Ready(Ok(())),
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
        }
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let mut inner = self.inner.clone();
        let dlc = self.dlc_producer.clone();
        let policies: Vec<(ExceptionPolicy, Option<BoxProcessor>)> = self
            .policies
            .iter()
            .map(|(p, prod)| (p.clone(), prod.clone()))
            .collect();

        Box::pin(async move {
            let original = exchange.clone();
            let result = match inner.ready().await {
                Ok(svc) => svc.call(exchange).await,
                Err(e) => Err(e), // readiness error — enters retry/DLC/onException path below
            };

            let err = match result {
                Ok(ex) => return Ok(ex),
                Err(e) => e,
            };

            // Find the first matching policy.
            let matched = policies.into_iter().find(|(p, _)| (p.matches)(&err));

            if let Some((policy, policy_producer)) = matched {
                // Retry if configured.
                if let Some(ref backoff) = policy.retry {
                    for attempt in 0..backoff.max_attempts {
                        let delay = backoff.delay_for(attempt);
                        tokio::time::sleep(delay).await;

                        // Set redelivery headers
                        let mut ex = original.clone();
                        ex.input.set_header(HEADER_REDELIVERED, Value::Bool(true));
                        ex.input.set_header(
                            HEADER_REDELIVERY_COUNTER,
                            Value::Number((attempt + 1).into()),
                        );
                        ex.input.set_header(
                            HEADER_REDELIVERY_MAX_COUNTER,
                            Value::Number(backoff.max_attempts.into()),
                        );

                        let result = match inner.ready().await {
                            Ok(svc) => svc.call(ex).await,
                            Err(e) => Err(e), // readiness error — enters retry exhaustion path
                        };
                        match result {
                            Ok(ex) => return Ok(ex),
                            Err(retry_err) => {
                                if attempt + 1 == backoff.max_attempts {
                                    // Retries exhausted — send to handler.
                                    let mut original = original.clone();
                                    original
                                        .input
                                        .set_header(HEADER_REDELIVERED, Value::Bool(true));
                                    original.input.set_header(
                                        HEADER_REDELIVERY_COUNTER,
                                        Value::Number(backoff.max_attempts.into()),
                                    );
                                    original.input.set_header(
                                        HEADER_REDELIVERY_MAX_COUNTER,
                                        Value::Number(backoff.max_attempts.into()),
                                    );
                                    if let Some(ref steps) = policy.on_steps {
                                        let handler = policy_producer.clone().or(dlc.clone());
                                        return execute_on_steps(
                                            original,
                                            retry_err,
                                            steps,
                                            policy.disposition,
                                            handler,
                                        )
                                        .await;
                                    }
                                    original.set_error(retry_err);
                                    let handler = policy_producer.or(dlc);
                                    return send_to_handler(original, handler).await;
                                }
                            }
                        }
                    }
                }
                // No retry configured (or 0 attempts) — send to policy handler or DLC.
                if let Some(ref steps) = policy.on_steps {
                    let handler = policy_producer.or(dlc);
                    return execute_on_steps(original, err, steps, policy.disposition, handler)
                        .await;
                }
                let mut ex = original.clone();
                ex.set_error(err);
                let handler = policy_producer.or(dlc);
                send_to_handler(ex, handler).await
            } else {
                // No matching policy — forward directly to DLC.
                let mut ex = original;
                ex.set_error(err);
                send_to_handler(ex, dlc).await
            }
        })
    }
}

async fn send_to_handler(
    exchange: Exchange,
    producer: Option<BoxProcessor>,
) -> Result<Exchange, CamelError> {
    match producer {
        None => {
            // log-policy: system-broken
            tracing::error!(
                error = ?exchange.error,
                "Exchange failed with no error handler configured"
            );
            Ok(exchange)
        }
        Some(mut prod) => match prod.ready().await {
            Err(e) => {
                // log-policy: system-broken
                tracing::error!("DLC/handler not ready: {e}");
                Ok(exchange)
            }
            Ok(svc) => match svc.call(exchange.clone()).await {
                Ok(ex) => Ok(ex),
                Err(e) => {
                    // log-policy: system-broken
                    tracing::error!("DLC/handler call failed: {e}");
                    // Return the original exchange with original error intact.
                    Ok(exchange)
                }
            },
        },
    }
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use camel_api::{
        BoxProcessor, BoxProcessorExt, CamelError, Exchange, Message, OutcomePipeline,
        OutcomeSegment, PipelineOutcome, RetryableStep, SyncBoxProcessor, Value,
        error_handler::RedeliveryPolicy,
    };
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    };
    use std::time::Duration;
    use tower::ServiceExt;

    fn make_exchange() -> Exchange {
        Exchange::new(Message::new("test"))
    }

    fn failing_processor() -> BoxProcessor {
        BoxProcessor::from_fn(|_ex| {
            Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
        })
    }

    fn ok_processor() -> BoxProcessor {
        BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }))
    }

    fn fail_n_times(n: u32) -> BoxProcessor {
        let count = Arc::new(AtomicU32::new(0));
        BoxProcessor::from_fn(move |ex| {
            let count = Arc::clone(&count);
            Box::pin(async move {
                let c = count.fetch_add(1, Ordering::SeqCst);
                if c < n {
                    Err(CamelError::ProcessorError(format!("attempt {c}")))
                } else {
                    Ok(ex)
                }
            })
        })
    }

    #[tokio::test]
    async fn test_ok_passthrough() {
        let svc = ErrorHandlerService::new(ok_processor(), None, vec![]);
        let result = svc.oneshot(make_exchange()).await;
        assert!(result.is_ok());
        assert!(!result.unwrap().has_error());
    }

    #[tokio::test]
    async fn test_error_goes_to_dlc() {
        let received = Arc::new(std::sync::Mutex::new(Vec::<Exchange>::new()));
        let received_clone = Arc::clone(&received);
        let dlc = BoxProcessor::from_fn(move |ex: Exchange| {
            let r = Arc::clone(&received_clone);
            Box::pin(async move {
                r.lock().unwrap().push(ex.clone());
                Ok(ex)
            })
        });

        let svc = ErrorHandlerService::new(failing_processor(), Some(dlc), vec![]);
        let result = svc.oneshot(make_exchange()).await;
        assert!(result.is_ok());
        let ex = result.unwrap();
        assert!(ex.has_error());
        assert_eq!(received.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_retry_recovers() {
        let inner = fail_n_times(2);
        let policy = ExceptionPolicy {
            matches: Arc::new(|_| true),
            retry: Some(RedeliveryPolicy {
                max_attempts: 3,
                initial_delay: Duration::from_millis(1),
                multiplier: 1.0,
                max_delay: Duration::from_millis(10),
                jitter_factor: 0.0,
            }),
            handled_by: None,
            on_steps: None,
            disposition: ExceptionDisposition::Propagate,
        };
        let svc = ErrorHandlerService::new(inner, None, vec![(policy, None)]);
        let result = svc.oneshot(make_exchange()).await;
        assert!(result.is_ok());
        assert!(!result.unwrap().has_error());
    }

    #[tokio::test]
    async fn test_retry_exhausted_goes_to_dlc() {
        let inner = fail_n_times(10);
        let received = Arc::new(std::sync::Mutex::new(0u32));
        let received_clone = Arc::clone(&received);
        let dlc = BoxProcessor::from_fn(move |ex: Exchange| {
            let r = Arc::clone(&received_clone);
            Box::pin(async move {
                *r.lock().unwrap() += 1;
                Ok(ex)
            })
        });
        let policy = ExceptionPolicy {
            matches: Arc::new(|_| true),
            retry: Some(RedeliveryPolicy {
                max_attempts: 2,
                initial_delay: Duration::from_millis(1),
                multiplier: 1.0,
                max_delay: Duration::from_millis(10),
                jitter_factor: 0.0,
            }),
            handled_by: None,
            on_steps: None,
            disposition: ExceptionDisposition::Propagate,
        };
        let svc = ErrorHandlerService::new(inner, Some(dlc), vec![(policy, None)]);
        let result = svc.oneshot(make_exchange()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().has_error());
        assert_eq!(*received.lock().unwrap(), 1);
    }

    #[test]
    fn test_poll_ready_delegates_to_inner() {
        use std::sync::atomic::AtomicBool;

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
                    // Already marked ready (second+ call) → Ready
                    Poll::Ready(Ok(()))
                } else {
                    // First call → Pending, schedule a wake
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            }

            fn call(&mut self, ex: Exchange) -> Self::Future {
                Box::pin(async move { Ok(ex) })
            }
        }

        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        let inner = DelayedReadyService {
            ready: Arc::new(AtomicBool::new(false)),
        };
        let mut svc = ErrorHandlerService::new(inner, None, vec![]);

        // First poll_ready: inner returns Pending, so ErrorHandlerService must too.
        let first = Pin::new(&mut svc).poll_ready(&mut cx);
        assert!(first.is_pending(), "expected Pending on first poll_ready");

        // Second poll_ready: inner returns Ready, so ErrorHandlerService must too.
        let second = Pin::new(&mut svc).poll_ready(&mut cx);
        assert!(second.is_ready(), "expected Ready on second poll_ready");
    }

    #[tokio::test]
    async fn test_no_matching_policy_uses_dlc() {
        let received = Arc::new(std::sync::Mutex::new(0u32));
        let received_clone = Arc::clone(&received);
        let dlc = BoxProcessor::from_fn(move |ex: Exchange| {
            let r = Arc::clone(&received_clone);
            Box::pin(async move {
                *r.lock().unwrap() += 1;
                Ok(ex)
            })
        });
        let policy = ExceptionPolicy::new(|e| matches!(e, CamelError::Io(_)));
        let svc = ErrorHandlerService::new(failing_processor(), Some(dlc), vec![(policy, None)]);
        let result = svc.oneshot(make_exchange()).await;
        assert!(result.is_ok());
        assert_eq!(*received.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn test_redelivery_headers_are_set() {
        use camel_api::error_handler::{
            HEADER_REDELIVERED, HEADER_REDELIVERY_COUNTER, HEADER_REDELIVERY_MAX_COUNTER,
            RedeliveryPolicy,
        };

        let inner = fail_n_times(10);
        let received = Arc::new(std::sync::Mutex::new(None));
        let received_clone = Arc::clone(&received);
        let dlc = BoxProcessor::from_fn(move |ex: Exchange| {
            let r = Arc::clone(&received_clone);
            Box::pin(async move {
                *r.lock().unwrap() = Some(ex.clone());
                Ok(ex)
            })
        });

        let policy = ExceptionPolicy {
            matches: Arc::new(|_| true),
            retry: Some(RedeliveryPolicy {
                max_attempts: 2,
                initial_delay: Duration::from_millis(1),
                multiplier: 1.0,
                max_delay: Duration::from_millis(10),
                jitter_factor: 0.0,
            }),
            handled_by: None,
            on_steps: None,
            disposition: ExceptionDisposition::Propagate,
        };

        let svc = ErrorHandlerService::new(inner, Some(dlc), vec![(policy, None)]);
        let _ = svc.oneshot(make_exchange()).await.unwrap();

        let ex = received.lock().unwrap().take().unwrap();
        assert_eq!(
            ex.input.header(HEADER_REDELIVERED),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            ex.input.header(HEADER_REDELIVERY_COUNTER),
            Some(&Value::Number(2.into()))
        );
        assert_eq!(
            ex.input.header(HEADER_REDELIVERY_MAX_COUNTER),
            Some(&Value::Number(2.into()))
        );
    }

    #[tokio::test]
    async fn test_jitter_produces_varying_delays_in_retry_flow() {
        use std::time::Instant;

        let inner = fail_n_times(10);
        let received = Arc::new(std::sync::Mutex::new(None));
        let received_clone = Arc::clone(&received);
        let dlc = BoxProcessor::from_fn(move |ex: Exchange| {
            let r = Arc::clone(&received_clone);
            Box::pin(async move {
                *r.lock().unwrap() = Some(ex.clone());
                Ok(ex)
            })
        });

        let policy = ExceptionPolicy {
            matches: Arc::new(|_| true),
            retry: Some(RedeliveryPolicy {
                max_attempts: 5,
                initial_delay: Duration::from_millis(20),
                multiplier: 1.0,
                max_delay: Duration::from_millis(100),
                jitter_factor: 0.5,
            }),
            handled_by: None,
            on_steps: None,
            disposition: ExceptionDisposition::Propagate,
        };

        let start = Instant::now();
        let svc = ErrorHandlerService::new(inner, Some(dlc), vec![(policy, None)]);
        let _ = svc.oneshot(make_exchange()).await.unwrap();
        let elapsed = start.elapsed();

        assert!(
            received.lock().unwrap().is_some(),
            "DLC should have received exchange"
        );

        assert!(
            elapsed >= Duration::from_millis(50),
            "5 retries with 20ms base delay should take at least 50ms (with jitter low bound)"
        );

        assert!(
            elapsed <= Duration::from_millis(500),
            "5 retries with 20ms base delay + 50% jitter should not exceed 500ms"
        );
    }

    #[tokio::test]
    async fn test_on_steps_handled_true_consumes_error() {
        use tower::ServiceExt;

        let steps_pipeline = BoxProcessor::new(tower::service_fn(|mut ex: Exchange| {
            ex.input.body = camel_api::Body::Bytes("handled".into());
            async move { Ok(ex) }
        }));
        let policy = ExceptionPolicy {
            matches: Arc::new(|_| true),
            retry: None,
            handled_by: None,
            on_steps: Some(SyncBoxProcessor::new(steps_pipeline)),
            disposition: ExceptionDisposition::Handled,
        };
        let inner = tower::service_fn(|_ex: Exchange| async {
            Err::<Exchange, CamelError>(CamelError::RouteError("fail".to_string()))
        });
        let mut svc = ErrorHandlerService::new(inner, None, vec![(policy, None)]);
        let ex = Exchange::default();
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert!(result.error.is_none(), "handled:true should clear error");
        assert!(matches!(result.input.body, camel_api::Body::Bytes(_)));
    }

    #[tokio::test]
    async fn test_on_steps_handled_false_propagates_error() {
        use tower::ServiceExt;

        let steps_pipeline = BoxProcessor::new(tower::service_fn(|mut ex: Exchange| {
            ex.input.body = camel_api::Body::Bytes("handled".into());
            async move { Ok(ex) }
        }));
        let policy = ExceptionPolicy {
            matches: Arc::new(|_| true),
            retry: None,
            handled_by: None,
            on_steps: Some(SyncBoxProcessor::new(steps_pipeline)),
            disposition: ExceptionDisposition::Propagate,
        };
        let inner = tower::service_fn(|_ex: Exchange| async {
            Err::<Exchange, CamelError>(CamelError::RouteError("fail".to_string()))
        });
        let mut svc = ErrorHandlerService::new(inner, None, vec![(policy, None)]);
        let ex = Exchange::default();
        let result = svc.ready().await.unwrap().call(ex).await;
        assert!(result.is_err(), "handled:false should propagate error");
    }

    // --- Readiness error capture tests ---
    //
    // ErrorHandlerService must capture readiness errors (poll_ready returning Err)
    // and route them through retry/onException/DLC instead of propagating raw.

    /// A service whose `poll_ready` always returns `Ready(Err(...))` but whose
    /// `call` returns `Ok`. This simulates a permanently-not-ready endpoint.
    #[derive(Clone)]
    struct ReadinessFailService {
        error: CamelError,
    }

    impl ReadinessFailService {
        fn new(error: CamelError) -> Self {
            Self { error }
        }
    }

    impl Service<Exchange> for ReadinessFailService {
        type Response = Exchange;
        type Error = CamelError;
        type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Err(self.error.clone()))
        }

        fn call(&mut self, ex: Exchange) -> Self::Future {
            // call() should never be reached if poll_ready returns Err,
            // but Tower's ready().await on a clone will re-encounter the readiness error.
            Box::pin(async move { Ok(ex) })
        }
    }

    #[tokio::test]
    async fn test_readiness_error_goes_to_dlc() {
        let readiness_err = CamelError::ProcessorError("readiness-fail".into());
        let inner = ReadinessFailService {
            error: readiness_err,
        };

        let received = Arc::new(std::sync::Mutex::new(Vec::<Exchange>::new()));
        let received_clone = Arc::clone(&received);
        let dlc = BoxProcessor::from_fn(move |ex: Exchange| {
            let r = Arc::clone(&received_clone);
            Box::pin(async move {
                r.lock().unwrap().push(ex.clone());
                Ok(ex)
            })
        });

        let svc = ErrorHandlerService::new(inner, Some(dlc), vec![]);
        let result = svc.oneshot(make_exchange()).await;

        // The error must be absorbed (Ok), not propagated raw (Err).
        assert!(
            result.is_ok(),
            "readiness error should be captured and sent to DLC, got: {:?}",
            result
        );
        let ex = result.unwrap();
        assert!(ex.has_error(), "exchange should carry the readiness error");
        assert_eq!(
            received.lock().unwrap().len(),
            1,
            "DLC should have received the exchange exactly once"
        );
    }

    #[tokio::test]
    async fn test_readiness_error_goes_to_matching_policy() {
        let readiness_err = CamelError::ProcessorError("readiness-fail".into());
        let inner = ReadinessFailService {
            error: readiness_err,
        };

        let steps_pipeline = BoxProcessor::new(tower::service_fn(|mut ex: Exchange| {
            ex.input.body = camel_api::Body::Bytes("handled-readiness".into());
            async move { Ok(ex) }
        }));
        let policy = ExceptionPolicy {
            matches: Arc::new(|_| true),
            retry: None,
            handled_by: None,
            on_steps: Some(SyncBoxProcessor::new(steps_pipeline)),
            disposition: ExceptionDisposition::Handled,
        };

        let svc = ErrorHandlerService::new(inner, None, vec![(policy, None)]);
        let result = svc.oneshot(make_exchange()).await;

        // The error must be absorbed and routed to the on_steps handler.
        assert!(
            result.is_ok(),
            "readiness error should be captured by policy, got: {:?}",
            result
        );
        let ex = result.unwrap();
        assert!(ex.error.is_none(), "handled:true should clear error");
        assert!(
            matches!(ex.input.body, camel_api::Body::Bytes(_)),
            "on_steps should have modified the body"
        );
    }

    #[test]
    fn test_poll_ready_converts_readiness_error_to_ok() {
        let readiness_err = CamelError::ProcessorError("readiness-fail".into());
        let inner = ReadinessFailService {
            error: readiness_err,
        };
        let mut svc = ErrorHandlerService::new(inner, None, vec![]);

        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        // poll_ready must NOT propagate the readiness error — convert to Ok.
        let poll = Pin::new(&mut svc).poll_ready(&mut cx);
        match poll {
            Poll::Ready(Ok(())) => { /* correct */ }
            Poll::Ready(Err(e)) => panic!("poll_ready leaked readiness error: {:?}", e),
            Poll::Pending => panic!("poll_ready should be Ready for readiness errors"),
        }
    }

    // --- invoke_processor tests ---

    #[tokio::test]
    async fn test_invoke_processor_returns_ok_on_success() {
        let mut svc = ok_processor();
        let ex = make_exchange();
        let result = invoke_processor(&mut svc, ex).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_invoke_processor_captures_readiness_error() {
        let mut failing_ready: BoxProcessor = BoxProcessor::new(ReadinessFailService::new(
            CamelError::ProcessorError("not ready".into()),
        ));
        let ex = make_exchange();
        let result = invoke_processor(&mut failing_ready, ex).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_on_steps_handled_true_clears_exception_properties() {
        use tower::ServiceExt;

        let steps_pipeline = BoxProcessor::new(tower::service_fn(|mut ex: Exchange| {
            ex.input.body = camel_api::Body::Bytes("handled".into());
            async move { Ok(ex) }
        }));
        let policy = ExceptionPolicy {
            matches: Arc::new(|_| true),
            retry: None,
            handled_by: None,
            on_steps: Some(SyncBoxProcessor::new(steps_pipeline)),
            disposition: ExceptionDisposition::Handled,
        };
        let inner = tower::service_fn(|_ex: Exchange| async {
            Err::<Exchange, CamelError>(CamelError::RouteError("fail".to_string()))
        });
        let mut svc = ErrorHandlerService::new(inner, None, vec![(policy, None)]);
        let ex = Exchange::default();
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert!(result.error.is_none(), "handled:true should clear error");
        assert!(
            result
                .properties
                .get(camel_api::exchange::PROPERTY_EXCEPTION_MESSAGE)
                .is_none(),
            "handled:true should clear exception properties"
        );
        assert!(
            result
                .properties
                .get(camel_api::exchange::PROPERTY_EXCEPTION_KIND)
                .is_none(),
            "handled:true should clear exception kind property"
        );
        assert!(
            result
                .properties
                .get(camel_api::exchange::PROPERTY_EXCEPTION_CAUGHT)
                .is_none(),
            "handled:true should clear exception caught property"
        );
    }

    // ── DefaultRouteErrorHandler tests ──

    #[test]
    fn test_match_policy_returns_id_for_matching_error() {
        let handler = DefaultRouteErrorHandler::new(
            None,
            vec![(
                ExceptionPolicy::new(|e| matches!(e, CamelError::ProcessorError(_))),
                None,
            )],
        );
        let id = handler.match_policy(&CamelError::ProcessorError("test".into()));
        assert_eq!(id, Some(PolicyId(0)));
    }

    #[test]
    fn test_match_policy_returns_none_for_unmatched() {
        let handler = DefaultRouteErrorHandler::new(None, vec![]);
        let id = handler.match_policy(&CamelError::ProcessorError("test".into()));
        assert_eq!(id, None);
    }

    // ── retry_step tests ──

    #[tokio::test]
    async fn test_retry_step_succeeds_on_second_attempt() {
        let mut policy = ExceptionPolicy::new(|_| true);
        policy.retry = Some(RedeliveryPolicy::new(3));
        let handler = DefaultRouteErrorHandler::new(None, vec![(policy, None)]);
        let mut step = fail_n_times(1); // fails once, then succeeds
        let ex = make_exchange();
        let outcome = handler
            .retry_step(
                Some(PolicyId(0)),
                &mut step,
                ex,
                CamelError::ProcessorError("attempt 0".into()),
            )
            .await;
        assert!(matches!(outcome, RetryOutcome::Recovered(_)));
    }

    #[tokio::test]
    async fn test_retry_step_exhausted_when_all_fail() {
        let mut policy = ExceptionPolicy::new(|_| true);
        policy.retry = Some(RedeliveryPolicy::new(3));
        let handler = DefaultRouteErrorHandler::new(None, vec![(policy, None)]);
        let mut step = failing_processor();
        let ex = make_exchange();
        let outcome = handler
            .retry_step(
                Some(PolicyId(0)),
                &mut step,
                ex,
                CamelError::ProcessorError("boom".into()),
            )
            .await;
        assert!(matches!(outcome, RetryOutcome::Exhausted { .. }));
    }

    #[tokio::test]
    async fn test_retry_step_no_policy_returns_exhausted_immediately() {
        let handler = DefaultRouteErrorHandler::new(None, vec![]);
        let mut step = ok_processor();
        let ex = make_exchange();
        let outcome = handler
            .retry_step(
                None,
                &mut step,
                ex,
                CamelError::ProcessorError("boom".into()),
            )
            .await;
        assert!(matches!(
            outcome,
            RetryOutcome::Exhausted { policy: None, .. }
        ));
    }

    // ── handle_step tests ──

    #[tokio::test]
    async fn test_handle_step_propagate_sends_to_dlc() {
        let dlc = BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }));
        let handler = DefaultRouteErrorHandler::new(Some(dlc), vec![]);
        let ex = make_exchange();
        let result = handler
            .handle_step(None, ex, CamelError::ProcessorError("boom".into()))
            .await;
        assert!(matches!(result, Ok(StepDisposition::Propagate(_))));
    }

    #[tokio::test]
    async fn test_handle_step_handled_uses_handler_output() {
        let handler_producer = BoxProcessor::from_fn(|mut ex| {
            Box::pin(async move {
                ex.input.set_header("processed_by", Value::Bool(true));
                Ok(ex)
            })
        });
        let policy = ExceptionPolicy {
            matches: std::sync::Arc::new(|_| true),
            retry: None,
            handled_by: None,
            on_steps: None,
            disposition: ExceptionDisposition::Handled,
        };
        let handler = DefaultRouteErrorHandler::new(None, vec![(policy, Some(handler_producer))]);
        let mut ex = make_exchange();
        ex.set_error(CamelError::ProcessorError("boom".into()));
        let result = handler
            .handle_step(
                Some(PolicyId(0)),
                ex,
                CamelError::ProcessorError("boom".into()),
            )
            .await;
        match result {
            Ok(StepDisposition::Handled(ex)) => {
                assert!(!ex.has_error(), "error should be cleared");
                assert_eq!(
                    ex.input.header("processed_by"),
                    Some(&Value::Bool(true)),
                    "should use handler's output exchange"
                );
            }
            other => panic!("expected Handled, got {:?}", other.is_ok()),
        }
    }

    #[tokio::test]
    async fn test_handle_step_continued_clears_error() {
        let policy = ExceptionPolicy {
            matches: std::sync::Arc::new(|_| true),
            retry: None,
            handled_by: None,
            on_steps: None,
            disposition: ExceptionDisposition::Continued,
        };
        let handler = DefaultRouteErrorHandler::new(None, vec![(policy, None)]);
        let mut ex = make_exchange();
        ex.set_error(CamelError::ProcessorError("boom".into()));
        let result = handler
            .handle_step(
                Some(PolicyId(0)),
                ex,
                CamelError::ProcessorError("boom".into()),
            )
            .await;
        match result {
            Ok(StepDisposition::Continued(ex)) => assert!(!ex.has_error()),
            other => panic!("expected Continued, got {:?}", other.is_ok()),
        }
    }

    #[tokio::test]
    async fn test_handle_step_with_on_steps_handled() {
        let steps_pipeline = BoxProcessor::new(tower::service_fn(|mut ex: Exchange| {
            ex.input.body = camel_api::Body::Bytes("on_steps_ran".into());
            async move { Ok(ex) }
        }));
        let policy = ExceptionPolicy {
            matches: std::sync::Arc::new(|_| true),
            retry: None,
            handled_by: None,
            on_steps: Some(SyncBoxProcessor::new(steps_pipeline)),
            disposition: ExceptionDisposition::Handled,
        };
        let handler = DefaultRouteErrorHandler::new(None, vec![(policy, None)]);
        let mut ex = make_exchange();
        ex.set_error(CamelError::ProcessorError("boom".into()));
        let result = handler
            .handle_step(
                Some(PolicyId(0)),
                ex,
                CamelError::ProcessorError("boom".into()),
            )
            .await;
        match result {
            Ok(StepDisposition::Handled(ex)) => {
                assert!(!ex.has_error(), "error should be cleared");
                assert!(
                    matches!(ex.input.body, camel_api::Body::Bytes(_)),
                    "on_steps should have modified the body"
                );
            }
            other => panic!("expected Handled, got: {}", other.is_ok()),
        }
    }

    #[tokio::test]
    async fn test_handle_step_with_on_steps_propagate_falls_through() {
        let steps_pipeline = BoxProcessor::new(tower::service_fn(|mut ex: Exchange| {
            ex.input.body = camel_api::Body::Bytes("on_steps_ran".into());
            async move { Ok(ex) }
        }));
        let dlc_called = Arc::new(AtomicU32::new(0));
        let dlc_called_clone = dlc_called.clone();
        let dlc = BoxProcessor::from_fn(move |ex: Exchange| {
            let c = dlc_called_clone.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(ex)
            })
        });
        let policy = ExceptionPolicy {
            matches: std::sync::Arc::new(|_| true),
            retry: None,
            handled_by: None,
            on_steps: Some(SyncBoxProcessor::new(steps_pipeline)),
            disposition: ExceptionDisposition::Propagate,
        };
        let handler = DefaultRouteErrorHandler::new(Some(dlc), vec![(policy, None)]);
        let mut ex = make_exchange();
        ex.set_error(CamelError::ProcessorError("boom".into()));
        let result = handler
            .handle_step(
                Some(PolicyId(0)),
                ex,
                CamelError::ProcessorError("boom".into()),
            )
            .await;
        assert!(
            matches!(result, Ok(StepDisposition::Propagate(_))),
            "Propagate disposition should return Propagate"
        );
        assert_eq!(
            dlc_called.load(Ordering::SeqCst),
            1,
            "DLC should be called when on_steps disposition is Propagate"
        );
    }

    #[tokio::test]
    async fn test_handle_step_dlc_failure_propagates() {
        let failing_dlc = BoxProcessor::from_fn(|_| {
            Box::pin(async { Err(CamelError::ProcessorError("dlc broken".into())) })
        });
        let handler = DefaultRouteErrorHandler::new(Some(failing_dlc), vec![]);
        let ex = make_exchange();
        let result = handler
            .handle_step(None, ex, CamelError::ProcessorError("original".into()))
            .await;
        assert!(
            matches!(result, Ok(StepDisposition::Propagate(_))),
            "DLC failure should still return Propagate with original error"
        );
    }

    // ── handle_boundary tests ──

    #[tokio::test]
    async fn test_handle_boundary_security_error_goes_to_dlc() {
        let dlc_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let count_clone = dlc_count.clone();
        let dlc = BoxProcessor::from_fn(move |ex| {
            let c = count_clone.clone();
            Box::pin(async move {
                c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(ex)
            })
        });
        let handler = DefaultRouteErrorHandler::new(Some(dlc), vec![]);
        let ex = make_exchange();
        let result = handler
            .handle_boundary(
                BoundaryKind::Security,
                ex,
                CamelError::Unauthorized("denied".into()),
            )
            .await;
        assert!(result.is_ok());
        assert_eq!(dlc_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_handle_boundary_handled_clears_error() {
        let policy = ExceptionPolicy {
            matches: std::sync::Arc::new(|_| true),
            retry: None,
            handled_by: None,
            on_steps: None,
            disposition: ExceptionDisposition::Handled,
        };
        let handler = DefaultRouteErrorHandler::new(None, vec![(policy, None)]);
        let ex = make_exchange();
        let result = handler
            .handle_boundary(
                BoundaryKind::Security,
                ex,
                CamelError::Unauthorized("denied".into()),
            )
            .await;
        assert!(result.is_ok());
        assert!(
            !result.unwrap().has_error(),
            "Handled disposition should clear error"
        );
    }

    #[tokio::test]
    async fn test_handle_boundary_propagate_preserves_error() {
        let policy = ExceptionPolicy {
            matches: std::sync::Arc::new(|_| true),
            retry: None,
            handled_by: None,
            on_steps: None,
            disposition: ExceptionDisposition::Propagate,
        };
        let handler = DefaultRouteErrorHandler::new(None, vec![(policy, None)]);
        let ex = make_exchange();
        let result = handler
            .handle_boundary(
                BoundaryKind::CircuitBreaker,
                ex,
                CamelError::CircuitOpen("open".into()),
            )
            .await;
        assert!(result.is_ok(), "boundary errors always return Ok");
        assert!(
            result.unwrap().has_error(),
            "Propagate disposition should preserve error"
        );
    }

    #[tokio::test]
    async fn test_handle_boundary_continued_preserves_error_like_propagate() {
        let dlc_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let count_clone = dlc_count.clone();
        let dlc = BoxProcessor::from_fn(move |ex| {
            let c = count_clone.clone();
            Box::pin(async move {
                c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(ex)
            })
        });
        let policy = ExceptionPolicy {
            matches: std::sync::Arc::new(|_| true),
            retry: None,
            handled_by: None,
            on_steps: None,
            disposition: ExceptionDisposition::Continued,
        };
        let handler = DefaultRouteErrorHandler::new(Some(dlc), vec![(policy, None)]);
        let ex = make_exchange();
        let result = handler
            .handle_boundary(
                BoundaryKind::Security,
                ex,
                CamelError::Unauthorized("denied".into()),
            )
            .await;
        assert!(result.is_ok(), "boundary errors always return Ok");
        assert!(
            result.unwrap().has_error(),
            "Continued at boundary should preserve error"
        );
        assert_eq!(
            dlc_count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "DLC should be called"
        );
    }

    #[tokio::test]
    async fn test_handle_boundary_with_on_steps_handled() {
        let steps_pipeline = BoxProcessor::new(tower::service_fn(|mut ex: Exchange| {
            ex.input.body = camel_api::Body::Bytes("on_steps_ran".into());
            async move { Ok(ex) }
        }));
        let policy = ExceptionPolicy {
            matches: std::sync::Arc::new(|_| true),
            retry: None,
            handled_by: None,
            on_steps: Some(SyncBoxProcessor::new(steps_pipeline)),
            disposition: ExceptionDisposition::Handled,
        };
        let handler = DefaultRouteErrorHandler::new(None, vec![(policy, None)]);
        let ex = make_exchange();
        let result = handler
            .handle_boundary(
                BoundaryKind::Security,
                ex,
                CamelError::Unauthorized("denied".into()),
            )
            .await;
        assert!(result.is_ok(), "boundary errors always return Ok");
        let ex = result.unwrap();
        assert!(!ex.has_error(), "Handled disposition should clear error");
        assert!(
            matches!(ex.input.body, camel_api::Body::Bytes(_)),
            "on_steps should have modified the body"
        );
    }

    #[tokio::test]
    async fn retry_step_segment_stop_maps_to_retry_outcome_stopped() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        #[derive(Clone)]
        struct StoppingSegment {
            n: Arc<AtomicUsize>,
        }
        impl OutcomePipeline for StoppingSegment {
            fn clone_box(&self) -> Box<dyn OutcomePipeline> {
                Box::new(self.clone())
            }
            fn run<'a>(
                &'a mut self,
                ex: Exchange,
            ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
                let n = self.n.clone();
                Box::pin(async move {
                    n.fetch_add(1, Ordering::SeqCst);
                    PipelineOutcome::Stopped(ex)
                })
            }
        }

        let call_count = Arc::new(AtomicUsize::new(0));
        let seg = OutcomeSegment::new(Box::new(StoppingSegment {
            n: call_count.clone(),
        }));
        let mut retryable: Box<dyn RetryableStep> = Box::new(seg);

        let mut policy = ExceptionPolicy::new(|_e: &CamelError| true);
        policy.retry = Some(RedeliveryPolicy::new(3));
        let handler = DefaultRouteErrorHandler::new(None, vec![(policy, None)]);

        let original = Exchange::new(Message::new("retry-me"));
        let err = CamelError::ProcessorError("trigger retry".into());
        let outcome = handler
            .retry_step(Some(PolicyId(0)), retryable.as_mut(), original, err)
            .await;

        assert!(
            matches!(outcome, RetryOutcome::Stopped(_)),
            "Segment Stop must map to RetryOutcome::Stopped, got {:?}",
            outcome
        );
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "Stop must short-circuit retry — only 1 invoke expected, got {}",
            call_count.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn retry_step_new_signature_works_with_dlc_producer() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        #[derive(Clone)]
        struct CountingProducer {
            count: Arc<AtomicUsize>,
            succeed_on: usize,
        }
        impl tower::Service<Exchange> for CountingProducer {
            type Response = Exchange;
            type Error = CamelError;
            type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;
            fn poll_ready(
                &mut self,
                _cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Result<(), Self::Error>> {
                std::task::Poll::Ready(Ok(()))
            }
            fn call(&mut self, ex: Exchange) -> Self::Future {
                let n = self.count.fetch_add(1, Ordering::SeqCst);
                let succeed_on = self.succeed_on;
                Box::pin(async move {
                    if n >= succeed_on {
                        Ok(ex)
                    } else {
                        Err(CamelError::ProcessorError("retry".into()))
                    }
                })
            }
        }

        let count = Arc::new(AtomicUsize::new(0));
        let producer = CountingProducer {
            count: count.clone(),
            succeed_on: 2,
        };
        let sync_bp = SyncBoxProcessor::new(BoxProcessor::new(producer));
        let bp1 = sync_bp.clone_inner();
        let bp2 = sync_bp.clone_inner();
        let mut retryable1: Box<dyn RetryableStep> = Box::new(bp1);
        let mut retryable2: Box<dyn RetryableStep> = Box::new(bp2);

        let ex = Exchange::new(Message::new("dlc"));
        let outcome1 = retryable1.invoke(ex.clone()).await;
        let outcome2 = retryable2.invoke(ex).await;
        assert!(matches!(outcome1, PipelineOutcome::Failed(_)));
        assert!(matches!(outcome2, PipelineOutcome::Failed(_)));
        assert_eq!(
            count.load(Ordering::SeqCst),
            2,
            "DLC producer must be invoked exactly twice through SyncBoxProcessor"
        );
        drop(retryable1);
        drop(retryable2);
        drop(sync_bp);
    }
}
