// adapters/route_compiler.rs
// Pipeline compilation functions: compose BuilderSteps into a Tower BoxProcessor.
// Tower types live here as this is the adapter layer responsible for
// translating declarative route definitions into executable pipelines.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio_util::sync::CancellationToken;
use tower::Service;

use camel_api::metrics::MetricsCollector;
use camel_api::{
    BoxProcessor, CamelError, Exchange, IdentityProcessor, Message, NoOpMetrics,
    ORIGINAL_MESSAGE_EXTENSION, PipelineOutcome,
};

use camel_api::error_handler::{BoundaryKind, RetryOutcome, StepDisposition};
use camel_processor::{
    CircuitBreakerDecision, CircuitBreakerGate, RouteErrorHandler, invoke_processor,
};
use opentelemetry::trace::{SpanKind, Status, TraceContextExt, Tracer};
use opentelemetry::{Context as OtelContext, InstrumentationScope, KeyValue, global};
use tracing::Instrument;

use crate::lifecycle::adapters::body_coercing::wrap_if_needed;
use crate::lifecycle::adapters::step_compilers::CompiledStep;
use crate::shared::observability::adapters::TracingProcessor;
use crate::shared::observability::adapters::tracer::{
    SpanEndGuard, capped_correlation_id, record_exception, step_id_for, step_span_attributes,
};
use crate::shared::observability::domain::DetailLevel;

// Re-export outcome composition types so existing step_compiler import paths
// (`route_compiler::BoxProcessorSegment`, etc.) continue to work.
pub(crate) use super::outcome_composition::{
    BodyCoercingSegment, BoxProcessorSegment, StopSegment, compose_outcome_segment,
};

// Task-local cancel token — set by the pipeline task per-start, checked by
// `run_steps` between steps. Absent in direct tests (skip check).
//
// Design: per-start task-local, NOT compiled into the pipeline struct, to
// avoid the lifecycle bug where a compiled-in child token stays cancelled
// after stop→restart (the new start would inherit the cancelled state).
// ADR-0043.
tokio::task_local! {
    pub(crate) static CANCEL_TOKEN: CancellationToken;
}

/// Runtime context for metrics + route_id (B3). Cancel is via task-local (B1).
#[derive(Clone)]
pub struct PipelineRuntimeCtx {
    pub metrics: Arc<dyn MetricsCollector>,
    pub route_id: Arc<str>,
}

impl PipelineRuntimeCtx {
    /// Constructor for compile-time contexts where no MetricsCollector is available.
    /// The resulting pipeline will emit disposition counters to NoOpMetrics (no-op).
    /// Prefer constructing PipelineRuntimeCtx with real metrics at route startup.
    pub fn compile_time() -> Self {
        Self {
            metrics: Arc::new(NoOpMetrics),
            route_id: Arc::from(""),
        }
    }
}

/// Newtype around `Arc<[CompiledStep]>`.
///
/// `CompiledStep` contains `BoxProcessor` (`tower::util::BoxCloneSyncService`),
/// whose erased inner trait object is bounded `Send + Sync`. `CompiledStep` is
/// therefore `Send + Sync` by construction, and `SharedSnapshot` derives both
/// auto traits from `Arc<[CompiledStep]>` — the snapshot is shareable across
/// threads with auto-derived traits alone.
#[derive(Clone)]
struct SharedSnapshot(Arc<[CompiledStep]>);

// Compile-time guard: CompiledStep must remain Send + Sync so the snapshot
// stays shareable via auto-derivation. `Send` keeps the future returned by
// `run_steps` Send; `Sync` covers concurrent `&self` reads on
// `SequentialPipeline`/`TracedPipeline` clones (e.g. `poll_ready` on one
// thread, `call` on another).
#[allow(dead_code)]
const _: () = {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    fn _check() {
        assert_send::<CompiledStep>();
        assert_sync::<CompiledStep>();
    }
};

