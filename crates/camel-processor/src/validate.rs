use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use camel_api::{CamelError, Exchange, FilterPredicate};
use tower::Service;

/// Tower Service implementing the Validate EIP.
///
/// If the predicate returns `true`, the exchange continues (returned as `Ok`).
/// If `false`, a `CamelError::ValidationError` is returned.
#[derive(Clone)]
pub struct ValidateService {
    predicate: FilterPredicate,
    expression_source: String,
}

impl ValidateService {
    /// Create from a closure predicate and an expression source string (for error messages).
    pub fn new(
        predicate: impl Fn(&Exchange) -> bool + Send + Sync + 'static,
        expression_source: impl Into<String>,
    ) -> Self {
        Self {
            predicate: FilterPredicate::new(predicate),
            expression_source: expression_source.into(),
        }
    }

    /// Create from a pre-boxed `FilterPredicate` (used by `resolve_steps`).
    pub fn from_predicate(
        predicate: FilterPredicate,
        expression_source: impl Into<String>,
    ) -> Self {
        Self {
            predicate,
            expression_source: expression_source.into(),
        }
    }
}

impl Service<Exchange> for ValidateService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        if (self.predicate)(&exchange) {
            Box::pin(async move { Ok(exchange) })
        } else {
            let source = self.expression_source.clone();
            Box::pin(async move {
                Err(CamelError::ValidationError(format!(
                    "validate('{source}'): predicate returned false",
                )))
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::Message;

    // ── ValidateService tests ──

    // 1. Passing predicate returns Ok(exchange)
    #[tokio::test]
    async fn test_validate_passing_predicate_returns_ok() {
        let mut svc = ValidateService::new(|_ex: &Exchange| true, "true predicate");
        let ex = Exchange::new(Message::new("hello"));
        let result = svc.call(ex).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().input.body.as_text(), Some("hello"));
    }

    // 2. Failing predicate returns Err(ValidationError)
    #[tokio::test]
    async fn test_validate_failing_predicate_returns_err() {
        let mut svc = ValidateService::new(|_ex: &Exchange| false, "false predicate");
        let ex = Exchange::new(Message::new("hello"));
        let result = svc.call(ex).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            CamelError::ValidationError(msg) => {
                assert!(
                    msg.contains("false predicate"),
                    "error message should contain expression source, got: {msg}"
                );
            }
            other => panic!("expected ValidationError, got: {other:?}"),
        }
    }

    // 3. Predicate evaluates the exchange (body-based validation)
    #[tokio::test]
    async fn test_validate_predicate_evaluates_body() {
        let mut svc = ValidateService::new(
            |ex: &Exchange| ex.input.body.as_text().is_some_and(|s| s.len() > 3),
            "body length > 3",
        );
        let short = Exchange::new(Message::new("ab"));
        let long = Exchange::new(Message::new("abcdef"));

        assert!(svc.call(short).await.is_err());
        assert!(svc.call(long).await.is_ok());
    }

    // 4. ValidateService is Clone
    #[tokio::test]
    async fn test_validate_clone_is_independent() {
        let svc = ValidateService::new(|_ex: &Exchange| true, "true predicate");
        let mut cloned = svc.clone();
        let ex = Exchange::new(Message::new("hi"));
        let result = cloned.call(ex).await;
        assert!(result.is_ok());
    }

    // 5. poll_ready is always Ready(Ok(()))
    #[tokio::test]
    async fn test_validate_poll_ready() {
        let mut svc = ValidateService::new(|_ex: &Exchange| true, "true predicate");
        let poll = svc.poll_ready(&mut Context::from_waker(futures::task::noop_waker_ref()));
        assert!(poll.is_ready());
        // unwrap the Poll<Result<...>>
        match poll {
            std::task::Poll::Ready(Ok(())) => {}
            other => panic!("expected Ready(Ok(())), got: {other:?}"),
        }
    }
}
