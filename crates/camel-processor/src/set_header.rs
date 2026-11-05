use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::{CamelError, Exchange, PROPERTY_HTTP_CONTENT_TYPE_INHERITED, Value};

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
            .set_header(self.key.clone(), self.value.clone());
        if self.key.eq_ignore_ascii_case("content-type") {
            exchange.remove_property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED);
        }
        let fut = self.inner.call(exchange);
        Box::pin(fut)
    }
}

/// A processor that sets a header on the exchange's input message,
/// but ONLY if the header is not already present (if-absent semantics).
#[derive(Clone)]
pub struct SetHeaderIfAbsent<P> {
    inner: P,
    key: String,
    value: Value,
}

impl<P> SetHeaderIfAbsent<P> {
    /// Create a new SetHeaderIfAbsent processor that adds the given header
    /// only when it is not already present.
    pub fn new(inner: P, key: impl Into<String>, value: impl Into<Value>) -> Self {
        Self {
            inner,
            key: key.into(),
            value: value.into(),
        }
    }
}

impl<P> Service<Exchange> for SetHeaderIfAbsent<P>
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
        if exchange.input.header_ic(&self.key).is_none() {
            exchange
                .input
                .set_header(self.key.clone(), self.value.clone());
            if self.key.eq_ignore_ascii_case("content-type") {
                exchange.remove_property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED);
            }
        }
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
    async fn set_header_replaces_different_case() {
        let mut exchange = Exchange::new(Message::default());
        exchange
            .input
            .set_header("X-Flag", Value::String("old".into()));

        let processor = SetHeader::new(IdentityProcessor, "x-flag", Value::String("new".into()));

        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.input.header("x-flag"),
            Some(&Value::String("new".into()))
        );
        assert_eq!(result.input.header("X-Flag"), None);
        assert_eq!(result.input.headers.len(), 1);
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

    // ── SetHeaderIfAbsent ──

    #[tokio::test]
    async fn test_set_header_if_absent_adds_when_missing() {
        let exchange = Exchange::new(Message::default());

        let processor =
            SetHeaderIfAbsent::new(IdentityProcessor, "source", Value::String("timer".into()));

        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.input.header("source"),
            Some(&Value::String("timer".into()))
        );
    }

    #[tokio::test]
    async fn test_set_header_if_absent_preserves_existing() {
        let mut exchange = Exchange::new(Message::default());
        exchange
            .input
            .set_header("key", Value::String("old".into()));

        let processor =
            SetHeaderIfAbsent::new(IdentityProcessor, "key", Value::String("new".into()));

        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.input.header("key"),
            Some(&Value::String("old".into()))
        );
    }

    #[tokio::test]
    async fn set_header_if_absent_respects_different_case() {
        let mut exchange = Exchange::new(Message::default());
        exchange
            .input
            .set_header("X-Flag", Value::String("old".into()));

        let processor =
            SetHeaderIfAbsent::new(IdentityProcessor, "x-flag", Value::String("new".into()));

        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.input.header_ic("x-flag"),
            Some(&Value::String("old".into()))
        );
        assert_eq!(result.input.header("x-flag"), None);
        assert_eq!(result.input.headers.len(), 1);
    }

    #[tokio::test]
    async fn set_header_if_absent_sees_raw_map_different_case() {
        let mut exchange = Exchange::new(Message::default());
        exchange
            .input
            .headers
            .insert("X-Flag".into(), Value::String("old".into()));

        let processor =
            SetHeaderIfAbsent::new(IdentityProcessor, "x-flag", Value::String("new".into()));

        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.input.header_ic("x-flag"),
            Some(&Value::String("old".into()))
        );
        assert_eq!(result.input.header("x-flag"), None);
        assert_eq!(result.input.headers.len(), 1);
    }

    #[tokio::test]
    async fn test_set_header_if_absent_preserves_body() {
        let exchange = Exchange::new(Message::new("body content"));

        let processor = SetHeaderIfAbsent::new(IdentityProcessor, "header", Value::Bool(true));

        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("body content"));
        assert_eq!(result.input.header("header"), Some(&Value::Bool(true)));
    }

    // ── Content-Type provenance marker ──

    #[tokio::test]
    async fn set_header_clears_marker_on_content_type() {
        let mut exchange = Exchange::new(Message::default());
        exchange.set_property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED, Value::Bool(true));

        let processor = SetHeader::new(IdentityProcessor, "Content-Type", "application/json");

        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED),
            None,
            "writing Content-Type must clear the inherited-content-type marker"
        );
        assert_eq!(
            result.input.header("Content-Type"),
            Some(&Value::String("application/json".into()))
        );
    }

    #[tokio::test]
    async fn set_header_keeps_marker_on_other_header() {
        let mut exchange = Exchange::new(Message::default());
        exchange.set_property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED, Value::Bool(true));

        let processor = SetHeader::new(IdentityProcessor, "X-Flag", "on");

        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED),
            Some(&Value::Bool(true)),
            "writing an unrelated header must leave the marker untouched"
        );
    }

    #[tokio::test]
    async fn set_header_if_absent_clears_marker_only_when_written() {
        // (a) Header absent: the write happens, so the marker is cleared.
        let mut exchange = Exchange::new(Message::default());
        exchange.set_property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED, Value::Bool(true));

        let processor =
            SetHeaderIfAbsent::new(IdentityProcessor, "Content-Type", "application/json");

        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED),
            None,
            "an if-absent write of Content-Type must clear the marker"
        );
        assert_eq!(
            result.input.header("Content-Type"),
            Some(&Value::String("application/json".into()))
        );

        // (b) Header already present: no write, so the marker survives.
        let mut exchange = Exchange::new(Message::default());
        exchange.set_property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED, Value::Bool(true));
        exchange
            .input
            .set_header("Content-Type", Value::String("text/plain".into()));

        let processor =
            SetHeaderIfAbsent::new(IdentityProcessor, "Content-Type", "application/json");

        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED),
            Some(&Value::Bool(true)),
            "an if-absent no-op must not clear the marker"
        );
        assert_eq!(
            result.input.header("Content-Type"),
            Some(&Value::String("text/plain".into()))
        );
    }
}
