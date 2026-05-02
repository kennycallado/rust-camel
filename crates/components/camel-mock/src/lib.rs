use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::sync::{Mutex, Notify};
use tower::Service;

use camel_component_api::parse_uri;
use camel_component_api::{BoxProcessor, CamelError, Exchange};
use camel_component_api::{Component, Consumer, Endpoint, ProducerContext};

// ---------------------------------------------------------------------------
// MockComponent
// ---------------------------------------------------------------------------

/// The Mock component is a testing utility that records every exchange it
/// receives via its producer.  It exposes helpers to inspect and assert on
/// the recorded exchanges.
///
/// URI format: `mock:name`
///
/// When `create_endpoint` is called multiple times with the same name, the
/// returned endpoints share the same received-exchanges storage. This enables
/// test assertions: create mock, register it, run routes, then inspect via
/// `component.get_endpoint("name")`.
#[derive(Clone)]
pub struct MockComponent {
    registry: Arc<std::sync::Mutex<HashMap<String, Arc<MockEndpointInner>>>>,
}

impl MockComponent {
    pub fn new() -> Self {
        Self {
            registry: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Retrieve a previously created endpoint's inner data by name.
    ///
    /// This is the primary way to inspect recorded exchanges in tests.
    pub fn get_endpoint(&self, name: &str) -> Option<Arc<MockEndpointInner>> {
        let registry = self
            .registry
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock");
        registry.get(name).cloned()
    }
}

impl Default for MockComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for MockComponent {
    fn scheme(&self) -> &str {
        "mock"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let parts = parse_uri(uri)?;
        if parts.scheme != "mock" {
            return Err(CamelError::InvalidUri(format!(
                "expected scheme 'mock', got '{}'",
                parts.scheme
            )));
        }

        let name = parts.path;
        let mut registry = self.registry.lock().map_err(|e| {
            CamelError::EndpointCreationFailed(format!("mock registry lock poisoned: {e}"))
        })?;
        let inner = registry
            .entry(name.clone())
            .or_insert_with(|| {
                Arc::new(MockEndpointInner {
                    uri: uri.to_string(),
                    name,
                    received: Arc::new(Mutex::new(Vec::new())),
                    notify: Arc::new(Notify::new()),
                })
            })
            .clone();

        Ok(Box::new(MockEndpoint(inner)))
    }
}

// ---------------------------------------------------------------------------
// MockEndpoint / MockEndpointInner
// ---------------------------------------------------------------------------

/// A mock endpoint that records all exchanges sent to it.
///
/// This is a thin wrapper around `Arc<MockEndpointInner>`. Multiple
/// `MockEndpoint` instances created with the same name share the same inner
/// storage.
pub struct MockEndpoint(Arc<MockEndpointInner>);

/// The actual data behind a mock endpoint. Shared across all `MockEndpoint`
/// instances created with the same name via `MockComponent`.
///
/// Use `get_received_exchanges` and `assert_exchange_count` to inspect
/// recorded exchanges in tests.
pub struct MockEndpointInner {
    uri: String,
    pub name: String,
    received: Arc<Mutex<Vec<Exchange>>>,
    notify: Arc<Notify>,
}

impl MockEndpointInner {
    /// Return a snapshot of all exchanges received so far.
    pub async fn get_received_exchanges(&self) -> Vec<Exchange> {
        self.received.lock().await.clone()
    }

    /// Assert that exactly `expected` exchanges have been received.
    ///
    /// # Panics
    ///
    /// Panics if the count does not match.
    pub async fn assert_exchange_count(&self, expected: usize) {
        let actual = self.received.lock().await.len();
        assert_eq!(
            actual, expected,
            "MockEndpoint expected {expected} exchanges, got {actual}"
        );
    }

    /// Wait until at least `count` exchanges have been received, or panic on timeout.
    ///
    /// Uses `tokio::sync::Notify` — no polling. Returns immediately if `count`
    /// exchanges are already present.
    ///
    /// # Panics
    ///
    /// Panics if `timeout` elapses before `count` exchanges arrive.
    pub async fn await_exchanges(&self, count: usize, timeout: std::time::Duration) {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            {
                let received = self.received.lock().await;
                if received.len() >= count {
                    return;
                }
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                // Re-check in case the final exchange arrived between the lock drop
                // above and entering the select — Notify does not buffer permits.
                let got = self.received.lock().await.len();
                if got >= count {
                    return;
                }
                panic!(
                    "MockEndpoint '{}': timed out waiting for {} exchanges (got {} after {:?})",
                    self.name, count, got, timeout
                );
            }
            tokio::select! {
                _ = self.notify.notified() => {}
                _ = tokio::time::sleep(remaining) => {}
            }
        }
    }

    /// Return an [`ExchangeAssert`] for the exchange at `idx`.
    ///
    /// # Panics
    ///
    /// Panics if `idx` is out of bounds. Always call [`await_exchanges`] first
    /// to ensure the exchange has been received.
    ///
    /// Panics if called from a single-threaded tokio runtime. Use
    /// `#[tokio::test(flavor = "multi_thread")]` for tests that call this method.
    ///
    /// [`await_exchanges`]: MockEndpointInner::await_exchanges
    pub fn exchange(&self, idx: usize) -> ExchangeAssert {
        let received = tokio::task::block_in_place(|| self.received.blocking_lock());
        if idx >= received.len() {
            panic!(
                "MockEndpoint '{}': exchange index {} out of bounds (got {} exchanges)",
                self.name,
                idx,
                received.len()
            );
        }
        ExchangeAssert {
            exchange: received[idx].clone(),
            idx,
            endpoint_name: self.name.clone(),
        }
    }
}

impl Endpoint for MockEndpoint {
    fn uri(&self) -> &str {
        &self.0.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Err(CamelError::EndpointCreationFailed(
            "mock endpoint does not support consumers (it is a sink)".to_string(),
        ))
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(MockProducer {
            received: Arc::clone(&self.0.received),
            notify: Arc::clone(&self.0.notify),
        }))
    }
}

