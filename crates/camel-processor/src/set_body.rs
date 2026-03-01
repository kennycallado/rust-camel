use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::body::Body;
use camel_api::{CamelError, Exchange};

/// A processor that sets the message body using an expression closure.
/// The closure receives the full exchange (read-only) and returns a new Body.
#[derive(Clone)]
pub struct SetBody<P, F> {
    inner: P,
    expr: F,
}

impl<P, F> SetBody<P, F>
where
    F: Fn(&Exchange) -> Body,
{
    pub fn new(inner: P, expr: F) -> Self {
        Self { inner, expr }
    }
}

/// A Tower Layer that wraps an inner service with a [`SetBody`].
#[derive(Clone)]
pub struct SetBodyLayer<F> {
    expr: F,
}

impl<F> SetBodyLayer<F> {
    pub fn new(expr: F) -> Self {
        Self { expr }
    }
}

impl<S, F> tower::Layer<S> for SetBodyLayer<F>
where
    F: Clone,
{
    type Service = SetBody<S, F>;

    fn layer(&self, inner: S) -> Self::Service {
        SetBody {
            inner,
            expr: self.expr.clone(),
        }
    }
}

impl<P, F> Service<Exchange> for SetBody<P, F>
where
    P: Service<Exchange, Response = Exchange, Error = CamelError> + Clone + Send + 'static,
    P::Future: Send,
    F: Fn(&Exchange) -> Body + Clone + Send + Sync + 'static,
{
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        exchange.input.body = (self.expr)(&exchange);
        let fut = self.inner.call(exchange);
        Box::pin(fut)
    }
}

#[cfg(test)]
mod tests {
    use camel_api::{Exchange, IdentityProcessor, Message};
    use camel_api::body::Body;
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn test_set_body_static_replaces_body() {
        let exchange = Exchange::new(Message::new("original"));
        let svc = SetBody::new(IdentityProcessor, |_ex: &Exchange| Body::Text("replaced".into()));
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("replaced"));
    }

    #[tokio::test]
    async fn test_set_body_dynamic_reads_exchange() {
        let mut msg = Message::new("hello");
        msg.set_header("suffix", camel_api::Value::String("!".into()));
        let exchange = Exchange::new(msg);

        let svc = SetBody::new(IdentityProcessor, |ex: &Exchange| {
            let base = ex.input.body.as_text().unwrap_or("");
            let suffix = ex.input.header("suffix")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Body::Text(format!("{}{}", base, suffix))
        });

        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("hello!"));
    }

    #[tokio::test]
    async fn test_set_body_preserves_headers() {
        let mut msg = Message::default();
        msg.set_header("keep", camel_api::Value::Bool(true));
        let exchange = Exchange::new(msg);

        let svc = SetBody::new(IdentityProcessor, |_ex: &Exchange| Body::Text("new".into()));
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.header("keep"), Some(&camel_api::Value::Bool(true)));
        assert_eq!(result.input.body.as_text(), Some("new"));
    }

    #[tokio::test]
    async fn test_set_body_layer_composes() {
        use tower::ServiceBuilder;

        let svc = ServiceBuilder::new()
            .layer(SetBodyLayer::new(|_ex: &Exchange| Body::Text("layered".into())))
            .service(IdentityProcessor);

        let exchange = Exchange::new(Message::default());
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("layered"));
    }
}
