use std::future::Future;
use std::pin::Pin;

// ── FilterSegment (ADR-0025 OutcomePipeline) ─────────────────────────────

/// Outcome-aware structural EIP segment for the Filter pattern.
///
/// When the predicate passes, delegates to `body` (which can return
/// `Completed`, `Stopped`, or `Failed`). When the predicate evaluates to
/// `false`, returns `Completed(original_exchange)` — the exchange is returned
/// as-is and the body is skipped entirely. A failed predicate evaluation
/// yields `Failed(err)`; the exchange is never silently dropped.
///
/// The segment operates at the `PipelineOutcome` layer and preserves the
/// exchange at the Stop point including all mutations.
pub struct FilterSegment {
    pub predicate: camel_api::PredicateSource,
    pub body: camel_api::OutcomeSegment,
}

impl Clone for FilterSegment {
    fn clone(&self) -> Self {
        Self {
            predicate: self.predicate.clone(),
            body: self.body.clone(),
        }
    }
}

impl camel_api::OutcomePipeline for FilterSegment {
    fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: camel_api::Exchange,
    ) -> Pin<Box<dyn Future<Output = camel_api::PipelineOutcome> + Send + 'a>> {
        Box::pin(async move {
            match self.predicate.matches(&exchange).await {
                Ok(true) => self.body.run(exchange).await,
                Ok(false) => camel_api::PipelineOutcome::Completed(exchange),
                Err(err) => camel_api::PipelineOutcome::Failed(err),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_processor_segment::processor_segment;
    use camel_api::{
        Body, BoxProcessor, BoxProcessorExt, CamelError, Exchange, FilterPredicate, Message,
        OutcomePipeline, PipelineOutcome, PredicateSource, Value,
    };

    fn passthrough() -> BoxProcessor {
        BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }))
    }

    fn uppercase_body() -> BoxProcessor {
        BoxProcessor::from_fn(|mut ex: Exchange| {
            Box::pin(async move {
                if let Body::Text(s) = &ex.input.body {
                    ex.input.body = Body::Text(s.to_uppercase());
                }
                Ok(ex)
            })
        })
    }

    fn failing() -> BoxProcessor {
        BoxProcessor::from_fn(|_ex| {
            Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
        })
    }

    fn predicate(f: impl Fn(&Exchange) -> bool + Send + Sync + 'static) -> PredicateSource {
        PredicateSource::Sync(FilterPredicate::new(f))
    }

    fn segment(predicate: PredicateSource, body: BoxProcessor) -> FilterSegment {
        FilterSegment {
            predicate,
            body: processor_segment(body),
        }
    }

    fn completed_exchange(outcome: PipelineOutcome) -> Exchange {
        match outcome {
            PipelineOutcome::Completed(ex) => ex,
            other => panic!("expected PipelineOutcome::Completed, got {other:?}"),
        }
    }

    // 1. Matching exchange is forwarded to body.
    #[tokio::test]
    async fn filter_segment_passes_matching_exchange() {
        let mut seg = segment(
            predicate(|ex: &Exchange| ex.input.header("active").is_some()),
            uppercase_body(),
        );
        let mut ex = Exchange::new(Message::new("hello"));
        ex.input.set_header("active", Value::Bool(true));
        let result = completed_exchange(seg.run(ex).await);
        assert_eq!(result.input.body.as_text(), Some("HELLO"));
    }

    // 2. Non-matching exchange is returned as-is, body not called.
    #[tokio::test]
    async fn filter_segment_blocks_non_matching_exchange() {
        let mut seg = segment(
            predicate(|ex: &Exchange| ex.input.header("active").is_some()),
            uppercase_body(),
        );
        let ex = Exchange::new(Message::new("hello"));
        let result = completed_exchange(seg.run(ex).await);
        // body unchanged — uppercase_body body was NOT called
        assert_eq!(result.input.body.as_text(), Some("hello"));
    }

    // 3. Result is the body's output, not the original exchange.
    #[tokio::test]
    async fn filter_segment_sub_pipeline_transforms_body() {
        let mut seg = segment(predicate(|_: &Exchange| true), uppercase_body());
        let ex = Exchange::new(Message::new("world"));
        let result = completed_exchange(seg.run(ex).await);
        assert_eq!(result.input.body.as_text(), Some("WORLD"));
    }

    // 4. Body errors surface as Failed.
    #[tokio::test]
    async fn filter_segment_sub_pipeline_error_propagates() {
        let mut seg = segment(predicate(|_: &Exchange| true), failing());
        let ex = Exchange::new(Message::new("x"));
        match seg.run(ex).await {
            PipelineOutcome::Failed(err) => {
                assert!(err.to_string().contains("boom"), "unexpected error: {err}");
            }
            other => panic!("expected PipelineOutcome::Failed, got {other:?}"),
        }
    }

    // 5. Predicate receives the original exchange before the body mutates it.
    #[tokio::test]
    async fn filter_segment_predicate_receives_original_exchange() {
        let mut seg = segment(
            predicate(|ex: &Exchange| ex.input.body.as_text() == Some("check")),
            uppercase_body(),
        );
        let ex = Exchange::new(Message::new("check"));
        let result = completed_exchange(seg.run(ex).await);
        assert_eq!(result.input.body.as_text(), Some("CHECK"));
    }

    // 6. Cloned segment runs independently.
    #[tokio::test]
    async fn filter_segment_clone_is_independent() {
        let seg = segment(predicate(|_: &Exchange| true), passthrough());
        let mut clone = seg.clone();
        let ex = Exchange::new(Message::new("hi"));
        let result = completed_exchange(clone.run(ex).await);
        assert_eq!(result.input.body.as_text(), Some("hi"));
    }

    // ── Fallible predicate path (language-value-boundary task 1.4) ──

    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use camel_api::ExpressionErrorClass;

    fn expression_failed() -> CamelError {
        CamelError::ExpressionFailed {
            language: "rhai".to_string(),
            route_id: "r1".to_string(),
            step_id: "step#0".to_string(),
            verb: "filter".to_string(),
            class: ExpressionErrorClass::Runtime,
            position: None,
            conversion: None,
            cause: None,
        }
    }

    fn async_err_predicate(err: CamelError) -> PredicateSource {
        PredicateSource::Async(Arc::new(move |_: &Exchange| {
            let err = err.clone();
            Box::pin(async move { Err(err) }) as camel_api::BoxBoolFuture
        }))
    }

    struct RecordRun(Arc<AtomicU32>);

    impl camel_api::OutcomePipeline for RecordRun {
        fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
            Box::new(RecordRun(Arc::clone(&self.0)))
        }
        fn run<'a>(
            &'a mut self,
            exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            let c = Arc::clone(&self.0);
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                PipelineOutcome::Completed(exchange)
            })
        }
    }

    #[tokio::test]
    async fn filter_segment_propagates_predicate_error() {
        let body_calls = Arc::new(AtomicU32::new(0));
        let mut seg = FilterSegment {
            predicate: async_err_predicate(expression_failed()),
            body: camel_api::OutcomeSegment::new(Box::new(RecordRun(Arc::clone(&body_calls)))),
        };
        let outcome = seg.run(Exchange::default()).await;
        match outcome {
            PipelineOutcome::Failed(err) => {
                assert!(
                    matches!(err, CamelError::ExpressionFailed { .. }),
                    "expected ExpressionFailed, got {err:?}"
                );
            }
            other => panic!("expected PipelineOutcome::Failed, got {other:?}"),
        }
        assert_eq!(
            body_calls.load(Ordering::SeqCst),
            0,
            "filter body must not run when the predicate errors"
        );
    }
}