// ---------------------------------------------------------------------------
// MockProducer
// ---------------------------------------------------------------------------

/// A producer that simply records each exchange it processes.
#[derive(Clone)]
struct MockProducer {
    received: Arc<Mutex<Vec<Exchange>>>,
    notify: Arc<Notify>,
}

impl Service<Exchange> for MockProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let received = Arc::clone(&self.received);
        let notify = Arc::clone(&self.notify);
        Box::pin(async move {
            received.lock().await.push(exchange.clone());
            notify.notify_waiters();
            Ok(exchange)
        })
    }
}

// ---------------------------------------------------------------------------
// ExchangeAssert
// ---------------------------------------------------------------------------

/// A handle for making synchronous assertions on a recorded exchange.
///
/// Obtain one via [`MockEndpointInner::exchange`] after calling
/// [`MockEndpointInner::await_exchanges`].
///
/// All methods panic with descriptive messages on failure, making test output
/// self-explanatory without additional context.
pub struct ExchangeAssert {
    exchange: Exchange,
    idx: usize,
    endpoint_name: String,
}

impl ExchangeAssert {
    fn location(&self) -> String {
        format!(
            "MockEndpoint '{}' exchange[{}]",
            self.endpoint_name, self.idx
        )
    }

    /// Assert that the body is `Body::Text` equal to `expected`.
    pub fn assert_body_text(self, expected: &str) -> Self {
        match self.exchange.input.body.as_text() {
            Some(actual) if actual == expected => {}
            Some(actual) => panic!(
                "{}: expected body text {:?}, got {:?}",
                self.location(),
                expected,
                actual
            ),
            None => panic!(
                "{}: expected body text {:?}, but body is not Body::Text (got {:?})",
                self.location(),
                expected,
                self.exchange.input.body
            ),
        }
        self
    }

    /// Assert that the body is `Body::Json` equal to `expected`.
    pub fn assert_body_json(self, expected: serde_json::Value) -> Self {
        match &self.exchange.input.body {
            camel_component_api::Body::Json(actual) if *actual == expected => {}
            camel_component_api::Body::Json(actual) => panic!(
                "{}: expected body JSON {}, got {}",
                self.location(),
                expected,
                actual
            ),
            other => panic!(
                "{}: expected body JSON {}, but body is not Body::Json (got {:?})",
                self.location(),
                expected,
                other
            ),
        }
        self
    }