/// Compose a list of CompiledSteps into a sub-pipeline (EIP internal).
///
/// Uses `into_tower_result()` so `PipelineOutcome::Stopped` maps to `Ok(ex)`.
/// Use [`compose_pipeline_with_handler`] for the top-level consumer-facing pipeline.
pub fn compose_pipeline(processors: Vec<CompiledStep>, ctx: PipelineRuntimeCtx) -> BoxProcessor {
    if processors.is_empty() {
        return BoxProcessor::new(IdentityProcessor);
    }
    BoxProcessor::new(SequentialPipeline {
        steps: SharedSnapshot(processors.into()),
        handler: None,
        ctx,
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
    ctx: PipelineRuntimeCtx,
) -> BoxProcessor {
    if processors.is_empty() {
        return BoxProcessor::new(IdentityProcessor);
    }
    BoxProcessor::new(SequentialPipeline {
        steps: SharedSnapshot(processors.into()),
        handler,
        ctx,
    })
}

/// Compose a list of CompiledSteps into a traced pipeline with Stop→Ok translation.
///
/// Each processor is wrapped with TracingProcessor to emit spans for observability,
/// and the pipeline opens one Internal route root span per invocation (named after
/// `route_id`) that parents every step span. Step spans are named
/// `{route_id}:{label}` when the compiled step carries a DSL label (e.g.
/// `to:direct`, `split`); unlabeled steps fall back to the positional
/// `{route_id}:step-{index}` name. Empty traced routes still return a
/// `TracedPipeline` so the root span records the route invocation with zero steps.
/// When tracing is disabled, falls back to [`compose_pipeline_with_handler`] with zero overhead.
pub fn compose_traced_pipeline(
    processors: Vec<CompiledStep>,
    route_id: &str,
    trace_enabled: bool,
    detail_level: DetailLevel,
    metrics: Option<Arc<dyn MetricsCollector>>,
    handler: Option<Arc<dyn RouteErrorHandler>>,
    ctx: PipelineRuntimeCtx,
) -> BoxProcessor {
    if !trace_enabled {
        return compose_pipeline_with_handler(processors, handler, ctx);
    }

    let wrapped: Vec<CompiledStep> = processors
        .into_iter()
        .enumerate()
        .map(|(idx, step)| {
            let (p, c, lc, lbl) = match step {
                CompiledStep::Process {
                    processor,
                    body_contract,
                    lifecycle,
                    label,
                } => (processor, body_contract, lifecycle, label),
                CompiledStep::Stop => return CompiledStep::Stop,
                CompiledStep::Segment { .. } => return step,
            };
            let traced = BoxProcessor::new(TracingProcessor::new(
                p,
                route_id.to_string(),
                idx,
                detail_level.clone(),
                metrics.clone(),
                lbl.clone(),
            ));
            CompiledStep::Process {
                processor: traced,
                body_contract: c,
                lifecycle: lc,
                label: lbl,
            }
        })
        .collect();

    BoxProcessor::new(TracedPipeline {
        steps: SharedSnapshot(wrapped.into()),
        route_id: route_id.to_string(),
        handler,
        ctx,
    })
}

/// Compose a list of `CompiledStep` items into a single pipeline with body coercion.
///
/// Each processor is optionally wrapped with `BodyCoercingProcessor` based on its
/// contract. Processors with `None` contract are passed through with zero overhead.
/// `CompiledStep::Stop` passes through without coercion.
pub fn compose_pipeline_with_contracts(
    processors: Vec<CompiledStep>,
    handler: Option<Arc<dyn RouteErrorHandler>>,
    ctx: PipelineRuntimeCtx,
) -> BoxProcessor {
    let wrapped: Vec<CompiledStep> = processors
        .into_iter()
        .map(|step| match step {
            CompiledStep::Process {
                processor,
                body_contract,
                lifecycle,
                label,
            } => {
                let coerced = wrap_if_needed(processor, body_contract);
                CompiledStep::Process {
                    processor: coerced,
                    body_contract: None,
                    lifecycle,
                    label,
                }
            }
            CompiledStep::Stop => CompiledStep::Stop,
            CompiledStep::Segment { .. } => step,
        })
        .collect();
    compose_pipeline_with_handler(wrapped, handler, ctx)
}

/// Compose a list of `CompiledStep` items into a traced pipeline with body coercion.
///
/// Applies body coercion contracts first, then wraps with `TracingProcessor`.
/// The pipeline opens one Internal route root span per invocation (named after
/// `route_id`); empty traced routes still return a `TracedPipeline` so the
/// root span records the route invocation with zero steps.
/// When tracing is disabled, falls back to [`compose_pipeline_with_contracts`].
pub(crate) fn compose_traced_pipeline_with_contracts(
    processors: Vec<CompiledStep>,
    route_id: &str,
    trace_enabled: bool,
    detail_level: DetailLevel,
    metrics: Option<Arc<dyn MetricsCollector>>,
    handler: Option<Arc<dyn RouteErrorHandler>>,
    ctx: PipelineRuntimeCtx,
) -> BoxProcessor {
    if !trace_enabled {
        return compose_pipeline_with_contracts(processors, handler, ctx);
    }

    let coerced: Vec<CompiledStep> = processors
        .into_iter()
        .map(|step| match step {
            CompiledStep::Process {
                processor,
                body_contract,
                lifecycle,
                label,
            } => {
                let processor = wrap_if_needed(processor, body_contract);
                CompiledStep::Process {
                    processor,
                    body_contract: None,
                    lifecycle,
                    label,
                }
            }
            CompiledStep::Stop => CompiledStep::Stop,
            CompiledStep::Segment { .. } => step,
        })
        .collect();

    compose_traced_pipeline(
        coerced,
        route_id,
        trace_enabled,
        detail_level,
        metrics,
        handler,
        ctx,
    )
}

/// A service that executes a sequence of CompiledSteps in order.
///
/// Uses `into_tower_result()` so `PipelineOutcome::Stopped(ex)` maps to
/// `Ok(ex)` — the Bug B fix that makes Stop indistinguishable from Completed
/// at the consumer boundary.
#[derive(Clone)]
struct SequentialPipeline {
    steps: SharedSnapshot,
    handler: Option<Arc<dyn RouteErrorHandler>>,
    ctx: PipelineRuntimeCtx,
}

impl Service<Exchange> for SequentialPipeline {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.steps.0.first() {
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
        // Cheap Arc::clone (refcount bump) on the SharedSnapshot newtype.
        // `SharedSnapshot: Send` so the future returned by `run_steps`
        // captures it directly without needing a Send-asserting wrapper.
        let steps = self.steps.clone();
        let handler = self.handler.clone();
        let ctx = self.ctx.clone();
        Box::pin(async move {
            run_steps(steps, exchange, handler, false, &ctx.route_id, &ctx)
                .await
                .into_tower_result()
        })
    }
}

/// A traced service pipeline for wrapped CompiledSteps.
///
/// Each invocation opens one Internal route root span named after the route;
/// step spans (from `TracingProcessor`) nest under it. The root span handle
/// lives inside the `call` async body — never on `self` — so hot-reload
/// pipeline swaps are unaffected.
#[derive(Clone)]
struct TracedPipeline {
    steps: SharedSnapshot,
    route_id: String,
    handler: Option<Arc<dyn RouteErrorHandler>>,
    ctx: PipelineRuntimeCtx,
}

impl Service<Exchange> for TracedPipeline {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.steps.0.first() {
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
    //
    // Route root span (trace-model-tree T1.3): one Internal span per route
    // invocation, named after the route, parenting every step span. Derived
    // from the entry context (not the ambient current context) so parent
    // entries such as baggage stay attached; the entry context is restored
    // on the result exchange when one comes back.
    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let steps = self.steps.clone();
        let route_id = self.route_id.clone();
        let handler = self.handler.clone();
        let ctx = self.ctx.clone();
        Box::pin(async move {
            let tracer = global::tracer_with_scope(
                InstrumentationScope::builder("camel-core")
                    .with_version(env!("CARGO_PKG_VERSION"))
                    .build(),
            );
            let entry_cx = exchange.otel_context.clone();
            let root_span = tracer
                .span_builder(route_id.clone())
                .with_kind(SpanKind::Internal)
                .with_attributes([
                    KeyValue::new("messaging.system", "camel"),
                    KeyValue::new("route_id", route_id.clone()),
                    KeyValue::new(
                        "correlation_id",
                        capped_correlation_id(exchange.correlation_id()).to_string(),
                    ),
                ])
                .start_with_context(&tracer, &entry_cx);
            let root_cx = entry_cx.with_span(root_span);
            // Guard ends the root span even if a step panics.
            let _root_guard = SpanEndGuard(root_cx.clone());
            let mut exchange = exchange;
            exchange.otel_context = root_cx.clone();

            let outcome = run_steps(steps, exchange, handler, true, &route_id, &ctx).await;
            finish_span_outcome(outcome, &root_cx, entry_cx).into_tower_result()
        })
    }
}

/// Run a sequence of CompiledSteps with optional error recovery.
///
/// Each step is unified under [`OwnedRetryable`] — Process and
/// Segment variants are treated uniformly via a stack-allocated enum
/// that dispatches to the existing `RetryableStep` impls on
/// `BoxProcessor` and `OutcomeSegment`. This eliminates the per-step
/// `Box::new(...) as Box<dyn RetryableStep>` heap allocation that the
/// pre-A2 implementation paid for every step of every Exchange (A2).
///
/// On the traced path (`trace == true`, `route_id` from the traced
/// pipeline), Segment steps dispatch through [`TracedSegmentStep`]
/// instead, so the initial invocation AND every retry attempt opened by
/// the error handler runs through the same span wrapper (T1.4).
///
/// On failure:
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
async fn run_steps(
    steps: SharedSnapshot,
    exchange: Exchange,
    handler: Option<Arc<dyn RouteErrorHandler>>,
    trace: bool,
    route_id: &str,
    ctx: &PipelineRuntimeCtx,
) -> PipelineOutcome {
    use camel_api::error_handler::RetryableStep;
    let mut ex = exchange;
    // Index-based loop (not `for (i, step) in steps.0.iter().enumerate()`):
    // retained to avoid holding a `&[CompiledStep]` borrow across the
    // `.await` below — `&steps.0[i]` is consumed by the `match` scrutinee
    // and drops before the await, so no borrow is live across the await
    // point. The original `CompiledStep: !Sync` rationale is gone
    // (`BoxProcessor` is now `Send + Sync` via `BoxCloneSyncService`);
    // the loop shape is kept purely for borrow hygiene — no behavior change.
    let len = steps.0.len();
    for i in 0..len {
        // B1: cooperative cancellation between steps via task-local.
        // If the task-local is not set (direct test calls), skip the check.
        let cancelled = CANCEL_TOKEN.try_with(|t| t.is_cancelled()).unwrap_or(false);
        if cancelled {
            return PipelineOutcome::Failed(CamelError::ConsumerStopping);
        }
        // A2: dispatch to existing `RetryableStep` impls through a stack
        // enum instead of paying `Box::new(...) as Box<dyn RetryableStep>`
        // per step. `OwnedRetryable` is `enum { Processor, Segment }` with
        // discriminant-by-value layout — no extra heap alloc. On the traced
        // path, Segment steps take the `TracedSegment` variant so every
        // attempt (initial + retries) gets its own step span (T1.4).
        let mut retryable: OwnedRetryable = match &steps.0[i] {
            CompiledStep::Stop => return PipelineOutcome::Stopped(ex),
            CompiledStep::Process { processor, .. } => OwnedRetryable::Processor(processor.clone()),
            CompiledStep::Segment { segment, label, .. } => {
                if trace {
                    OwnedRetryable::TracedSegment(TracedSegmentStep {
                        segment: segment.clone(),
                        route_id: route_id.to_string(),
                        index: i,
                        label: label.clone(),
                    })
                } else {
                    OwnedRetryable::Segment(segment.clone())
                }
            }
        };

        let original = handler.as_ref().map(|_| ex.clone());
        let outcome = if trace {
            invoke_with_span(&mut retryable, ex, i).await
        } else {
            retryable.invoke(ex).await
        };

        match outcome {
            PipelineOutcome::Completed(next) => {
                if camel_api::is_camel_stop(&next) {
                    return PipelineOutcome::Stopped(next);
                }
                ex = next;
            }
            PipelineOutcome::Stopped(stopped_ex) => {
                return PipelineOutcome::Stopped(stopped_ex);
            }
            PipelineOutcome::Failed(err) => {
                let (Some(handler), Some(original)) = (handler.as_ref(), original) else {
                    return PipelineOutcome::Failed(err);
                };
                let policy = handler.match_policy(&err);
                // `&mut retryable` auto-coerces from `&mut OwnedRetryable` to
                // `&mut dyn RetryableStep` via the trait impl on the enum.
                match handler
                    .retry_step(policy, &mut retryable, original, err)
                    .await
                {
                    RetryOutcome::Recovered(exchange) => {
                        ctx.metrics.record_counter(
                            "pipeline_disposition",
                            1.0,
                            &[("disposition", "recovered"), ("route_id", &ctx.route_id)],
                        );
                        ex = exchange;
                    }
                    RetryOutcome::Stopped(stopped_ex) => {
                        ctx.metrics.record_counter(
                            "pipeline_disposition",
                            1.0,
                            &[("disposition", "stopped"), ("route_id", &ctx.route_id)],
                        );
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
                                ctx.metrics.record_counter(
                                    "pipeline_disposition",
                                    1.0,
                                    &[("disposition", "propagated"), ("route_id", &ctx.route_id)],
                                );
                                return PipelineOutcome::Failed(e);
                            }
                            Ok(StepDisposition::Handled(done)) => {
                                ctx.metrics.record_counter(
                                    "pipeline_disposition",
                                    1.0,
                                    &[("disposition", "handled"), ("route_id", &ctx.route_id)],
                                );
                                return PipelineOutcome::Completed(done);
                            }
                            Ok(StepDisposition::Continued(next)) => {
                                ctx.metrics.record_counter(
                                    "pipeline_disposition",
                                    1.0,
                                    &[("disposition", "continued"), ("route_id", &ctx.route_id)],
                                );
                                ex = next;
                            }
                            Err(e) => {
                                ctx.metrics.record_counter(
                                    "pipeline_disposition",
                                    1.0,
                                    &[
                                        ("disposition", "handler_error"),
                                        ("route_id", &ctx.route_id),
                                    ],
                                );
                                return PipelineOutcome::Failed(e);
                            }
                            // Future StepDisposition variants fail the pipeline.
                            _ => {
                                return PipelineOutcome::Failed(CamelError::ProcessorError(
                                    "unknown step disposition".to_string(),
                                ));
                            }
                        }
                    }
                    // Future RetryOutcome variants fail the pipeline.
                    _ => {
                        return PipelineOutcome::Failed(CamelError::ProcessorError(
                            "unknown retry outcome".to_string(),
                        ));
                    }
                }
            }
        }
    }
    PipelineOutcome::Completed(ex)
}

