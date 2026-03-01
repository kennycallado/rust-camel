use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::{BoxProcessor, CamelError, Exchange, FilterPredicate};

/// Tower Service implementing the Filter EIP.
///
/// If the predicate returns `true`, the exchange is forwarded through the sub-pipeline.
/// If `false`, the exchange is returned as-is and the sub-pipeline is skipped entirely.
#[derive(Clone)]
pub struct FilterService {
    predicate: FilterPredicate,
    sub_pipeline: BoxProcessor,
}

impl FilterService {
    /// Create from a closure predicate and a resolved sub-pipeline.
    pub fn new(
        predicate: impl Fn(&Exchange) -> bool + Send + Sync + 'static,
        sub_pipeline: BoxProcessor,
    ) -> Self {
        Self {
            predicate: Arc::new(predicate),
            sub_pipeline,
        }
    }

    /// Create from a pre-boxed `FilterPredicate` (used by `resolve_steps`).
    pub fn from_predicate(predicate: FilterPredicate, sub_pipeline: BoxProcessor) -> Self {
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
        if (self.predicate)(&exchange) {
            let fut = self.sub_pipeline.call(exchange);
            Box::pin(fut)
        } else {
            Box::pin(async move { Ok(exchange) })
        }
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
}
