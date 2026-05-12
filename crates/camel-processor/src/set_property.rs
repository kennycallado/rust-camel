use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::{CamelError, Exchange, Value};

#[derive(Clone)]
pub struct SetProperty<P> {
    inner: P,
    key: String,
    value: Value,
}

impl<P> SetProperty<P> {
    pub fn new(inner: P, key: impl Into<String>, value: impl Into<Value>) -> Self {
        Self {
            inner,
            key: key.into(),
            value: value.into(),
        }
    }
}

#[derive(Clone)]
pub struct SetPropertyLayer {
    key: String,
    value: Value,
}

impl SetPropertyLayer {
    pub fn new(key: impl Into<String>, value: impl Into<Value>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

impl<S> tower::Layer<S> for SetPropertyLayer {
    type Service = SetProperty<S>;

    fn layer(&self, inner: S) -> Self::Service {
        SetProperty::new(inner, self.key.clone(), self.value.clone())
    }
}

impl<P> Service<Exchange> for SetProperty<P>
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
        exchange.set_property(self.key.clone(), self.value.clone());
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
    async fn test_set_property_adds_property() {
        let exchange = Exchange::new(Message::default());

        let processor =
            SetProperty::new(IdentityProcessor, "source", Value::String("timer".into()));

        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.property("source"),
            Some(&Value::String("timer".into()))
        );
    }

    #[tokio::test]
    async fn test_set_property_overwrites_existing() {
        let mut exchange = Exchange::new(Message::default());
        exchange.set_property("key", Value::String("old".into()));

        let processor = SetProperty::new(IdentityProcessor, "key", Value::String("new".into()));

        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(result.property("key"), Some(&Value::String("new".into())));
    }

    #[tokio::test]
    async fn test_set_property_preserves_body() {
        let exchange = Exchange::new(Message::new("body content"));

        let processor = SetProperty::new(IdentityProcessor, "prop", Value::Bool(true));

        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("body content"));
        assert_eq!(result.property("prop"), Some(&Value::Bool(true)));
    }

    #[tokio::test]
    async fn test_set_property_layer_composes() {
        use tower::ServiceBuilder;

        let svc = ServiceBuilder::new()
            .layer(super::SetPropertyLayer::new(
                "env",
                Value::String("test".into()),
            ))
            .service(IdentityProcessor);

        let exchange = Exchange::new(Message::default());
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.property("env"), Some(&Value::String("test".into())));
    }
}