/// Stack-allocated dispatcher that unifies `BoxProcessor` and
/// `OutcomeSegment` for the retry path without the heap allocation a
/// `Box<dyn RetryableStep>` would require. Sized by-value, dispatched
/// through a single trait method that fans out to the existing
/// `RetryableStep` impls on each variant.
///
/// A2: replaces `Box::new(processor.clone()) as Box<dyn RetryableStep>`
/// (and the equivalent for segments) with this enum, saving one heap
/// allocation per pipeline step per Exchange invocation.
enum OwnedRetryable {
    Processor(camel_api::BoxProcessor),
    Segment(camel_api::OutcomeSegment),
    /// Traced segment dispatch (T1.4): every attempt — the initial
    /// invocation and each retry opened by the error handler — goes
    /// through `TracedSegmentStep` so each gets its own step span.
    TracedSegment(TracedSegmentStep),
}

impl camel_api::error_handler::RetryableStep for OwnedRetryable {
    fn invoke<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        match self {
            OwnedRetryable::Processor(p) => p.invoke(exchange),
            OwnedRetryable::Segment(s) => s.invoke(exchange),
            OwnedRetryable::TracedSegment(s) => s.invoke(exchange),
        }
    }
}

/// Start an Internal span for one segment step attempt, parented by
/// `entry_cx` (the traced pipeline's root context) with the Minimal-level
/// attribute set from `step_span_attributes` (trace-model-tree T1.4).
///
/// Named `{route_id}:{label}` when the segment step carries a DSL label
/// (e.g. `split`); unlabeled segments fall back to the positional
/// `{route_id}:step-{index}` name — same contract as process step spans.
fn segment_span(
    tracer: &global::BoxedTracer,
    route_id: &str,
    index: usize,
    label: Option<Arc<str>>,
    entry_cx: &OtelContext,
    correlation_id: &str,
) -> global::BoxedSpan {
    tracer
        .span_builder(format!(
            "{route_id}:{}",
            label.as_deref().unwrap_or(&step_id_for(index))
        ))
        .with_kind(SpanKind::Internal)
        .with_attributes(step_span_attributes(route_id, index, correlation_id))
        .start_with_context(tracer, entry_cx)
}

