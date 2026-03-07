use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::body_converter::{BodyType, convert};
use camel_api::{CamelError, Exchange};

/// A processor that converts the message body to a target type.
///
/// Supported conversions: Text ↔ Json ↔ Bytes.
/// `Body::Stream` always returns `TypeConversionFailed` — materialize first.
/// Same-type conversions are noops.
#[derive(Clone)]
pub struct ConvertBodyTo<P> {
    inner: P,
    target: BodyType,
}

impl<P> ConvertBodyTo<P> {
    pub fn new(inner: P, target: BodyType) -> Self {
        Self { inner, target }
    }
}

impl<P> Service<Exchange> for ConvertBodyTo<P>
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
        let target = self.target;
        let body = std::mem::replace(&mut exchange.input.body, camel_api::body::Body::Empty);
        match convert(body, target) {
            Ok(new_body) => {
                exchange.input.body = new_body;
                let fut = self.inner.call(exchange);
                Box::pin(fut)
            }
            Err(e) => Box::pin(async move { Err(e) }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use camel_api::body::Body;
    use camel_api::{IdentityProcessor, Message};
    use serde_json::json;
    use tower::ServiceExt;

    #[tokio::test]
    async fn text_to_json_in_pipeline() {
        let exchange = Exchange::new(Message::new(Body::Text(r#"{"n":1}"#.to_string())));
        let svc = ConvertBodyTo::new(IdentityProcessor, BodyType::Json);
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body, Body::Json(json!({"n": 1})));
    }

    #[tokio::test]
    async fn json_to_text_in_pipeline() {
        let exchange = Exchange::new(Message::new(Body::Json(json!({"x": 2}))));
        let svc = ConvertBodyTo::new(IdentityProcessor, BodyType::Text);
        let result = svc.oneshot(exchange).await.unwrap();
        assert!(matches!(result.input.body, Body::Text(_)));
    }

    #[tokio::test]
    async fn bytes_to_text_in_pipeline() {
        let exchange = Exchange::new(Message::new(Body::Bytes(Bytes::from_static(b"hello"))));
        let svc = ConvertBodyTo::new(IdentityProcessor, BodyType::Text);
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body, Body::Text("hello".to_string()));
    }

    #[tokio::test]
    async fn invalid_conversion_returns_err() {
        let exchange = Exchange::new(Message::new(Body::Empty));
        let svc = ConvertBodyTo::new(IdentityProcessor, BodyType::Text);
        let result = svc.oneshot(exchange).await;
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[tokio::test]
    async fn noop_same_type() {
        let exchange = Exchange::new(Message::new(Body::Text("hi".to_string())));
        let svc = ConvertBodyTo::new(IdentityProcessor, BodyType::Text);
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body, Body::Text("hi".to_string()));
    }
}
