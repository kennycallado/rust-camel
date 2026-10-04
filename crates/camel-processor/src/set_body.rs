use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::body::Body;
use camel_api::{CamelError, Exchange, Value, ValueSource};

/// Map an evaluated expression value onto a message body.
///
/// `Null` maps to [`Body::Empty`], strings to [`Body::Text`], everything
/// else to [`Body::Json`] (same rules as the camel-core `value_to_body`).
fn value_to_body(value: Value) -> Body {
    match value {
        Value::Null => Body::Empty,
        Value::String(s) => Body::Text(s),
        other => Body::Json(other),
    }
}

/// A processor that sets the message body from a fallible [`ValueSource`].
///
/// A failed evaluation fails the step WITHOUT replacing the body and WITHOUT
/// invoking the inner service.
#[derive(Clone)]
pub struct SetBody<P> {
    inner: P,
    source: ValueSource,
}

impl<P> SetBody<P> {
    pub fn new(inner: P, source: impl Into<ValueSource>) -> Self {
        Self {
            inner,
            source: source.into(),
        }
    }
}

/// A Tower Layer that wraps an inner service with a [`SetBody`].
#[derive(Clone)]
pub struct SetBodyLayer {
    source: ValueSource,
}

impl SetBodyLayer {
    pub fn new(source: impl Into<ValueSource>) -> Self {
        Self {
            source: source.into(),
        }
    }
}

impl<S> tower::Layer<S> for SetBodyLayer {
    type Service = SetBody<S>;

    fn layer(&self, inner: S) -> Self::Service {
        SetBody {
            inner,
            source: self.source.clone(),
        }
    }
}

impl<P> Service<Exchange> for SetBody<P>
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
        let source = self.source.clone();
        // Clone-and-replace: move the polled inner into the future so the
        // async body owns its service; a fresh clone stays behind.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        Box::pin(async move {
            let value = source.evaluate(&exchange).await?;
            exchange.input.body = value_to_body(value);
            inner.call(exchange).await
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use camel_api::{
        BoxValueFuture, CamelError, Exchange, IdentityProcessor, Message, Value, ValueSource,
    };
    use tower::ServiceExt;

    use super::*;

    fn sync_source<F>(f: F) -> ValueSource
    where
        F: Fn(&Exchange) -> Value + Send + Sync + 'static,
    {
        ValueSource::Sync(Arc::new(f))
    }

    fn failing_source() -> ValueSource {
        ValueSource::Async(Arc::new(|_: &Exchange| {
            Box::pin(async { Err(CamelError::ProcessorError("expr boom".into())) })
                as BoxValueFuture
        }))
    }

    /// Inner service that records how many times it was invoked.
    #[derive(Clone)]
    struct CountingInner {
        called: Arc<AtomicUsize>,
    }

    impl Service<Exchange> for CountingInner {
        type Response = Exchange;
        type Error = CamelError;
        type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, exchange: Exchange) -> Self::Future {
            self.called.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(exchange) })
        }
    }

    #[tokio::test]
    async fn set_body_dynamic_error_keeps_body() {
        let called = Arc::new(AtomicUsize::new(0));
        let svc = SetBody::new(
            CountingInner {
                called: Arc::clone(&called),
            },
            failing_source(),
        );

        // A failed body expression fails the step: the body mutation is never
        // applied and the inner service is never invoked (so the original
        // body is preserved — nothing downstream observes a mutation).
        let result = svc.oneshot(Exchange::new(Message::new("original"))).await;

        assert!(result.is_err(), "failed evaluation must fail the step");
        assert_eq!(
            called.load(Ordering::SeqCst),
            0,
            "inner must NOT run when evaluation fails"
        );
    }

    #[tokio::test]
    async fn test_set_body_static_replaces_body() {
        let exchange = Exchange::new(Message::new("original"));
        let svc = SetBody::new(
            IdentityProcessor,
            sync_source(|_ex: &Exchange| Value::String("replaced".into())),
        );
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("replaced"));
    }

    #[tokio::test]
    async fn test_set_body_dynamic_reads_exchange() {
        let mut msg = Message::new("hello");
        msg.set_header("suffix", camel_api::Value::String("!".into()));
        let exchange = Exchange::new(msg);

        let svc = SetBody::new(
            IdentityProcessor,
            sync_source(|ex: &Exchange| {
                let base = ex.input.body.as_text().unwrap_or("");
                let suffix = ex
                    .input
                    .header("suffix")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                Value::String(format!("{}{}", base, suffix))
            }),
        );

        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("hello!"));
    }

    #[tokio::test]
    async fn test_set_body_preserves_headers() {
        let mut msg = Message::default();
        msg.set_header("keep", camel_api::Value::Bool(true));
        let exchange = Exchange::new(msg);

        let svc = SetBody::new(
            IdentityProcessor,
            sync_source(|_ex: &Exchange| Value::String("new".into())),
        );
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.input.header("keep"),
            Some(&camel_api::Value::Bool(true))
        );
        assert_eq!(result.input.body.as_text(), Some("new"));
    }

    #[tokio::test]
    async fn test_set_body_layer_composes() {
        use tower::ServiceBuilder;

        let svc = ServiceBuilder::new()
            .layer(SetBodyLayer::new(sync_source(|_ex: &Exchange| {
                Value::String("layered".into())
            })))
            .service(IdentityProcessor);

        let exchange = Exchange::new(Message::default());
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("layered"));
    }
}
