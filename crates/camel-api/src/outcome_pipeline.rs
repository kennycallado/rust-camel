//! Outcome-aware internal pipeline trait. Lives one layer above Tower (ADR-0024 §3).
//!
//! Structural EIPs with child sub-pipelines implement this trait; the top-level
//! route pipeline still uses `BoxProcessor` (`Service<Exchange, Response=Exchange,
//! Error=CamelError>`). See ADR-0025.

use crate::exchange::Exchange;
use crate::pipeline_outcome::PipelineOutcome;
use std::future::Future;
use std::pin::Pin;

/// Outcome-aware internal pipeline. Object-safe via `clone_box`.
///
/// `run` returns `PipelineOutcome` directly (NOT `Result<Exchange, CamelError>`).
/// This is the core abstraction that lets structural EIPs propagate
/// `PipelineOutcome::Stopped(ex)` with the Exchange intact across sub-pipeline
/// boundaries — the core fix for Option E.
///
/// Implementations include: `FilterSegment`, `ChoiceSegment`, `LoopSegment`,
/// `ThrottleSegment`, `DoTrySegment`, `SplitSegment`, `StreamingSplitSegment`,
/// `MulticastSegment`, `LoadBalanceSegment`, plus `BodyCoercingSegment` wrapper.
///
/// See ADR-0025 for full design rationale.
pub trait OutcomePipeline: Send + 'static {
    /// Clone the segment into a new boxed instance. Required because
    /// `Box<dyn OutcomePipeline>` cannot directly derive `Clone`.
    fn clone_box(&self) -> Box<dyn OutcomePipeline>;

    /// Execute the segment against `exchange`, returning a `PipelineOutcome`.
    ///
    /// `Stopped(ex)` MUST be returned with the Exchange at the Stop point
    /// (including mutations made inside this segment before Stop fired).
    /// This is the **stopped-exchange-state-preservation invariant** (ADR-0025 §3).
    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>>;
}

impl Clone for Box<dyn OutcomePipeline> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exchange::Exchange;
    use crate::message::Message;
    use crate::pipeline_outcome::PipelineOutcome;

    struct EchoSegment;

    impl OutcomePipeline for EchoSegment {
        fn clone_box(&self) -> Box<dyn OutcomePipeline> {
            Box::new(EchoSegment)
        }
        fn run<'a>(
            &'a mut self,
            exchange: Exchange,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PipelineOutcome> + Send + 'a>>
        {
            Box::pin(async move { PipelineOutcome::Completed(exchange) })
        }
    }

    #[tokio::test]
    async fn clone_box_produces_equivalent_segment() {
        let mut seg: Box<dyn OutcomePipeline> = Box::new(EchoSegment);
        let ex = Exchange::new(Message::new("hello"));
        let out1 = seg.run(ex.clone()).await;
        let mut cloned = seg.clone_box();
        let out2 = cloned.run(ex).await;
        assert!(matches!(out1, PipelineOutcome::Completed(_)));
        assert!(matches!(out2, PipelineOutcome::Completed(_)));
    }

    #[tokio::test]
    async fn boxed_clone_works_via_trait() {
        let seg: Box<dyn OutcomePipeline> = Box::new(EchoSegment);
        let _cloned: Box<dyn OutcomePipeline> = seg.clone();
    }
}
