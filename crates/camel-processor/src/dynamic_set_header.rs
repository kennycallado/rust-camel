use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::{CamelError, Exchange, ValueSource};

/// A processor that sets a header from a fallible [`ValueSource`].
///
/// The source is evaluated first; a failed evaluation fails the step WITHOUT
/// setting the header and WITHOUT invoking the inner service.
#[derive(Clone)]
pub struct DynamicSetHeader<P> {
    inner: P,
    key: String,
    source: ValueSource,
}

impl<P> DynamicSetHeader<P> {
    pub fn new(inner: P, key: impl Into<String>, source: impl Into<ValueSource>) -> Self {
        Self {
            inner,
            key: key.into(),
            source: source.into(),
        }
    }
}

/// A Tower Layer that wraps an inner service with a [`DynamicSetHeader`].
#[derive(Clone)]
pub struct DynamicSetHeaderLayer {
    key: String,
    source: ValueSource,
}

impl DynamicSetHeaderLayer {
    pub fn new(key: impl Into<String>, source: impl Into<ValueSource>) -> Self {
        Self {
            key: key.into(),
            source: source.into(),
        }
    }
}

impl<S> tower::Layer<S> for DynamicSetHeaderLayer {
    type Service = DynamicSetHeader<S>;

    fn layer(&self, inner: S) -> Self::Service {
        DynamicSetHeader {
            inner,
            key: self.key.clone(),
            source: self.source.clone(),
        }
    }
}

impl<P> Service<Exchange> for DynamicSetHeader<P>
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
            exchange.input.set_header(key, value);
            inner.call(exchange).await
        })
    }
}

/// A processor that sets a header from a fallible [`ValueSource`],
/// but ONLY if the header is not already present (if-absent semantics).
/// The presence check happens BEFORE expression evaluation — if the
/// header exists, the expression is never evaluated.
#[derive(Clone)]
pub struct DynamicSetHeaderIfAbsent<P> {
    inner: P,
    key: String,
    source: ValueSource,
}

impl<P> DynamicSetHeaderIfAbsent<P> {
    pub fn new(inner: P, key: impl Into<String>, source: impl Into<ValueSource>) -> Self {
        Self {
            inner,
            key: key.into(),
            source: source.into(),
        }
    }
}