    /// Assert that the body is `Body::Bytes` equal to `expected`.
    pub fn assert_body_bytes(self, expected: &[u8]) -> Self {
        match &self.exchange.input.body {
            camel_component_api::Body::Bytes(actual) if actual.as_ref() == expected => {}
            camel_component_api::Body::Bytes(actual) => panic!(
                "{}: expected body bytes {:?}, got {:?}",
                self.location(),
                expected,
                actual
            ),
            other => panic!(
                "{}: expected body bytes {:?}, but body is not Body::Bytes (got {:?})",
                self.location(),
                expected,
                other
            ),
        }
        self
    }

    /// Assert that header `key` exists and equals `expected`.
    ///
    /// # Panics
    ///
    /// Panics if the header is missing or its value does not match `expected`.
    pub fn assert_header(self, key: &str, expected: serde_json::Value) -> Self {
        match self.exchange.input.headers.get(key) {
            Some(actual) if *actual == expected => {}
            Some(actual) => panic!(
                "{}: expected header {:?} = {}, got {}",
                self.location(),
                key,
                expected,
                actual
            ),
            None => panic!(
                "{}: expected header {:?} = {}, but header is absent",
                self.location(),
                key,
                expected
            ),
        }
        self
    }

    /// Assert that header `key` is present (any value).
    ///
    /// # Panics
    ///
    /// Panics if the header key is absent.
    pub fn assert_header_exists(self, key: &str) -> Self {
        if !self.exchange.input.headers.contains_key(key) {
            panic!(
                "{}: expected header {:?} to be present, but it was absent",
                self.location(),
                key
            );
        }
        self
    }

    /// Assert that the exchange has an error (`exchange.error` is `Some`).
    ///
    /// # Panics
    ///
    /// Panics if `exchange.error` is `None`.
    pub fn assert_has_error(self) -> Self {
        if self.exchange.error.is_none() {
            panic!(
                "{}: expected exchange to have an error, but error is None",
                self.location()
            );
        }
        self
    }

