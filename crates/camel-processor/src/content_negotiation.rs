use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::{CamelError, Exchange, Value};

/// Injected verdict closure for content negotiation.
///
/// Receives the resolved `Content-Type` and `Accept` header values
/// (`None` when absent or not a JSON string) and returns `Ok(())` to pass,
/// or a [`CamelError`] (e.g. `UnsupportedMediaType` / `NotAcceptable`) to
/// reject the exchange.
pub type ContentNegotiationCheck =
    Arc<dyn Fn(Option<&str>, Option<&str>) -> Result<(), CamelError> + Send + Sync>;

/// Header-only media negotiation gate (REST DSL v2 strict mode).
///
/// The verdict is injected at compile time as a [`ContentNegotiationCheck`]
/// closure; the processor never touches the message body (no polling, no
/// materialization) and passes the exchange through unchanged when the
/// check accepts it.
#[derive(Clone)]
pub struct ContentNegotiationProcessor {
    check: ContentNegotiationCheck,
}

impl ContentNegotiationProcessor {
    /// Create a new gate with the given verdict closure.
    pub fn new(check: ContentNegotiationCheck) -> Self {
        Self { check }
    }
}

impl Service<Exchange> for ContentNegotiationProcessor {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let check = Arc::clone(&self.check);
        Box::pin(async move {
            let content_type = exchange
                .input
                .header_ic("Content-Type")
                .and_then(Value::as_str);
            let accept = exchange.input.header_ic("Accept").and_then(Value::as_str);
            check(content_type, accept)?;
            Ok(exchange)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use camel_api::{Body, Message, StreamBody};
    use futures::Stream;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    type SeenArgs = Vec<(Option<String>, Option<String>)>;

    /// One-chunk stream that counts every `poll_next` on a shared counter.
    struct PollCountingStream {
        inner: futures::stream::Iter<std::vec::IntoIter<Result<Bytes, CamelError>>>,
        polls: Arc<AtomicUsize>,
    }

    impl Stream for PollCountingStream {
        type Item = Result<Bytes, CamelError>;

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            self.polls.fetch_add(1, Ordering::SeqCst);
            Pin::new(&mut self.inner).poll_next(cx)
        }
    }

    fn counting_stream_body(polls: Arc<AtomicUsize>) -> Body {
        let chunks: Vec<Result<Bytes, CamelError>> = vec![Ok(Bytes::from_static(b"payload"))];
        let stream = PollCountingStream {
            inner: futures::stream::iter(chunks),
            polls,
        };
        Body::Stream(StreamBody {
            stream: Arc::new(tokio::sync::Mutex::new(Some(Box::pin(stream)))),
            metadata: Default::default(),
        })
    }

    fn recording_check(seen: Arc<std::sync::Mutex<SeenArgs>>) -> ContentNegotiationCheck {
        Arc::new(move |content_type: Option<&str>, accept: Option<&str>| {
            seen.lock()
                .unwrap()
                .push((content_type.map(str::to_string), accept.map(str::to_string)));
            Ok(())
        })
    }

    #[tokio::test]
    async fn gate_passes_exchange_through_untouched() {
        let polls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut msg = Message::default();
        msg.set_header("Content-Type", "application/json");
        msg.set_header("Accept", "application/json");
        msg.body = counting_stream_body(Arc::clone(&polls));
        let headers_before = msg.headers.clone();
        let exchange = Exchange::new(msg);

        let processor = ContentNegotiationProcessor::new(recording_check(Arc::clone(&seen)));

        let result = processor.oneshot(exchange).await.unwrap();

        assert_eq!(result.input.headers, headers_before);
        assert!(matches!(result.input.body, Body::Stream { .. }));
        assert_eq!(polls.load(Ordering::SeqCst), 0);
        let seen_args = seen.lock().unwrap();
        assert_eq!(seen_args.len(), 1);
        assert_eq!(
            seen_args[0],
            (
                Some("application/json".to_string()),
                Some("application/json".to_string())
            )
        );
    }

    #[tokio::test]
    async fn gate_propagates_check_error() {
        let mut msg = Message::default();
        msg.set_header("Content-Type", "text/plain");
        let exchange = Exchange::new(msg);

        let check: ContentNegotiationCheck = Arc::new(|_content_type, _accept| {
            Err(CamelError::UnsupportedMediaType {
                consumed: "text/plain".into(),
                declared: "application/json".into(),
            })
        });
        let processor = ContentNegotiationProcessor::new(check);

        let result = processor.oneshot(exchange).await;

        match result {
            Err(CamelError::UnsupportedMediaType { consumed, declared }) => {
                assert_eq!(consumed, "text/plain");
                assert_eq!(declared, "application/json");
            }
            other => panic!("expected UnsupportedMediaType, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn gate_header_lookup_resolves_casings() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut msg = Message::default();
        msg.set_header("content-type", "application/json");
        msg.set_header("accept", "*/*");
        let exchange = Exchange::new(msg);

        let processor = ContentNegotiationProcessor::new(recording_check(Arc::clone(&seen)));

        processor.oneshot(exchange).await.unwrap();

        let seen_args = seen.lock().unwrap();
        assert_eq!(seen_args.len(), 1);
        assert_eq!(
            seen_args[0],
            (
                Some("application/json".to_string()),
                Some("*/*".to_string())
            )
        );
    }

    #[tokio::test]
    async fn gate_absent_headers_yield_none() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));

        let exchange = Exchange::new(Message::default());

        let processor = ContentNegotiationProcessor::new(recording_check(Arc::clone(&seen)));

        let result = processor.oneshot(exchange).await.unwrap();

        let seen_args = seen.lock().unwrap();
        assert_eq!(seen_args[0], (None, None));
        assert!(result.input.headers.is_empty());
    }

    #[tokio::test]
    async fn gate_non_string_header_values_yield_none() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut msg = Message::default();
        msg.set_header("Content-Type", 1);
        msg.set_header("Accept", true);
        let exchange = Exchange::new(msg);

        let processor = ContentNegotiationProcessor::new(recording_check(Arc::clone(&seen)));

        let result = processor.oneshot(exchange).await.unwrap();

        let seen_args = seen.lock().unwrap();
        assert_eq!(seen_args[0], (None, None));
        assert!(result.input.headers.contains_key("Content-Type"));
    }
}
