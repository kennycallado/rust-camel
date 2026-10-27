//! Test-only adapter from `BoxProcessor` to `OutcomePipeline`.
//!
//! `#[cfg(test)]`-gated and `pub(crate)`: lets Segment tests keep the same
//! `BoxProcessor` fixtures that the deleted Tower services used, wrapped as an
//! `OutcomeSegment` body.

use std::future::Future;
use std::pin::Pin;

use camel_api::{BoxProcessor, Exchange, OutcomePipeline, OutcomeSegment, PipelineOutcome};

struct ProcessorSegment(BoxProcessor);

impl Clone for ProcessorSegment {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl ProcessorSegment {
    fn wrap(processor: BoxProcessor) -> OutcomeSegment {
        OutcomeSegment::new(Box::new(Self(processor)))
    }
}

impl OutcomePipeline for ProcessorSegment {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        Box::pin(async move {
            match tower::ServiceExt::oneshot(self.0.clone(), exchange).await {
                Ok(exchange) => PipelineOutcome::Completed(exchange),
                Err(err) => PipelineOutcome::Failed(err),
            }
        })
    }
}

/// Wrap a processor as an `OutcomeSegment` body for Segment tests.
pub(crate) fn processor_segment(processor: BoxProcessor) -> OutcomeSegment {
    ProcessorSegment::wrap(processor)
}
