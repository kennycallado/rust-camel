use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::{CamelError, Exchange};

/// A processor that removes a header from the exchange's input message.
#[derive(Clone)]
pub struct RemoveHeader<P> {
    inner: P,
    key: String,
}

impl<P> RemoveHeader<P> {
    /// Create a new RemoveHeader processor that removes the given header.
    pub fn new(inner: P, key: impl Into<String>) -> Self {
        Self {
            inner,
            key: key.into(),
        }
    }
}

/// A Tower Layer that wraps an inner service with a [`RemoveHeader`].
#[derive(Clone)]
pub struct RemoveHeaderLayer {
    key: String,
}

impl RemoveHeaderLayer {
    pub fn new(key: impl Into<String>) -> Self {
        Self { key: key.into() }
    }
}

impl<S> tower::Layer<S> for RemoveHeaderLayer {
    type Service = RemoveHeader<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RemoveHeader::new(inner, self.key.clone())
    }
}

impl<P> Service<Exchange> for RemoveHeader<P>
where
    P: Service<Exchange, Response = Exchange, Error = CamelError> + Clone + Send + 'static,
    P::Future: Send,
{
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        exchange.input.headers.remove(&self.key);
        let fut = self.inner.call(exchange);
        Box::pin(fut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{IdentityProcessor, Message, Value};
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_remove_header_deletes_existing() {
        let mut exchange = Exchange::new(Message::default());
        exchange
            .input
            .set_header("CamelHttpPath", Value::String("x".into()));

        let processor = RemoveHeader::new(IdentityProcessor, "CamelHttpPath");

        let result = processor.oneshot(exchange).await.unwrap();
        assert!(!result.input.headers.contains_key("CamelHttpPath"));
    }

    #[tokio::test]
    async fn test_remove_header_noop_on_missing() {
        let mut exchange = Exchange::new(Message::default());
        exchange.input.set_header("A", Value::from(1));
        exchange.input.set_header("B", Value::from(2));

        let processor = RemoveHeader::new(IdentityProcessor, "C");

        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.header("A"), Some(&Value::from(1)));
        assert_eq!(result.input.header("B"), Some(&Value::from(2)));
        assert!(!result.input.headers.contains_key("C"));
    }

    #[tokio::test]
    async fn test_remove_header_preserves_other_headers() {
        let mut exchange = Exchange::new(Message::default());
        exchange.input.set_header("X", Value::from(1));
        exchange.input.set_header("Y", Value::from(2));
        exchange.input.set_header("Z", Value::from(3));

        let processor = RemoveHeader::new(IdentityProcessor, "Y");

        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.header("X"), Some(&Value::from(1)));
        assert_eq!(result.input.header("Z"), Some(&Value::from(3)));
        assert!(!result.input.headers.contains_key("Y"));
    }

    #[tokio::test]
    async fn test_remove_header_preserves_body() {
        let exchange = Exchange::new(Message::new("hello"));

        let processor = RemoveHeader::new(IdentityProcessor, "anything");

        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("hello"));
    }
}
