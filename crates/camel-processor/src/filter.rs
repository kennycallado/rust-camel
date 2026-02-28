use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::{CamelError, Exchange};

/// A processor that conditionally passes exchanges through based on a predicate.
///
/// If the predicate returns `true`, the exchange is forwarded to the inner processor.
/// If `false`, the exchange is returned as-is (skipping downstream processing).
#[derive(Clone)]
pub struct Filter<P, F> {
    inner: P,
    predicate: F,
}

impl<P, F> Filter<P, F>
where
    F: Fn(&Exchange) -> bool,
{
    /// Create a new Filter wrapping the inner processor with the given predicate.
    pub fn new(inner: P, predicate: F) -> Self {
        Self { inner, predicate }
    }
}

/// A Tower Layer that wraps an inner service with a [`Filter`].
#[derive(Clone)]
pub struct FilterLayer<F> {
    predicate: F,
}

impl<F> FilterLayer<F> {
    /// Create a new FilterLayer with the given predicate.
    pub fn new(predicate: F) -> Self {
        Self { predicate }
    }
}

impl<S, F> tower::Layer<S> for FilterLayer<F>
where
    F: Clone,
{
    type Service = Filter<S, F>;

    fn layer(&self, inner: S) -> Self::Service {
        Filter {
            inner,
            predicate: self.predicate.clone(),
        }
    }
}

impl<P, F> Service<Exchange> for Filter<P, F>
where
    P: Service<Exchange, Response = Exchange, Error = CamelError> + Clone + Send + 'static,
    P::Future: Send,
    F: Fn(&Exchange) -> bool + Clone + Send + Sync + 'static,
{
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        if (self.predicate)(&exchange) {
            let fut = self.inner.call(exchange);
            Box::pin(fut)
        } else {
            // Exchange did not match predicate; return as-is
            Box::pin(async move { Ok(exchange) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{IdentityProcessor, Message, Value};
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_filter_passes_matching_exchange() {
        let mut exchange = Exchange::new(Message::default());
        exchange.input.set_header("active", Value::Bool(true));

        let filter = Filter::new(IdentityProcessor, |ex: &Exchange| {
            ex.input.header("active").is_some()
        });

        let result = filter.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.header("active"), Some(&Value::Bool(true)));
    }

    #[tokio::test]
    async fn test_filter_blocks_non_matching_exchange() {
        let exchange = Exchange::new(Message::new("no active header"));

        let filter = Filter::new(IdentityProcessor, |ex: &Exchange| {
            ex.input.header("active").is_some()
        });

        let result = filter.oneshot(exchange).await.unwrap();
        // Exchange returned as-is (not processed by inner)
        assert_eq!(result.input.body.as_text(), Some("no active header"));
    }

    #[tokio::test]
    async fn test_filter_with_body_predicate() {
        let exchange = Exchange::new(Message::new("important"));

        let filter = Filter::new(IdentityProcessor, |ex: &Exchange| {
            ex.input.body.as_text() == Some("important")
        });

        let result = filter.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("important"));
    }

    #[tokio::test]
    async fn test_filter_layer_composes() {
        use tower::ServiceBuilder;

        let svc = ServiceBuilder::new()
            .layer(super::FilterLayer::new(|ex: &Exchange| {
                ex.input.header("active").is_some()
            }))
            .service(IdentityProcessor);

        let mut exchange = Exchange::new(Message::default());
        exchange.input.set_header("active", Value::Bool(true));

        let result = svc.oneshot(exchange).await.unwrap();
        assert!(result.input.header("active").is_some());
    }

    #[tokio::test]
    async fn test_filter_layer_blocks() {
        use tower::ServiceBuilder;

        let svc = ServiceBuilder::new()
            .layer(super::FilterLayer::new(|ex: &Exchange| {
                ex.input.header("active").is_some()
            }))
            .service(IdentityProcessor);

        let exchange = Exchange::new(Message::new("no header"));
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("no header"));
    }
}
