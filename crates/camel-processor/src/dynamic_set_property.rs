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
