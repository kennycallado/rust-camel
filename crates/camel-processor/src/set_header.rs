use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::{CamelError, Exchange, Value};

/// A processor that sets a header on the exchange's input message.
#[derive(Clone)]
pub struct SetHeader<P> {
    inner: P,
    key: String,
    value: Value,
}

impl<P> SetHeader<P> {
    /// Create a new SetHeader processor that adds the given header.
    pub fn new(inner: P, key: impl Into<String>, value: impl Into<Value>) -> Self {
        Self {
            inner,
            key: key.into(),
            value: value.into(),
        }
    }
}

/// A Tower Layer that wraps an inner service with a [`SetHeader`].
#[derive(Clone)]
pub struct SetHeaderLayer {
    key: String,
    value: Value,
}

impl SetHeaderLayer {
    pub fn new(key: impl Into<String>, value: impl Into<Value>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

impl<S> tower::Layer<S> for SetHeaderLayer {
    type Service = SetHeader<S>;

    fn layer(&self, inner: S) -> Self::Service {
        SetHeader::new(inner, self.key.clone(), self.value.clone())
    }
}

impl<P> Service<Exchange> for SetHeader<P>
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
        exchange
            .input
            .headers
            .insert(self.key.clone(), self.value.clone());
        let fut = self.inner.call(exchange);
        Box::pin(fut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{IdentityProcessor, Message};
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_set_header_adds_header() {
        let exchange = Exchange::new(Message::default());

        let processor = SetHeader::new(IdentityProcessor, "source", Value::String("timer".into()));

        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.input.header("source"),
            Some(&Value::String("timer".into()))
        );
    }

    #[tokio::test]
    async fn test_set_header_overwrites_existing() {
        let mut exchange = Exchange::new(Message::default());
        exchange
            .input
            .set_header("key", Value::String("old".into()));

        let processor = SetHeader::new(IdentityProcessor, "key", Value::String("new".into()));

        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.input.header("key"),
            Some(&Value::String("new".into()))
        );
    }

    #[tokio::test]
    async fn test_set_header_preserves_body() {
        let exchange = Exchange::new(Message::new("body content"));

        let processor = SetHeader::new(IdentityProcessor, "header", Value::Bool(true));

        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("body content"));
        assert_eq!(result.input.header("header"), Some(&Value::Bool(true)));
    }

    #[tokio::test]
    async fn test_set_header_layer_composes() {
        use tower::ServiceBuilder;

        let svc = ServiceBuilder::new()
            .layer(super::SetHeaderLayer::new(
                "env",
                Value::String("test".into()),
            ))
            .service(IdentityProcessor);

        let exchange = Exchange::new(Message::default());
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.input.header("env"),
            Some(&Value::String("test".into()))
        );
    }
}
