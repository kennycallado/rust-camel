//! Per-item trace restart for split bodies (splittrace 2.1).
//!
//! [`TraceRestartBody`] wraps a split body segment and mints one linked
//! root span per fragment when the fragment count exceeds the configured
//! threshold. Policy:
//!
//! - Fragment count `<= threshold` (or missing `CamelSplitSize` metadata):
//!   the body runs unchanged in the caller's context — nested spans, zero
//!   span overhead. This is the legacy shape and the shape small splits
//!   keep.
//! - Fragment count `> threshold`: every fragment gets its own root span
//!   (`{route_id}:split-item`), started on an EMPTY `OtelContext` (fresh
//!   trace id, no parent span id) with exactly one link back to the origin
//!   context's span (the `{route_id}:split` segment span). The fragment
//!   body runs inside the item span; on completion the origin context is
//!   restored on the outcome's exchange so the outer route continues on
//!   its original trace.
//!
//! The threshold gate lives at compile time (`step_compilers/splitting.rs`):
//! `trace_item_threshold >= 1` wraps, `0` never does.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use camel_api::{Exchange, OutcomeSegment, PipelineOutcome};
use opentelemetry::trace::{Link, SpanKind, Status, TraceContextExt, Tracer};
use opentelemetry::{Context as OtelContext, InstrumentationScope, KeyValue, global};

use crate::shared::observability::adapters::tracer::{SpanEndGuard, record_exception};

/// Per-fragment body wrapper that restarts traces above the split threshold.
#[derive(Clone)]
pub(crate) struct TraceRestartBody {
    inner: OutcomeSegment,
    route_id: Arc<str>,
    threshold: usize,
}

impl TraceRestartBody {
    /// Wrap `inner` so fragment counts above `threshold` restart traces.
    pub(crate) fn wrap(
        inner: OutcomeSegment,
        route_id: Arc<str>,
        threshold: usize,
    ) -> OutcomeSegment {
        OutcomeSegment::new(Box::new(TraceRestartBody {
            inner,
            route_id,
            threshold,
        }))
    }

    /// Read a `u64` split-metadata property stamped by `SplitSegment`.
    ///
    /// Missing or non-numeric values read as "below threshold": legacy
    /// producers that never stamp the metadata keep the nested shape.
    fn split_u64(exchange: &Exchange, key: &str) -> Option<u64> {
        match exchange.property(key) {
            Some(camel_api::Value::Number(n)) => n.as_u64(),
            _ => None,
        }
    }
}

impl camel_api::OutcomePipeline for TraceRestartBody {
    fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        mut exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        Box::pin(async move {
            let Some(total) =
                Self::split_u64(&exchange, camel_processor::splitter::CAMEL_SPLIT_SIZE)
            else {
                return self.inner.run(exchange).await;
            };
            if total as usize <= self.threshold {
                return self.inner.run(exchange).await;
            }
            let index = Self::split_u64(&exchange, camel_processor::splitter::CAMEL_SPLIT_INDEX)
                .unwrap_or_default();

            let origin_cx = exchange.otel_context.clone();
            let origin_sc = origin_cx.span().span_context().clone();
            // Same scope value as `segment_span` in route_compiler.rs so the
            // item roots carry the crate's instrumentation identity.
            let tracer = global::tracer_with_scope(
                InstrumentationScope::builder("camel-core")
                    .with_version(env!("CARGO_PKG_VERSION"))
                    .build(),
            );
            let builder = tracer
                .span_builder(format!("{}:split-item", self.route_id))
                .with_kind(SpanKind::Internal)
                .with_attributes(vec![
                    KeyValue::new("split.item.index", index as i64),
                    KeyValue::new("split.item.total", total as i64),
                ])
                .with_links(vec![Link::new(origin_sc, Vec::new(), 0)]);
            // Empty parent context: fresh trace id, no parent span id.
            let span = tracer.build_with_context(builder, &OtelContext::new());
            let cx = OtelContext::new().with_span(span);
            // Guard ends the item span even if the body panics.
            let _guard = SpanEndGuard(cx.clone());
            exchange.otel_context = cx.clone();

            // Same outcome semantics as `finish_span_outcome` in
            // route_compiler.rs: Ok on Completed/Stopped with the entry
            // context restored on the exchange, exception recorded on Failed.
            match self.inner.run(exchange).await {
                PipelineOutcome::Completed(mut ex) => {
                    cx.span().set_status(Status::Ok);
                    ex.otel_context = origin_cx;
                    PipelineOutcome::Completed(ex)
                }
                PipelineOutcome::Stopped(mut ex) => {
                    cx.span().set_status(Status::Ok);
                    ex.otel_context = origin_cx;
                    PipelineOutcome::Stopped(ex)
                }
                PipelineOutcome::Failed(e) => {
                    record_exception(&cx.span(), &e);
                    PipelineOutcome::Failed(e)
                }
            }
        })
    }
}
