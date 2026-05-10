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
