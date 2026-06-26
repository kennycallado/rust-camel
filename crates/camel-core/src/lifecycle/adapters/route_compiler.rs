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

// Re-export outcome composition types so existing step_compiler import paths
// (`route_compiler::BoxProcessorSegment`, etc.) continue to work.
pub(crate) use super::outcome_composition::{
    BodyCoercingSegment, BoxProcessorSegment, StopSegment, compose_outcome_segment,
};

/// Compose a list of CompiledSteps into a sub-pipeline (EIP internal).
///
/// Uses `into_tower_result()` so `PipelineOutcome::Stopped` maps to `Ok(ex)`.
/// Use [`compose_pipeline_with_handler`] for the top-level consumer-facing pipeline.
pub fn compose_pipeline(processors: Vec<CompiledStep>) -> BoxProcessor {
    if processors.is_empty() {
        return BoxProcessor::new(IdentityProcessor);
    }
    BoxProcessor::new(SequentialPipeline {
        steps: processors,
        handler: None,
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
    })
}

/// Compose a list of CompiledSteps into a traced pipeline with Stop→Ok translation.
///
/// Each processor is wrapped with TracingProcessor to emit spans for observability.
/// When tracing is disabled, falls back to [`compose_pipeline_with_handler`] with zero overhead.
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
            let (p, c, lc) = match step {
                CompiledStep::Process {
                    processor,
                    body_contract,
                    lifecycle,
                } => (processor, body_contract, lifecycle),
                CompiledStep::Stop => return CompiledStep::Stop,
                CompiledStep::Segment { .. } => return step,
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
                lifecycle: lc,
            }
        })
        .collect();

    BoxProcessor::new(TracedPipeline {
        steps: wrapped,
        handler,
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
                lifecycle,
            } => {
                let coerced = wrap_if_needed(processor, body_contract);
                CompiledStep::Process {
                    processor: coerced,
                    body_contract: None,
                    lifecycle,
                }
            }
            CompiledStep::Stop => CompiledStep::Stop,
            CompiledStep::Segment { .. } => step,
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
                lifecycle,
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
                    lifecycle,
                }
            }
            CompiledStep::Stop => CompiledStep::Stop,
            CompiledStep::Segment { .. } => step,
        })
        .collect();

    BoxProcessor::new(TracedPipeline {
        steps: wrapped,
        handler,
    })
}

/// A service that executes a sequence of CompiledSteps in order.
///
/// Uses `into_tower_result()` so `PipelineOutcome::Stopped(ex)` maps to
/// `Ok(ex)` — the Bug B fix that makes Stop indistinguishable from Completed
/// at the consumer boundary.
#[derive(Clone)]
struct SequentialPipeline {
    steps: Vec<CompiledStep>,
    handler: Option<Arc<dyn RouteErrorHandler>>,
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
            Some(CompiledStep::Segment { .. }) => Poll::Ready(Ok(())),
            None => Poll::Ready(Ok(())),
        }
    }

    // ADR-0024 reply-channel adapter: PipelineOutcome → Result<Exchange, CamelError>.
    // Completed(ex) and Stopped(ex) both map to Ok(ex); Failed(err) maps to Err.
    // Downstream consumers (RouteChannelService, ExchangeUoWLayer, HTTP/Kafka reply
    // finalisers) see Result<Exchange, CamelError> and treat Stop as success.
    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let steps = self.steps.clone();
        let handler = self.handler.clone();
        Box::pin(async move {
            let outcome = run_steps(steps, exchange, handler, false).await;
            outcome.into_tower_result()
        })
    }
}

/// A traced service pipeline for wrapped CompiledSteps.
#[derive(Clone)]
struct TracedPipeline {
    steps: Vec<CompiledStep>,
    handler: Option<Arc<dyn RouteErrorHandler>>,
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
            Some(CompiledStep::Segment { .. }) => Poll::Ready(Ok(())),
            None => Poll::Ready(Ok(())),
        }
    }

    // ADR-0024 reply-channel adapter (same as SequentialPipeline::call):
    // Completed(ex) and Stopped(ex) both map to Ok(ex). Bug B fix.
    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let steps = self.steps.clone();
        let handler = self.handler.clone();
        Box::pin(async move {
            let outcome = run_steps(steps, exchange, handler, true).await;
            outcome.into_tower_result()
        })
    }
}

/// Run a sequence of CompiledSteps with optional error recovery.
///
/// Each step is unified under `Box<dyn RetryableStep>` — both Process and
/// Segment variants are treated uniformly. On failure:
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
    use camel_api::error_handler::RetryableStep;
    let mut ex = exchange;
    for (i, step) in steps.into_iter().enumerate() {
        let (mut retryable, _body_contract): (Box<dyn RetryableStep>, _) = match step {
            CompiledStep::Stop => return PipelineOutcome::Stopped(ex),
            CompiledStep::Process {
                processor,
                body_contract,
                ..
            } => {
                let boxed: Box<dyn RetryableStep> = Box::new(processor);
                (boxed, body_contract)
            }
            CompiledStep::Segment {
                segment,
                body_contract,
                ..
            } => {
                let boxed: Box<dyn RetryableStep> = Box::new(segment);
                (boxed, body_contract)
            }
        };

        let original = ex.clone();
        let outcome = if trace {
            invoke_with_span(&mut retryable, ex, i).await
        } else {
            retryable.invoke(ex).await
        };

        match outcome {
            PipelineOutcome::Completed(next) => {
                ex = next;
            }
            PipelineOutcome::Stopped(stopped_ex) => {
                return PipelineOutcome::Stopped(stopped_ex);
            }
            PipelineOutcome::Failed(err) => {
                let Some(handler) = handler.as_ref() else {
                    return PipelineOutcome::Failed(err);
                };
                let policy = handler.match_policy(&err);
                match handler
                    .retry_step(policy, retryable.as_mut(), original, err)
                    .await
                {
                    RetryOutcome::Recovered(exchange) => {
                        ex = exchange;
                    }
                    RetryOutcome::Stopped(stopped_ex) => {
                        return PipelineOutcome::Stopped(stopped_ex);
                    }
                    RetryOutcome::Exhausted {
                        exchange,
                        error,
                        policy,
                    } => {
                        let disposition = if trace {
                            handler
                                .handle_step(policy, exchange, error)
                                .instrument(tracing::debug_span!("error_handler", step_index = i))
                                .await
                        } else {
                            handler.handle_step(policy, exchange, error).await
                        };
                        match disposition {
                            Ok(StepDisposition::Propagate(e)) => {
                                return PipelineOutcome::Failed(e);
                            }
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

async fn invoke_with_span(
    retryable: &mut Box<dyn camel_api::error_handler::RetryableStep>,
    exchange: Exchange,
    idx: usize,
) -> PipelineOutcome {
    retryable
        .invoke(exchange)
        .instrument(tracing::debug_span!("pipeline_step", index = idx))
        .await
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
#[path = "route_compiler_tests.rs"]
mod tests;
