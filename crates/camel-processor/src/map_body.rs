use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::body::Body;
use camel_api::{CamelError, Exchange};

/// A processor that transforms the message body using a mapping function.
#[derive(Clone)]
pub struct MapBody<P, F> {
    inner: P,
    mapper: F,
}

impl<P, F> MapBody<P, F>
where
    F: Fn(Body) -> Body,
{
    /// Create a new MapBody processor wrapping the inner processor.
    pub fn new(inner: P, mapper: F) -> Self {
        Self { inner, mapper }
    }
}

/// A Tower Layer that wraps an inner service with a [`MapBody`].
#[derive(Clone)]
pub struct MapBodyLayer<F> {
    mapper: F,
}

impl<F> MapBodyLayer<F> {
    pub fn new(mapper: F) -> Self {
        Self { mapper }
    }
}

impl<S, F> tower::Layer<S> for MapBodyLayer<F>
where
    F: Clone,
{
    type Service = MapBody<S, F>;

    fn layer(&self, inner: S) -> Self::Service {
        MapBody {
            inner,
            mapper: self.mapper.clone(),
        }
    }
}

impl<P, F> Service<Exchange> for MapBody<P, F>
where
    P: Service<Exchange, Response = Exchange, Error = CamelError> + Clone + Send + 'static,
    P::Future: Send,
    F: Fn(Body) -> Body + Clone + Send + Sync + 'static,
{
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        exchange.input.body = (self.mapper)(exchange.input.body);
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
    async fn test_map_body_transforms_text() {
        let exchange = Exchange::new(Message::new("hello"));

        let mapper = MapBody::new(IdentityProcessor, |body: Body| {
            if let Some(text) = body.as_text() {
                Body::Text(text.to_uppercase())
            } else {
                body
            }
        });

        let result = mapper.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("HELLO"));
    }

    #[tokio::test]
    async fn test_map_body_text_to_json() {
        let exchange = Exchange::new(Message::new("value"));

        let mapper = MapBody::new(IdentityProcessor, |body: Body| {
            if let Some(text) = body.as_text() {
                Body::Json(serde_json::json!({"data": text}))
            } else {
                body
            }
        });

        let result = mapper.oneshot(exchange).await.unwrap();
        assert!(matches!(result.input.body, Body::Json(_)));
    }

    #[tokio::test]
    async fn test_map_body_empty_passthrough() {
        let exchange = Exchange::new(Message::default());

        let mapper = MapBody::new(IdentityProcessor, |body: Body| body);

        let result = mapper.oneshot(exchange).await.unwrap();
        assert!(result.input.body.is_empty());
    }

    #[tokio::test]
    async fn test_map_body_layer_composes() {
        use tower::ServiceBuilder;

        let svc = ServiceBuilder::new()
            .layer(super::MapBodyLayer::new(|body: Body| {
                if let Some(text) = body.as_text() {
                    Body::Text(text.to_uppercase())
                } else {
                    body
                }
            }))
            .service(IdentityProcessor);

        let exchange = Exchange::new(Message::new("hello"));
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("HELLO"));
    }
}
