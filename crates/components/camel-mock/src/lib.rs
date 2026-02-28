use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::sync::Mutex;
use tower::Service;

use camel_api::{BoxProcessor, CamelError, Exchange};
use camel_component::{Component, Consumer, Endpoint};
use camel_endpoint::parse_uri;

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
        let registry = self.registry.lock().unwrap();
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

    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
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

    fn create_producer(&self) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(MockProducer {
            received: Arc::clone(&self.0.received),
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
        Box::pin(async move {
            received.lock().await.push(exchange.clone());
            Ok(exchange)
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::Message;
    use tower::ServiceExt;

    #[test]
    fn test_mock_component_scheme() {
        let component = MockComponent::new();
        assert_eq!(component.scheme(), "mock");
    }

    #[test]
    fn test_mock_creates_endpoint() {
        let component = MockComponent::new();
        let endpoint = component.create_endpoint("mock:result");
        assert!(endpoint.is_ok());
    }

    #[test]
    fn test_mock_wrong_scheme() {
        let component = MockComponent::new();
        let result = component.create_endpoint("timer:tick");
        assert!(result.is_err());
    }

    #[test]
    fn test_mock_endpoint_no_consumer() {
        let component = MockComponent::new();
        let endpoint = component.create_endpoint("mock:result").unwrap();
        assert!(endpoint.create_consumer().is_err());
    }

    #[test]
    fn test_mock_endpoint_creates_producer() {
        let component = MockComponent::new();
        let endpoint = component.create_endpoint("mock:result").unwrap();
        assert!(endpoint.create_producer().is_ok());
    }

    #[tokio::test]
    async fn test_mock_producer_records_exchange() {
        let component = MockComponent::new();
        let endpoint = component.create_endpoint("mock:test").unwrap();

        let mut producer = endpoint.create_producer().unwrap();

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
        let component = MockComponent::new();
        let endpoint = component.create_endpoint("mock:passthrough").unwrap();

        let producer = endpoint.create_producer().unwrap();
        let exchange = Exchange::new(Message::new("hello"));
        let result = producer.oneshot(exchange).await.unwrap();

        // Producer should return the exchange unchanged
        assert_eq!(result.input.body.as_text(), Some("hello"));
    }

    #[tokio::test]
    async fn test_mock_assert_count_passes() {
        let component = MockComponent::new();
        let endpoint = component.create_endpoint("mock:count").unwrap();
        let inner = component.get_endpoint("count").unwrap();

        inner.assert_exchange_count(0).await;

        let mut producer = endpoint.create_producer().unwrap();
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
        let _endpoint = component.create_endpoint("mock:fail").unwrap();
        let inner = component.get_endpoint("fail").unwrap();

        inner.assert_exchange_count(5).await;
    }

    #[tokio::test]
    async fn test_mock_component_shared_registry() {
        let component = MockComponent::new();
        let ep1 = component.create_endpoint("mock:shared").unwrap();
        let ep2 = component.create_endpoint("mock:shared").unwrap();

        // Producing via ep1's producer...
        let mut p1 = ep1.create_producer().unwrap();
        p1.call(Exchange::new(Message::new("from-ep1")))
            .await
            .unwrap();

        // ...and via ep2's producer...
        let mut p2 = ep2.create_producer().unwrap();
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
}
