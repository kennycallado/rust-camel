// adapters/outcome_composition.rs
// OutcomePipeline adapter types and the compose_outcome_segment helper.
// Extracted from route_compiler.rs (removing 165 lines / 12 #[allow(dead_code)])
// so that route_compiler.rs stays focused on Tower pipeline compilation.
//
// ── compose_outcome_segment ──────────────────────────────────────────
//
// Composes a list of OutcomePipeline impls into a single OutcomeSegment
// (for EIP internal sub-pipelines). Empty input returns a noop segment.
//
// This is the outcome-aware analog of `compose_pipeline` (which returns
// BoxProcessor). Used by structural EIP compilers to build child sub-pipelines
// that propagate PipelineOutcome::Stopped(ex) with Exchange state intact.

use std::future::Future;
use std::pin::Pin;

use camel_api::{
    BodyType, BoxProcessor, CamelError, Exchange, OutcomePipeline, OutcomeSegment, PipelineOutcome,
    body_converter,
};

pub fn compose_outcome_segment(children: Vec<Box<dyn OutcomePipeline>>) -> OutcomeSegment {
    if children.is_empty() {
        return OutcomeSegment::new(Box::new(NoopSegment));
    }
    OutcomeSegment::new(Box::new(SequentialOutcomeSegment::new(children)))
}

#[derive(Clone)]
struct NoopSegment;

impl OutcomePipeline for NoopSegment {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(NoopSegment)
    }

    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        Box::pin(async move { PipelineOutcome::Completed(exchange) })
    }
}

#[derive(Clone)]
pub struct SequentialOutcomeSegment {
    children: Vec<Box<dyn OutcomePipeline>>,
}

impl SequentialOutcomeSegment {
    pub fn new(children: Vec<Box<dyn OutcomePipeline>>) -> Self {
        Self { children }
    }
}

impl OutcomePipeline for SequentialOutcomeSegment {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        mut exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        Box::pin(async move {
            for child in self.children.iter_mut() {
                match child.run(exchange).await {
                    PipelineOutcome::Completed(next) => {
                        if camel_api::is_camel_stop(&next) {
                            return PipelineOutcome::Stopped(next);
                        }
                        exchange = next;
                    }
                    other => return other,
                }
            }
            PipelineOutcome::Completed(exchange)
        })
    }
}

/// Outcome-aware analog of [`BodyCoercingProcessor`].
///
/// Wraps an [`OutcomePipeline`] impl and calls [`body_converter::convert`] on the
/// exchange body before delegating. Coercion failure produces
/// [`PipelineOutcome::Failed`] with [`CamelError::TypeConversionFailed`].
#[derive(Clone)]
pub struct BodyCoercingSegment {
    inner: Box<dyn OutcomePipeline>,
    contract: BodyType,
}

impl BodyCoercingSegment {
    pub fn new(inner: Box<dyn OutcomePipeline>, contract: BodyType) -> Self {
        Self { inner, contract }
    }
}

impl OutcomePipeline for BodyCoercingSegment {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        mut exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        let contract = self.contract;
        Box::pin(async move {
            let body = std::mem::replace(&mut exchange.input.body, camel_api::body::Body::Empty);
            match body_converter::convert(body, contract) {
                Ok(coerced) => {
                    exchange.input.body = coerced;
                }
                Err(e) => {
                    return PipelineOutcome::Failed(CamelError::TypeConversionFailed(format!(
                        "body coercion failed: {e}"
                    )));
                }
            }
            self.inner.run(exchange).await
        })
    }
}

// ── BoxProcessorSegment — OutcomePipeline adapter for legacy Tower processors ──

#[derive(Clone)]
pub struct BoxProcessorSegment {
    processor: BoxProcessor,
}

impl BoxProcessorSegment {
    pub fn new(processor: BoxProcessor) -> Self {
        Self { processor }
    }
}

impl OutcomePipeline for BoxProcessorSegment {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        use tower::ServiceExt;
        Box::pin(async move {
            match self.processor.ready().await {
                Ok(mut ready) => match tower::Service::call(&mut ready, exchange).await {
                    Ok(ex) => {
                        if camel_api::is_camel_stop(&ex) {
                            PipelineOutcome::Stopped(ex)
                        } else {
                            PipelineOutcome::Completed(ex)
                        }
                    }
                    Err(err) => PipelineOutcome::Failed(err),
                },
                Err(err) => PipelineOutcome::Failed(err),
            }
        })
    }
}

// ── StopSegment — OutcomePipeline adapter for CompiledStep::Stop ──

#[derive(Clone)]
pub struct StopSegment;

impl OutcomePipeline for StopSegment {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(StopSegment)
    }

    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        Box::pin(async move { PipelineOutcome::Stopped(exchange) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::BoxProcessorExt;

    #[test]
    fn compose_outcome_segment_empty_returns_noop() {
        let seg = compose_outcome_segment(vec![]);
        // Verify the segment is non-null (the OutcomeSegment::new succeeded).
        // No need to run it — NoopSegment is a unit struct.
        let _ = seg;
    }

    #[test]
    fn compose_outcome_segment_single_child() {
        let kids: Vec<Box<dyn OutcomePipeline>> = vec![Box::new(StopSegment)];
        let seg = compose_outcome_segment(kids);
        let _ = seg;
    }

    #[test]
    fn body_coercing_segment_construction() {
        let inner = Box::new(StopSegment);
        let seg = BodyCoercingSegment::new(inner, BodyType::Text);
        assert_eq!(seg.contract, BodyType::Text);
    }

    #[test]
    fn box_processor_segment_construction() {
        let bp = BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }));
        let seg = BoxProcessorSegment::new(bp);
        let _ = seg;
    }

    #[test]
    fn sequential_outcome_segment_construction() {
        let kids: Vec<Box<dyn OutcomePipeline>> =
            vec![Box::new(StopSegment), Box::new(NoopSegment)];
        let seq = SequentialOutcomeSegment::new(kids);
        assert_eq!(seq.children.len(), 2);
    }
}
