use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::{CamelError, Exchange, Value};

#[derive(Clone)]
pub struct DynamicSetProperty<P, F> {
    inner: P,
    key: String,
    expr: F,
}

impl<P, F> DynamicSetProperty<P, F>
where
    F: Fn(&Exchange) -> Value,
{
    pub fn new(inner: P, key: impl Into<String>, expr: F) -> Self {
        Self {
            inner,
            key: key.into(),
            expr,
        }
    }
}

#[derive(Clone)]
pub struct DynamicSetPropertyLayer<F> {
    key: String,
    expr: F,
}

impl<F> DynamicSetPropertyLayer<F> {
    pub fn new(key: impl Into<String>, expr: F) -> Self {
        Self {
            key: key.into(),
            expr,
        }
    }
}

impl<S, F> tower::Layer<S> for DynamicSetPropertyLayer<F>
where
    F: Clone,
{
    type Service = DynamicSetProperty<S, F>;

    fn layer(&self, inner: S) -> Self::Service {
        DynamicSetProperty {
            inner,
            key: self.key.clone(),
            expr: self.expr.clone(),
        }
    }
}

impl<P, F> Service<Exchange> for DynamicSetProperty<P, F>
where
    P: Service<Exchange, Response = Exchange, Error = CamelError> + Clone + Send + 'static,
    P::Future: Send,
    F: Fn(&Exchange) -> Value + Clone + Send + Sync + 'static,
{
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let value = (self.expr)(&exchange);
        exchange.set_property(self.key.clone(), value);
        let fut = self.inner.call(exchange);
        Box::pin(fut)
    }
}

#[cfg(test)]
mod tests {
    use camel_api::{Exchange, IdentityProcessor, Message, Value};
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn test_dynamic_set_property_from_body() {
        let exchange = Exchange::new(Message::new("world"));

        let svc = DynamicSetProperty::new(IdentityProcessor, "greeting", |ex: &Exchange| {
            Value::String(format!("hello {}", ex.input.body.as_text().unwrap_or("")))
        });

        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.property("greeting"),
            Some(&Value::String("hello world".into()))
        );
    }

    #[tokio::test]
    async fn test_dynamic_set_property_overwrites_existing() {
        let mut exchange = Exchange::new(Message::new("new"));
        exchange.set_property("key", Value::String("old".into()));

        let svc = DynamicSetProperty::new(IdentityProcessor, "key", |ex: &Exchange| {
            Value::String(ex.input.body.as_text().unwrap_or("").into())
        });

        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.property("key"),
            Some(&Value::String("new".into()))
        );
    }

    #[tokio::test]
    async fn test_dynamic_set_property_preserves_body() {
        let exchange = Exchange::new(Message::new("body content"));

        let svc = DynamicSetProperty::new(IdentityProcessor, "len", |ex: &Exchange| {
            let len = ex.input.body.as_text().map(|t| t.len() as i64).unwrap_or(0);
            Value::Number(len.into())
        });

        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("body content"));
        assert_eq!(result.property("len"), Some(&Value::Number(12.into())));
    }

    #[tokio::test]
    async fn test_dynamic_set_property_layer_composes() {
        use tower::ServiceBuilder;

        let svc = ServiceBuilder::new()
            .layer(DynamicSetPropertyLayer::new("computed", |_ex: &Exchange| {
                Value::Bool(true)
            }))
            .service(IdentityProcessor);

        let exchange = Exchange::new(Message::default());
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.property("computed"), Some(&Value::Bool(true)));
    }
}