    /// Assert that the exchange has no error (`exchange.error` is `None`).
    ///
    /// # Panics
    ///
    /// Panics if `exchange.error` is `Some`.
    pub fn assert_no_error(self) -> Self {
        if let Some(ref err) = self.exchange.error {
            panic!(
                "{}: expected exchange to have no error, but got: {}",
                self.location(),
                err
            );
        }
        self
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::Message;
    use camel_component_api::NoOpComponentContext;
    use tower::ServiceExt;

    fn test_producer_ctx() -> ProducerContext {
        ProducerContext::new()
    }

    #[test]
    fn test_mock_component_scheme() {
        let component = MockComponent::new();
        assert_eq!(component.scheme(), "mock");
    }

    #[test]
    fn test_mock_component_default() {
        let component = MockComponent::default();
        assert_eq!(component.scheme(), "mock");
        assert!(component.get_endpoint("missing").is_none());
    }

    #[test]
    fn test_mock_creates_endpoint() {
        let component = MockComponent::new();
        let endpoint = component.create_endpoint("mock:result", &NoOpComponentContext);
        assert!(endpoint.is_ok());
    }

    #[test]
    fn test_mock_wrong_scheme() {
        let component = MockComponent::new();
        let result = component.create_endpoint("timer:tick", &NoOpComponentContext);
        assert!(result.is_err());
    }

    #[test]
    fn test_mock_endpoint_no_consumer() {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:result", &NoOpComponentContext)
            .unwrap();
        assert!(endpoint.create_consumer().is_err());
    }

    #[test]
    fn test_mock_endpoint_creates_producer() {
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:result", &NoOpComponentContext)
            .unwrap();
        assert!(endpoint.create_producer(&ctx).is_ok());
    }

    #[test]
    fn test_mock_endpoint_uri() {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:uri-check", &NoOpComponentContext)
            .unwrap();
        assert_eq!(endpoint.uri(), "mock:uri-check");
    }

    #[test]
    fn test_mock_get_endpoint_returns_same_inner_for_same_name() {
        let component = MockComponent::new();
        let _ = component
            .create_endpoint("mock:shared-inner", &NoOpComponentContext)
            .unwrap();
        let _ = component
            .create_endpoint("mock:shared-inner", &NoOpComponentContext)
            .unwrap();

        let first = component.get_endpoint("shared-inner").unwrap();
        let second = component.get_endpoint("shared-inner").unwrap();
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[tokio::test]
    async fn test_mock_producer_records_exchange() {
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:test", &NoOpComponentContext)
            .unwrap();

        let mut producer = endpoint.create_producer(&ctx).unwrap();

        let ex1 = Exchange::new(Message::new("first"));
        let ex2 = Exchange::new(Message::new("second"));

        producer.call(ex1).await.unwrap();
        producer.call(ex2).await.unwrap();

        let inner = component.get_endpoint("test").unwrap();
        inner.assert_exchange_count(2).await;

        let received = inner.get_received_exchanges().await;
        assert_eq!(received[0].input.body.as_text(), Some("first"));
        assert_eq!(received[1].input.body.as_text(), Some("second"));
    }

    #[tokio::test]
    async fn test_mock_producer_passes_through_exchange() {
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:passthrough", &NoOpComponentContext)
            .unwrap();

        let producer = endpoint.create_producer(&ctx).unwrap();
        let exchange = Exchange::new(Message::new("hello"));
        let result = producer.oneshot(exchange).await.unwrap();

        // Producer should return the exchange unchanged
        assert_eq!(result.input.body.as_text(), Some("hello"));
    }

    #[tokio::test]
    async fn test_mock_assert_count_passes() {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:count", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("count").unwrap();

        inner.assert_exchange_count(0).await;

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(&ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("one")))
            .await
            .unwrap();

        inner.assert_exchange_count(1).await;
    }

    #[tokio::test]
    #[should_panic(expected = "MockEndpoint expected 5 exchanges, got 0")]
    async fn test_mock_assert_count_fails() {
        let component = MockComponent::new();
        // Endpoint not created yet, so get_endpoint returns None.
        // Create it first, then assert.
        let _endpoint = component
            .create_endpoint("mock:fail", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("fail").unwrap();

        inner.assert_exchange_count(5).await;
    }

    #[tokio::test]
    async fn test_mock_component_shared_registry() {
        let component = MockComponent::new();
        let ep1 = component
            .create_endpoint("mock:shared", &NoOpComponentContext)
            .unwrap();
        let ep2 = component
            .create_endpoint("mock:shared", &NoOpComponentContext)
            .unwrap();

        // Producing via ep1's producer...
        let ctx = test_producer_ctx();
        let mut p1 = ep1.create_producer(&ctx).unwrap();
        p1.call(Exchange::new(Message::new("from-ep1")))
            .await
            .unwrap();

        // ...and via ep2's producer...
        let mut p2 = ep2.create_producer(&ctx).unwrap();
        p2.call(Exchange::new(Message::new("from-ep2")))
            .await
            .unwrap();

        // ...both should be visible via the shared storage
        let inner = component.get_endpoint("shared").unwrap();
        inner.assert_exchange_count(2).await;

        let received = inner.get_received_exchanges().await;
        assert_eq!(received[0].input.body.as_text(), Some("from-ep1"));
        assert_eq!(received[1].input.body.as_text(), Some("from-ep2"));
    }

    #[tokio::test]
    async fn await_exchanges_resolves_immediately() {
        // If exchanges are already present, await_exchanges returns without timeout.
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:immediate", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("immediate").unwrap();

        let mut producer = endpoint.create_producer(&ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("a")))
            .await
            .unwrap();
        producer
            .call(Exchange::new(Message::new("b")))
            .await
            .unwrap();

        // Should return immediately — both exchanges already received.
        inner
            .await_exchanges(2, std::time::Duration::from_millis(100))
            .await;
    }

    #[tokio::test]
    async fn await_exchanges_waits_then_resolves() {
        // await_exchanges unblocks when a producer sends after the call.
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:waiter", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("waiter").unwrap();

        // Spawn producer that sends after a short delay.
        let mut producer = endpoint.create_producer(&ctx).unwrap();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            producer
                .call(Exchange::new(Message::new("delayed")))
                .await
                .unwrap();
        });

        // This should block until the spawned task delivers the exchange.
        inner
            .await_exchanges(1, std::time::Duration::from_millis(500))
            .await;

