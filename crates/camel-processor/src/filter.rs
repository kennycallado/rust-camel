use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;
use tower::ServiceExt;

use camel_api::{BoxProcessor, CamelError, Exchange, FilterPredicate, PredicateSource};

/// Tower Service implementing the Filter EIP.
///
/// If the predicate evaluates to `true`, the exchange is forwarded through the
/// sub-pipeline. If `false`, the exchange is returned as-is and the
/// sub-pipeline is skipped entirely. A failed (async) predicate evaluation
/// propagates as `Err` — the exchange is never silently dropped.
#[derive(Clone)]
pub struct FilterService {
    predicate: PredicateSource,
    sub_pipeline: BoxProcessor,
}

impl FilterService {
    /// Create from a closure predicate and a resolved sub-pipeline.
    pub fn new(
        predicate: impl Fn(&Exchange) -> bool + Send + Sync + 'static,
        sub_pipeline: BoxProcessor,
    ) -> Self {
        Self {
            predicate: PredicateSource::Sync(FilterPredicate::new(predicate)),
            sub_pipeline,
        }
    }

    /// Create from a fallible `PredicateSource` (used by `resolve_steps`).
    pub fn from_predicate(predicate: PredicateSource, sub_pipeline: BoxProcessor) -> Self {
        Self {
            predicate,
            sub_pipeline,
        }
    }
}

impl Service<Exchange> for FilterService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.sub_pipeline.poll_ready(cx)
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        // Clone-and-replace: the future owns its state so the predicate can be
        // awaited before deciding whether to invoke the sub-pipeline.
        let predicate = self.predicate.clone();
        let mut sub_pipeline = self.sub_pipeline.clone();
        Box::pin(async move {
            match predicate.matches(&exchange).await {
                Ok(true) => sub_pipeline.ready().await?.call(exchange).await,
                Ok(false) => Ok(exchange),
                Err(err) => Err(err),
            }
        })
    }
}

// ── FilterSegment (ADR-0025 OutcomePipeline) ─────────────────────────────

/// Outcome-aware structural EIP segment for the Filter pattern.
///
/// When the predicate passes, delegates to `body` (which can return
/// `Completed`, `Stopped`, or `Failed`). When the predicate evaluates to
/// `false`, returns `Completed(original_exchange)` — the exchange is returned
/// as-is and the body is skipped entirely. A failed predicate evaluation
/// yields `Failed(err)`; the exchange is never silently dropped.
///
/// Unlike `FilterService` (which operates at the Tower layer and cannot
/// preserve `Stopped(ex)` with mutations), `FilterSegment` operates at
/// the `PipelineOutcome` layer and preserves the exchange at the Stop
/// point including all mutations.
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
    use camel_api::{Body, BoxProcessorExt, Message, Value};
    use tower::ServiceExt;

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

    // 1. Matching exchange is forwarded to sub_pipeline.
    #[tokio::test]
    async fn test_filter_passes_matching_exchange() {
        let mut svc = FilterService::new(
            |ex: &Exchange| ex.input.header("active").is_some(),
            uppercase_body(),
        );
        let mut ex = Exchange::new(Message::new("hello"));
        ex.input.set_header("active", Value::Bool(true));
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("HELLO"));
    }

    // 2. Non-matching exchange is returned as-is, sub_pipeline not called.
    #[tokio::test]
    async fn test_filter_blocks_non_matching_exchange() {
        let mut svc = FilterService::new(
            |ex: &Exchange| ex.input.header("active").is_some(),
            uppercase_body(),
        );
        let ex = Exchange::new(Message::new("hello"));
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        // body unchanged — uppercase_body was NOT called
        assert_eq!(result.input.body.as_text(), Some("hello"));
    }

    // 3. Result is the sub_pipeline's output, not the original exchange.
    #[tokio::test]
    async fn test_filter_sub_pipeline_transforms_body() {
        let mut svc = FilterService::new(|_: &Exchange| true, uppercase_body());
        let ex = Exchange::new(Message::new("world"));
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("WORLD"));
    }

    // 4. Sub-pipeline errors propagate.
    #[tokio::test]
    async fn test_filter_sub_pipeline_error_propagates() {
        let mut svc = FilterService::new(|_: &Exchange| true, failing());
        let ex = Exchange::new(Message::new("x"));
        let result = svc.ready().await.unwrap().call(ex).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("boom"));
    }

    // 5. Predicate receives the original exchange before sub_pipeline mutates it.
    #[tokio::test]
    async fn test_filter_predicate_receives_original_exchange() {
        let mut svc = FilterService::new(
            |ex: &Exchange| ex.input.body.as_text() == Some("check"),
            uppercase_body(),
        );
        let ex = Exchange::new(Message::new("check"));
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("CHECK"));
    }

    // 6. Cloned FilterService shares no mutable state (BoxProcessor clone is independent).
    #[tokio::test]
    async fn test_filter_clone_is_independent() {
        let svc = FilterService::new(|_: &Exchange| true, passthrough());
        let mut clone = svc.clone();
        let ex = Exchange::new(Message::new("hi"));
        let result = clone.ready().await.unwrap().call(ex).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("hi"));
    }

    // ── Fallible predicate path (language-value-boundary task 1.4) ──

    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use camel_api::outcome_pipeline::OutcomePipeline as _;
    use camel_api::{ExpressionErrorClass, PipelineOutcome, PredicateSource};

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
