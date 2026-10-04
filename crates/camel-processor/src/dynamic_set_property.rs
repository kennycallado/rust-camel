use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::{CamelError, Exchange, ValueSource};

/// Sets an exchange property from a fallible [`ValueSource`].
///
/// The value source is evaluated first; a failed evaluation fails the step
/// WITHOUT setting the property and WITHOUT invoking the inner service.
#[derive(Clone)]
pub struct DynamicSetProperty<P> {
    inner: P,
    key: String,
    source: ValueSource,
}

impl<P> DynamicSetProperty<P> {
    pub fn new(inner: P, key: impl Into<String>, source: impl Into<ValueSource>) -> Self {
        Self {
            inner,
            key: key.into(),
            source: source.into(),
        }
    }
}

#[derive(Clone)]
pub struct DynamicSetPropertyLayer {
    key: String,
    source: ValueSource,
}

impl DynamicSetPropertyLayer {
    pub fn new(key: impl Into<String>, source: impl Into<ValueSource>) -> Self {
        Self {
            key: key.into(),
            source: source.into(),
        }
    }
}

impl<S> tower::Layer<S> for DynamicSetPropertyLayer {
    type Service = DynamicSetProperty<S>;

    fn layer(&self, inner: S) -> Self::Service {
        DynamicSetProperty {
            inner,
            key: self.key.clone(),
            source: self.source.clone(),
        }
    }
}

impl<P> Service<Exchange> for DynamicSetProperty<P>
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
        let key = self.key.clone();
        // Clone-and-replace: move the polled inner into the future so the
        // async body owns its service; a fresh clone stays behind.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        Box::pin(async move {
            let value = source.evaluate(&exchange).await?;
            exchange.set_property(key, value);
            inner.call(exchange).await
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use camel_api::{BoxValueFuture, Exchange, IdentityProcessor, Message, Value, ValueSource};
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
    async fn dynamic_set_property_error_sets_nothing() {
        let called = Arc::new(AtomicUsize::new(0));
        let svc = DynamicSetProperty::new(
            CountingInner {
                called: Arc::clone(&called),
            },
            "greeting",
            failing_source(),
        );

        let result = svc.oneshot(Exchange::new(Message::new("world"))).await;

        assert!(result.is_err(), "failed evaluation must fail the step");
        assert_eq!(
            called.load(Ordering::SeqCst),
            0,
            "inner must NOT run when evaluation fails"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn setter_works_on_current_thread_runtime() {
        let source = ValueSource::Async(Arc::new(|ex: &Exchange| {
            let text = ex.input.body.as_text().unwrap_or("").to_string();
            Box::pin(async move { Ok(Value::String(format!("hello {text}"))) }) as BoxValueFuture
        }));
        let svc = DynamicSetProperty::new(IdentityProcessor, "greeting", source);

        let result = svc
            .oneshot(Exchange::new(Message::new("world")))
            .await
            .unwrap();
        assert_eq!(
            result.property("greeting"),
            Some(&Value::String("hello world".into()))
        );
    }

    #[tokio::test]
    async fn setter_poll_ready_delegates_to_inner() {
        /// Inner service that reports Pending on its first poll and Ready on
        /// every poll after, counting how many times it was polled.
        #[derive(Clone)]
        struct PendingOnceInner {
            polls: Arc<AtomicUsize>,
        }

        impl Service<Exchange> for PendingOnceInner {
            type Response = Exchange;
            type Error = CamelError;
            type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

            fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
                if self.polls.fetch_add(1, Ordering::SeqCst) == 0 {
                    Poll::Pending
                } else {
                    Poll::Ready(Ok(()))
                }
            }

            fn call(&mut self, exchange: Exchange) -> Self::Future {
                Box::pin(async { Ok(exchange) })
            }
        }

        let polls = Arc::new(AtomicUsize::new(0));
        let mut svc = DynamicSetProperty::new(
            PendingOnceInner {
                polls: Arc::clone(&polls),
            },
            "k",
            sync_source(|_: &Exchange| Value::String("v".into())),
        );

        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        let first = Pin::new(&mut svc).poll_ready(&mut cx);
        assert!(
            first.is_pending(),
            "first poll_ready must surface the inner Pending, got {first:?}"
        );

        let second = Pin::new(&mut svc).poll_ready(&mut cx);
        assert!(
            matches!(second, Poll::Ready(Ok(()))),
            "second poll_ready must surface the inner Ready, got {second:?}"
        );

        let result = svc
            .call(Exchange::new(Message::new("world")))
            .await
            .expect("call must complete through the inner service");
        assert_eq!(result.property("k"), Some(&Value::String("v".into())));
        assert_eq!(
            polls.load(Ordering::SeqCst),
            2,
            "inner must be polled exactly twice before call"
        );
    }

    #[tokio::test]
    async fn test_dynamic_set_property_from_body() {
        let exchange = Exchange::new(Message::new("world"));

        let svc = DynamicSetProperty::new(
            IdentityProcessor,
            "greeting",
            sync_source(|ex: &Exchange| {
                Value::String(format!("hello {}", ex.input.body.as_text().unwrap_or("")))
            }),
        );

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

        let svc = DynamicSetProperty::new(
            IdentityProcessor,
            "key",
            sync_source(|ex: &Exchange| {
                Value::String(ex.input.body.as_text().unwrap_or("").into())
            }),
        );

        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.property("key"), Some(&Value::String("new".into())));
    }

    #[tokio::test]
    async fn test_dynamic_set_property_preserves_body() {
        let exchange = Exchange::new(Message::new("body content"));

        let svc = DynamicSetProperty::new(
            IdentityProcessor,
            "len",
            sync_source(|ex: &Exchange| {
                let len = ex.input.body.as_text().map(|t| t.len() as i64).unwrap_or(0);
                Value::Number(len.into())
            }),
        );

        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("body content"));
        assert_eq!(result.property("len"), Some(&Value::Number(12.into())));
    }

    #[tokio::test]
    async fn test_dynamic_set_property_layer_composes() {
        use tower::ServiceBuilder;

        let svc = ServiceBuilder::new()
            .layer(DynamicSetPropertyLayer::new(
                "computed",
                sync_source(|_ex: &Exchange| Value::Bool(true)),
            ))
            .service(IdentityProcessor);

        let exchange = Exchange::new(Message::default());
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.property("computed"), Some(&Value::Bool(true)));
    }
}
