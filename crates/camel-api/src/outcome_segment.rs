//! Wrapper around `Box<dyn OutcomePipeline>` with extension hooks for
//! tracing and metrics. Lives in camel-api (NOT camel-core) so EIP
//! segment structs in camel-processor can type their fields as
//! `camel_api::OutcomeSegment` (camel-processor depends on camel-api,
//! not camel-core). See ADR-0025.

use crate::exchange::Exchange;
use crate::outcome_pipeline::OutcomePipeline;
use crate::pipeline_outcome::PipelineOutcome;
use std::future::Future;
use std::pin::Pin;

/// Wrapper around `Box<dyn OutcomePipeline>` with extension hooks for
/// tracing and metrics. Constructed via `OutcomeSegment::new(...)` and
/// optionally enriched via `.with_tracing(route_id, idx, metrics)`.
#[derive(Clone)]
pub struct OutcomeSegment {
    inner: Box<dyn OutcomePipeline>,
}

impl std::fmt::Debug for OutcomeSegment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutcomeSegment").finish_non_exhaustive()
    }
}

impl OutcomeSegment {
    pub fn new(inner: Box<dyn OutcomePipeline>) -> Self {
        Self { inner }
    }

    pub fn with_tracing(
        self,
        _route_id: &str,
        _idx: usize,
        _metrics: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
    ) -> Self {
        self
    }

    pub async fn run(&mut self, exchange: Exchange) -> PipelineOutcome {
        self.inner.run(exchange).await
    }
}

/// OutcomeSegment implements RetryableStep so it can participate in the
/// retry path alongside BoxProcessor. Lives in camel-api (where both
/// OutcomeSegment and RetryableStep are defined).
impl crate::error_handler::RetryableStep for OutcomeSegment {
    fn invoke<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        Box::pin(async move { self.run(exchange).await })
    }
}

/// OutcomeSegment implements OutcomePipeline by delegating to its inner
/// `Box<dyn OutcomePipeline>`. `CompiledStep::Segment` can now produce
/// `segment` directly as `Box<dyn OutcomePipeline>`.
impl OutcomePipeline for OutcomeSegment {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        // Call the inherent async `run` method, NOT the trait method.
        // `OutcomeSegment::run` is unambiguous: inherent methods shadow
        // trait methods in Rust name resolution for plain method calls,
        // and the qualified `OutcomeSegment::run(self, exchange)` syntax
        // also selects the inherent fn.
        Box::pin(OutcomeSegment::run(self, exchange))
    }
}