        let received = inner.get_received_exchanges().await;
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].input.body.as_text(), Some("delayed"));
    }

    #[tokio::test]
    #[should_panic(expected = "timed out waiting for 5 exchanges")]
    async fn await_exchanges_times_out() {
        let component = MockComponent::new();
        let _endpoint = component
            .create_endpoint("mock:timeout", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("timeout").unwrap();

        // Nobody sends — should panic after timeout.
        inner
            .await_exchanges(5, std::time::Duration::from_millis(50))
            .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn exchange_idx_returns_assert() {
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:assert-idx", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("assert-idx").unwrap();

        let mut producer = endpoint.create_producer(&ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("hello")))
            .await
            .unwrap();

        inner
            .await_exchanges(1, std::time::Duration::from_millis(500))
            .await;
        // Should not panic — index 0 exists.
        let _assert = inner.exchange(0);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[should_panic(expected = "exchange index 5 out of bounds")]
    async fn exchange_idx_out_of_bounds() {
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:oob", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("oob").unwrap();

        let mut producer = endpoint.create_producer(&ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("only-one")))
            .await
            .unwrap();

        inner
            .await_exchanges(1, std::time::Duration::from_millis(500))
            .await;
        // Only 1 exchange, index 5 should panic.
        let _assert = inner.exchange(5);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn assert_body_text_pass() {
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:body-text-pass", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("body-text-pass").unwrap();
        let mut producer = endpoint.create_producer(&ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("hello")))
            .await
            .unwrap();
        inner
            .await_exchanges(1, std::time::Duration::from_millis(500))
            .await;
        inner.exchange(0).assert_body_text("hello");
    }

    #[tokio::test(flavor = "multi_thread")]
    #[should_panic(expected = "expected body text")]
    async fn assert_body_text_fail() {
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:body-text-fail", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("body-text-fail").unwrap();
        let mut producer = endpoint.create_producer(&ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("hello")))
            .await
            .unwrap();
        inner
            .await_exchanges(1, std::time::Duration::from_millis(500))
            .await;
        inner.exchange(0).assert_body_text("world");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn assert_body_json_pass() {
        use camel_component_api::Body;
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:body-json-pass", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("body-json-pass").unwrap();
        let mut producer = endpoint.create_producer(&ctx).unwrap();
        let mut msg = Message::new("");
        msg.body = Body::Json(serde_json::json!({"key": "value"}));
        producer.call(Exchange::new(msg)).await.unwrap();
        inner
            .await_exchanges(1, std::time::Duration::from_millis(500))
            .await;
        inner
            .exchange(0)
            .assert_body_json(serde_json::json!({"key": "value"}));
    }

    #[tokio::test(flavor = "multi_thread")]
    #[should_panic(expected = "expected body JSON")]
    async fn assert_body_json_fail() {
        use camel_component_api::Body;
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:body-json-fail", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("body-json-fail").unwrap();
        let mut producer = endpoint.create_producer(&ctx).unwrap();
        let mut msg = Message::new("");
        msg.body = Body::Json(serde_json::json!({"key": "value"}));
        producer.call(Exchange::new(msg)).await.unwrap();
        inner
            .await_exchanges(1, std::time::Duration::from_millis(500))
            .await;
        inner
            .exchange(0)
            .assert_body_json(serde_json::json!({"key": "other"}));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn assert_body_bytes_pass() {
        use bytes::Bytes;
        use camel_component_api::Body;
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:body-bytes-pass", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("body-bytes-pass").unwrap();
        let mut producer = endpoint.create_producer(&ctx).unwrap();
        let mut msg = Message::new("");
        msg.body = Body::Bytes(Bytes::from_static(b"binary"));
        producer.call(Exchange::new(msg)).await.unwrap();
        inner
            .await_exchanges(1, std::time::Duration::from_millis(500))
            .await;
        inner.exchange(0).assert_body_bytes(b"binary");
    }

    #[tokio::test(flavor = "multi_thread")]
    #[should_panic(expected = "expected body bytes")]
    async fn assert_body_bytes_fail() {
        use bytes::Bytes;
        use camel_component_api::Body;
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:body-bytes-fail", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("body-bytes-fail").unwrap();
        let mut producer = endpoint.create_producer(&ctx).unwrap();
        let mut msg = Message::new("");
        msg.body = Body::Bytes(Bytes::from_static(b"binary"));
        producer.call(Exchange::new(msg)).await.unwrap();
        inner
            .await_exchanges(1, std::time::Duration::from_millis(500))
            .await;
        inner.exchange(0).assert_body_bytes(b"different");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn assert_header_pass() {
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:hdr-pass", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("hdr-pass").unwrap();
        let mut producer = endpoint.create_producer(&ctx).unwrap();
        let mut msg = Message::new("body");
        msg.headers
            .insert("x-key".to_string(), serde_json::json!("value"));
        producer.call(Exchange::new(msg)).await.unwrap();
        inner
            .await_exchanges(1, std::time::Duration::from_millis(500))
            .await;
        inner
            .exchange(0)
            .assert_header("x-key", serde_json::json!("value"));
    }

    #[tokio::test(flavor = "multi_thread")]
    #[should_panic(expected = "expected header")]
    async fn assert_header_fail() {
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:hdr-fail", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("hdr-fail").unwrap();
        let mut producer = endpoint.create_producer(&ctx).unwrap();
        let mut msg = Message::new("body");
        msg.headers
            .insert("x-key".to_string(), serde_json::json!("value"));
        producer.call(Exchange::new(msg)).await.unwrap();
        inner
            .await_exchanges(1, std::time::Duration::from_millis(500))
            .await;
        inner
            .exchange(0)
            .assert_header("x-key", serde_json::json!("other"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn assert_header_exists_pass() {
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:hdr-exists-pass", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("hdr-exists-pass").unwrap();
        let mut producer = endpoint.create_producer(&ctx).unwrap();
        let mut msg = Message::new("body");
        msg.headers
            .insert("x-present".to_string(), serde_json::json!(42));
        producer.call(Exchange::new(msg)).await.unwrap();
        inner
            .await_exchanges(1, std::time::Duration::from_millis(500))
            .await;
        inner.exchange(0).assert_header_exists("x-present");
    }

    #[tokio::test(flavor = "multi_thread")]
    #[should_panic(expected = "expected header")]
    async fn assert_header_exists_fail() {
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:hdr-exists-fail", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("hdr-exists-fail").unwrap();
        let mut producer = endpoint.create_producer(&ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("body")))
            .await
            .unwrap();
        inner
            .await_exchanges(1, std::time::Duration::from_millis(500))
            .await;
        inner.exchange(0).assert_header_exists("x-missing");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn assert_has_error_pass() {
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:err-pass", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("err-pass").unwrap();
        let mut producer = endpoint.create_producer(&ctx).unwrap();
        let mut ex = Exchange::new(Message::new("body"));
        ex.error = Some(camel_component_api::CamelError::ProcessorError(
            "oops".to_string(),
        ));
        producer.call(ex).await.unwrap();
        inner
            .await_exchanges(1, std::time::Duration::from_millis(500))
            .await;
        inner.exchange(0).assert_has_error();
    }

    #[tokio::test(flavor = "multi_thread")]
    #[should_panic(expected = "expected exchange to have an error")]
    async fn assert_has_error_fail() {
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:has-err-fail", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("has-err-fail").unwrap();
        let mut producer = endpoint.create_producer(&ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("body")))
            .await
            .unwrap();
        inner
            .await_exchanges(1, std::time::Duration::from_millis(500))
            .await;
        inner.exchange(0).assert_has_error();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn assert_no_error_pass() {
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:no-err-pass", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("no-err-pass").unwrap();
        let mut producer = endpoint.create_producer(&ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("body")))
            .await
            .unwrap();
        inner
            .await_exchanges(1, std::time::Duration::from_millis(500))
            .await;
        inner.exchange(0).assert_no_error();
    }

    #[tokio::test(flavor = "multi_thread")]
    #[should_panic(expected = "expected exchange to have no error")]
    async fn assert_no_error_fail() {
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:no-err-fail", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("no-err-fail").unwrap();
        let mut producer = endpoint.create_producer(&ctx).unwrap();
        let mut ex = Exchange::new(Message::new("body"));
        ex.error = Some(camel_component_api::CamelError::ProcessorError(
            "oops".to_string(),
        ));
        producer.call(ex).await.unwrap();
        inner
            .await_exchanges(1, std::time::Duration::from_millis(500))
            .await;
        inner.exchange(0).assert_no_error();
    }
}