/// Per-attempt span adapter for `CompiledStep::Segment` on traced
/// pipelines (trace-model-tree T1.4).
///
/// Implements `RetryableStep` so BOTH the initial invocation and every
/// retry attempt dispatched by `RouteErrorHandler::retry_step` run
/// through the same wrapper: each `invoke` opens one fresh Internal span
/// parented by the incoming context (the route root), named
/// `{route_id}:{label}` when the segment step carries a DSL label (e.g.
/// `split`) and `{route_id}:step-{index}` otherwise. It runs the inner
/// segment with that span active, restores the incoming context on
/// outcomes that carry the exchange, and ends the span with the future —
/// spans never outlive the attempt.
///
/// Retry inputs are the error handler's preserved pre-attempt exchange,
/// which still carries the route root context (restored by a previous
/// attempt's Ok path, or never left on the first attempt), so every
/// attempt span nests under the route root, not under each other.
struct TracedSegmentStep {
    segment: camel_api::OutcomeSegment,
    route_id: String,
    index: usize,
    label: Option<Arc<str>>,
}

fn finish_span_outcome(
    outcome: PipelineOutcome,
    span_cx: &OtelContext,
    entry_cx: OtelContext,
) -> PipelineOutcome {
    match outcome {
        PipelineOutcome::Completed(mut ex) => {
            span_cx.span().set_status(Status::Ok);
            ex.otel_context = entry_cx;
            PipelineOutcome::Completed(ex)
        }
        PipelineOutcome::Stopped(mut ex) => {
            span_cx.span().set_status(Status::Ok);
            ex.otel_context = entry_cx;
            PipelineOutcome::Stopped(ex)
        }
        PipelineOutcome::Failed(e) => {
            record_exception(&span_cx.span(), &e);
            PipelineOutcome::Failed(e)
        }
    }
}

