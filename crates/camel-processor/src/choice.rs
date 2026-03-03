use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::{BoxProcessor, CamelError, Exchange, FilterPredicate};

/// A single when-clause: a predicate + the sub-pipeline to execute when it matches.
pub struct WhenClause {
    pub predicate: FilterPredicate,
    pub pipeline: BoxProcessor,
}

impl Clone for WhenClause {
    fn clone(&self) -> Self {
        Self {
            predicate: self.predicate.clone(),
            pipeline: self.pipeline.clone(),
        }
    }
}

/// Tower Service implementing the Choice EIP (Content-Based Router).
///
/// Evaluates `when` clauses in order. The first matching predicate routes the
/// exchange through its sub-pipeline. If no predicate matches, the `otherwise`
/// pipeline is used (if present); otherwise the exchange passes through unchanged.
#[derive(Clone)]
pub struct ChoiceService {
    whens: Vec<WhenClause>,
    otherwise: Option<BoxProcessor>,
}

impl ChoiceService {
    /// Create a new `ChoiceService`.
    ///
    /// `whens` — ordered list of `(predicate, sub_pipeline)` pairs.
    /// `otherwise` — optional fallback pipeline (executed when no `when` matches).
    pub fn new(whens: Vec<WhenClause>, otherwise: Option<BoxProcessor>) -> Self {
        Self { whens, otherwise }
    }
}

impl Service<Exchange> for ChoiceService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        for when in &mut self.whens {
            if (when.predicate)(&exchange) {
                let fut = when.pipeline.call(exchange);
                return Box::pin(fut);
            }
        }
        if let Some(otherwise) = &mut self.otherwise {
            let fut = otherwise.call(exchange);
            return Box::pin(fut);
        }
        Box::pin(async move { Ok(exchange) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{Body, BoxProcessorExt, Message, Value};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn append_body(suffix: &'static str) -> BoxProcessor {
        BoxProcessor::from_fn(move |mut ex: Exchange| {
            Box::pin(async move {
                if let Body::Text(s) = &ex.input.body {
                    ex.input.body = Body::Text(format!("{s}{suffix}"));
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

    fn pred_header(name: &'static str) -> FilterPredicate {
        Arc::new(move |ex: &Exchange| ex.input.header(name).is_some())
    }

    // 1. First matching when executes its pipeline.
    #[tokio::test]
    async fn test_choice_first_when_matches() {
        let whens = vec![
            WhenClause { predicate: pred_header("a"), pipeline: append_body("-A") },
            WhenClause { predicate: pred_header("b"), pipeline: append_body("-B") },
        ];
        let mut svc = ChoiceService::new(whens, None);
        let mut ex = Exchange::new(Message::new("x"));
        ex.input.set_header("a", Value::Bool(true));
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("x-A"));
    }

    // 2. Second when executes when first does not match.
    #[tokio::test]
    async fn test_choice_second_when_matches() {
        let whens = vec![
            WhenClause { predicate: pred_header("a"), pipeline: append_body("-A") },
            WhenClause { predicate: pred_header("b"), pipeline: append_body("-B") },
        ];
        let mut svc = ChoiceService::new(whens, None);
        let mut ex = Exchange::new(Message::new("x"));
        ex.input.set_header("b", Value::Bool(true));
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("x-B"));
    }

    // 3. Only the FIRST matching when fires (short-circuit — both a and b present).
    #[tokio::test]
    async fn test_choice_short_circuits_at_first_match() {
        let whens = vec![
            WhenClause { predicate: pred_header("a"), pipeline: append_body("-A") },
            WhenClause { predicate: pred_header("b"), pipeline: append_body("-B") },
        ];
        let mut svc = ChoiceService::new(whens, None);
        let mut ex = Exchange::new(Message::new("x"));
        ex.input.set_header("a", Value::Bool(true));
        ex.input.set_header("b", Value::Bool(true));
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("x-A"));
    }

    // 4. Otherwise executes when no when matches.
    #[tokio::test]
    async fn test_choice_otherwise_fires_when_no_when_matches() {
        let whens = vec![
            WhenClause { predicate: pred_header("a"), pipeline: append_body("-A") },
        ];
        let mut svc = ChoiceService::new(whens, Some(append_body("-else")));
        let ex = Exchange::new(Message::new("x"));
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("x-else"));
    }

    // 5. No match and no otherwise → exchange passes unchanged.
    #[tokio::test]
    async fn test_choice_no_match_no_otherwise_passthrough() {
        let whens = vec![
            WhenClause { predicate: pred_header("a"), pipeline: append_body("-A") },
        ];
        let mut svc = ChoiceService::new(whens, None);
        let ex = Exchange::new(Message::new("untouched"));
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("untouched"));
    }

    // 6. Errors in a matching when's pipeline propagate.
    #[tokio::test]
    async fn test_choice_error_in_when_propagates() {
        let whens = vec![
            WhenClause { predicate: pred_header("a"), pipeline: failing() },
        ];
        let mut svc = ChoiceService::new(whens, None);
        let mut ex = Exchange::new(Message::new("x"));
        ex.input.set_header("a", Value::Bool(true));
        let result = svc.ready().await.unwrap().call(ex).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("boom"));
    }

    // 7. Errors in otherwise pipeline propagate.
    #[tokio::test]
    async fn test_choice_error_in_otherwise_propagates() {
        let mut svc = ChoiceService::new(vec![], Some(failing()));
        let ex = Exchange::new(Message::new("x"));
        let result = svc.ready().await.unwrap().call(ex).await;
        assert!(result.is_err());
    }
}