impl<P> Service<Exchange> for DynamicSetHeaderIfAbsent<P>
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
        // Check BEFORE evaluating the expression: a present header skips
        // evaluation entirely (today's semantics).
        if exchange.input.header_ic(&self.key).is_some() {
            let clone = self.inner.clone();
            let mut inner = std::mem::replace(&mut self.inner, clone);
            return Box::pin(inner.call(exchange));
        }

        let source = self.source.clone();
        let key = self.key.clone();
        // Clone-and-replace: the future owns the (already polled) inner.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        Box::pin(async move {
            let value = source.evaluate(&exchange).await?;
            exchange.input.set_header(key, value);
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
    async fn dynamic_set_header_error_propagates() {
        let called = Arc::new(AtomicUsize::new(0));
        let svc = DynamicSetHeader::new(
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

    #[tokio::test]
    async fn dynamic_set_header_if_absent_error_propagates() {
        let called = Arc::new(AtomicUsize::new(0));
        let svc = DynamicSetHeaderIfAbsent::new(
            CountingInner {
                called: Arc::clone(&called),
            },
            "greeting",
            failing_source(),
        );

        // Header absent: the expression IS evaluated and its error propagates.
        let result = svc.oneshot(Exchange::new(Message::new("world"))).await;

        assert!(result.is_err(), "failed evaluation must fail the step");
        assert_eq!(
            called.load(Ordering::SeqCst),
            0,
            "inner must NOT run when evaluation fails"
        );
    }

    #[tokio::test]
    async fn dynamic_set_header_if_absent_present_header_skips_evaluation() {
        let evals = Arc::new(AtomicUsize::new(0));
        let evals_clone = evals.clone();
        let source = ValueSource::Async(Arc::new(move |_: &Exchange| {
            evals_clone.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(Value::Bool(true)) }) as BoxValueFuture
        }));

        let mut msg = Message::new("computed");
        msg.set_header("key", Value::String("original".into()));

        let svc = DynamicSetHeaderIfAbsent::new(IdentityProcessor, "key", source);
        let result = svc.oneshot(Exchange::new(msg)).await.unwrap();

        assert_eq!(
            result.input.header("key"),
            Some(&Value::String("original".into()))
        );
        assert_eq!(
            evals.load(Ordering::SeqCst),
            0,
            "expression must NOT be evaluated when the header is already present"
        );
    }

    #[tokio::test]
    async fn test_dynamic_set_header_from_body() {
        let exchange = Exchange::new(Message::new("world"));

        let svc = DynamicSetHeader::new(
            IdentityProcessor,
            "greeting",
            sync_source(|ex: &Exchange| {
                Value::String(format!("hello {}", ex.input.body.as_text().unwrap_or("")))
            }),
        );

        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.input.header("greeting"),
            Some(&Value::String("hello world".into()))
        );
    }

    #[tokio::test]
    async fn test_dynamic_set_header_overwrites_existing() {
        let mut msg = Message::new("new");
        msg.set_header("key", Value::String("old".into()));
        let exchange = Exchange::new(msg);

        let svc = DynamicSetHeader::new(
            IdentityProcessor,
            "key",
            sync_source(|ex: &Exchange| {
                Value::String(ex.input.body.as_text().unwrap_or("").into())
            }),
        );

        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.input.header("key"),
            Some(&Value::String("new".into()))
        );
    }

    #[tokio::test]
    async fn dynamic_set_header_replaces_different_case() {
        let mut msg = Message::default();
        msg.set_header("X-Flag", Value::String("old".into()));

        let svc = DynamicSetHeader::new(
            IdentityProcessor,
            "x-flag",
            sync_source(|_ex: &Exchange| Value::String("raw".into())),
        );

        let result = svc.oneshot(Exchange::new(msg)).await.unwrap();
        assert_eq!(
            result.input.header("x-flag"),
            Some(&Value::String("raw".into()))
        );
        assert_eq!(result.input.header("X-Flag"), None);
        assert_eq!(result.input.headers.len(), 1);
    }

    #[tokio::test]
    async fn test_dynamic_set_header_preserves_body() {
        let exchange = Exchange::new(Message::new("body content"));

        let svc = DynamicSetHeader::new(
            IdentityProcessor,
            "len",
            sync_source(|ex: &Exchange| {
                let len = ex.input.body.as_text().map(|t| t.len() as i64).unwrap_or(0);
                Value::Number(len.into())
            }),
        );

        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("body content"));
        assert_eq!(result.input.header("len"), Some(&Value::Number(12.into())));
    }

    #[tokio::test]
    async fn test_dynamic_set_header_layer_composes() {
        use tower::ServiceBuilder;

        let svc = ServiceBuilder::new()
            .layer(DynamicSetHeaderLayer::new(
                "computed",
                sync_source(|_ex: &Exchange| Value::Bool(true)),
            ))
            .service(IdentityProcessor);

        let exchange = Exchange::new(Message::default());
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.header("computed"), Some(&Value::Bool(true)));
    }

    // ── DynamicSetHeaderIfAbsent ──

    #[tokio::test]
    async fn test_dynamic_set_header_if_absent_adds_when_missing() {
        let exchange = Exchange::new(Message::new("world"));

        let svc = DynamicSetHeaderIfAbsent::new(
            IdentityProcessor,
            "greeting",
            sync_source(|ex: &Exchange| {
                Value::String(format!("hello {}", ex.input.body.as_text().unwrap_or("")))
            }),
        );

        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.input.header("greeting"),
            Some(&Value::String("hello world".into()))
        );
    }

    #[tokio::test]
    async fn test_dynamic_set_header_if_absent_preserves_existing() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let mut msg = Message::new("computed");
        msg.set_header("key", Value::String("original".into()));
        let exchange = Exchange::new(msg);

        // Side-effect counter: expression increments this.
        // If the header is present, the expression must NEVER be called.
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();

        let svc = DynamicSetHeaderIfAbsent::new(
            IdentityProcessor,
            "key",
            sync_source(move |ex: &Exchange| {
                cc.fetch_add(1, Ordering::SeqCst);
                Value::String(ex.input.body.as_text().unwrap_or("").into())
            }),
        );

        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.input.header("key"),
            Some(&Value::String("original".into()))
        );
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            0,
            "expression must NOT be evaluated when header is present"
        );
    }

    #[tokio::test]
    async fn dynamic_set_header_if_absent_respects_different_case() {
        let evals = Arc::new(AtomicUsize::new(0));
        let evals_clone = Arc::clone(&evals);
        let source = sync_source(move |_ex: &Exchange| {
            evals_clone.fetch_add(1, Ordering::SeqCst);
            Value::String("new".into())
        });

        let mut msg = Message::default();
        msg.set_header("X-Flag", Value::String("old".into()));

        let svc = DynamicSetHeaderIfAbsent::new(IdentityProcessor, "x-flag", source);
        let result = svc.oneshot(Exchange::new(msg)).await.unwrap();

        assert_eq!(
            result.input.header("X-Flag"),
            Some(&Value::String("old".into()))
        );
        assert_eq!(result.input.header("x-flag"), None);
        assert_eq!(result.input.headers.len(), 1);
        assert_eq!(
            evals.load(Ordering::SeqCst),
            0,
            "expression must NOT be evaluated when a different-case header is present"
        );
    }

    #[tokio::test]
    async fn dynamic_set_header_if_absent_sees_raw_map_different_case() {
        let evals = Arc::new(AtomicUsize::new(0));
        let evals_clone = Arc::clone(&evals);
        let source = sync_source(move |_ex: &Exchange| {
            evals_clone.fetch_add(1, Ordering::SeqCst);
            Value::String("new".into())
        });

        let mut msg = Message::default();
        msg.headers
            .insert("X-Flag".into(), Value::String("old".into()));

        let svc = DynamicSetHeaderIfAbsent::new(IdentityProcessor, "x-flag", source);
        let result = svc.oneshot(Exchange::new(msg)).await.unwrap();

        assert_eq!(
            result.input.header("X-Flag"),
            Some(&Value::String("old".into()))
        );
        assert_eq!(result.input.header("x-flag"), None);
        assert_eq!(result.input.headers.len(), 1);
        assert_eq!(
            evals.load(Ordering::SeqCst),
            0,
            "expression must NOT be evaluated when a raw-map case variant is present"
        );
    }

    #[tokio::test]
    async fn test_dynamic_set_header_if_absent_preserves_body() {
        let exchange = Exchange::new(Message::new("body content"));

        let svc = DynamicSetHeaderIfAbsent::new(
            IdentityProcessor,
            "len",
            sync_source(|ex: &Exchange| {
                let len = ex.input.body.as_text().map(|t| t.len() as i64).unwrap_or(0);
                Value::Number(len.into())
            }),
        );

        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("body content"));
        assert_eq!(result.input.header("len"), Some(&Value::Number(12.into())));
    }
}