impl camel_api::error_handler::RetryableStep for TracedSegmentStep {
    fn invoke<'a>(
        &'a mut self,
        mut exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        Box::pin(async move {
            let tracer = global::tracer_with_scope(
                InstrumentationScope::builder("camel-core")
                    .with_version(env!("CARGO_PKG_VERSION"))
                    .build(),
            );
            let entry_cx = exchange.otel_context.clone();
            let span = segment_span(
                &tracer,
                &self.route_id,
                self.index,
                self.label.clone(),
                &entry_cx,
                exchange.correlation_id(),
            );
            let cx = entry_cx.with_span(span);
            // Guard ends the attempt span even if the segment panics.
            let _guard = SpanEndGuard(cx.clone());
            exchange.otel_context = cx.clone();
            finish_span_outcome(self.segment.run(exchange).await, &cx, entry_cx)
        })
    }
}

async fn invoke_with_span(
    retryable: &mut dyn camel_api::error_handler::RetryableStep,
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
    /// When true, stash the original Message as `ORIGINAL_MESSAGE_EXTENSION`
    /// before any gate runs, so the error handler can restore it on failure.
    use_original_message: bool,
}

impl RouteChannelService {
    pub fn new(
        handler: Arc<dyn RouteErrorHandler>,
        security: Option<BoxProcessor>,
        cb_gate: Option<CircuitBreakerGate>,
        pipeline: BoxProcessor,
        use_original_message: bool,
    ) -> Self {
        Self {
            handler,
            security,
            cb_gate,
            pipeline,
            use_original_message,
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
        let use_original_message = self.use_original_message;

        Box::pin(async move {
            let mut ex = exchange;

            // Stash original message for use_original_message support.
            // Done BEFORE any gate so the DLC can restore the pre-route message.
            // Only stashes when the flag is true to avoid perf regression on every Exchange.
            if use_original_message {
                let original: Arc<Message> = Arc::new(ex.input.clone());
                ex.set_extension(ORIGINAL_MESSAGE_EXTENSION, original);
            }

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
