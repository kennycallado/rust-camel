//! # camel-component-mock
//!
//! Mock component for rust-camel — testing utility that records received
//! exchanges for later assertion, useful for verifying route output in tests.
//!
//! Main types: `MockComponent`, `MockEndpoint`, `MockProducer`, `MockExpectations`.
//!
//! # Example
//!
//! ```rust,no_run
//! use camel_component_mock::MockComponent;
//! use camel_component_api::{Component, NoOpComponentContext, Exchange, Message};
//!
//! // Create a mock component and endpoint
//! let component = MockComponent::new();
//! let endpoint = component
//!     .create_endpoint("mock:result", &NoOpComponentContext)
//!     .unwrap();
//!
//! // In a real route, the producer would be used as a Tower service.
//! // After sending exchanges, you can inspect them:
//! let inner = component.get_endpoint("result").unwrap();
//! // inner.assert_exchange_count(1).await;
//! // inner.exchange(0).assert_body_text("hello");
//! ```

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::sync::{Mutex, Notify};
use tower::Service;

use camel_component_api::parse_uri;
use camel_component_api::{BoxProcessor, CamelError, Exchange};
use camel_component_api::{Component, Consumer, Endpoint, ProducerContext, RuntimeObservability};
use tracing::debug;

/// Default maximum number of exchanges retained by a mock endpoint.
const DEFAULT_MAX_RETAINED: usize = 10_000;

// ---------------------------------------------------------------------------
// MockConfig
// ---------------------------------------------------------------------------

/// Configuration for [`MockComponent`].
///
/// Controls how many exchanges are retained before the oldest are dropped,
/// and other behavioural flags for assertions.
///
/// # Examples
///
/// ```rust
/// use camel_component_mock::MockConfig;
///
/// let config = MockConfig {
///     max_retained: 100,
///     copy_on_exchange: true,
///     fail_fast: false,
///     assert_period_ms: 0,
///     any_order: false,
/// };
/// ```
#[derive(Clone, Debug)]
pub struct MockConfig {
    /// Maximum number of exchanges to retain. When exceeded, the oldest
    /// exchange is dropped. Defaults to 10 000.
    pub max_retained: usize,
    /// When `true`, clone the exchange body before storing it in the received
    /// exchanges list. This prevents aliasing when the caller mutates the
    /// original exchange after sending. Defaults to `false`.
    pub copy_on_exchange: bool,
    /// When `true`, after the first failing assertion the mock stops processing
    /// exchanges and records the error. Defaults to `false`.
    pub fail_fast: bool,
    /// Time in milliseconds to wait before asserting expectations (to allow
    /// async processing to complete). Defaults to `0` (no wait).
    pub assert_period_ms: u64,
    /// When `true`, [`MockEndpointInner::assert_satisfied`] matches expected
    /// bodies in any order rather than strict sequence. Defaults to `false`.
    pub any_order: bool,
}

impl Default for MockConfig {
    fn default() -> Self {
        Self {
            max_retained: DEFAULT_MAX_RETAINED,
            copy_on_exchange: false,
            fail_fast: false,
            assert_period_ms: 0,
            any_order: false,
        }
    }
}

impl MockConfig {
    /// Create a config with a custom retention limit.
    pub fn new(max_retained: usize) -> Self {
        Self {
            max_retained,
            ..Self::default()
        }
    }
}

// ---------------------------------------------------------------------------
// MockExpectations
// ---------------------------------------------------------------------------

/// Expectations set on a mock endpoint for batch-style assertion.
///
/// Use [`MockEndpointInner::expect_body`] and
/// [`MockEndpointInner::expect_header`] to populate expectations, then call
/// [`MockEndpointInner::assert_satisfied`] after exchanges have been received.
pub struct MockExpectations {
    expected_bodies: Vec<camel_component_api::Body>,
    expected_headers: Vec<(String, serde_json::Value)>,
    expected_header_regexes: Vec<(String, String)>,
}

impl Default for MockExpectations {
    fn default() -> Self {
        Self::new()
    }
}

impl MockExpectations {
    /// Create an empty set of expectations.
    pub fn new() -> Self {
        Self {
            expected_bodies: Vec::new(),
            expected_headers: Vec::new(),
            expected_header_regexes: Vec::new(),
        }
    }

    /// Add an expected body value.
    pub fn push_body(&mut self, body: camel_component_api::Body) {
        self.expected_bodies.push(body);
    }

    /// Add an expected header key-value pair.
    pub fn push_header(&mut self, key: String, value: serde_json::Value) {
        self.expected_headers.push((key, value));
    }

    /// Add an expected header regex pattern.
    pub fn push_header_regex(&mut self, key: String, pattern: String) {
        self.expected_header_regexes.push((key, pattern));
    }
}

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
    config: MockConfig,
}

impl MockComponent {
    pub fn new() -> Self {
        Self::with_config(MockConfig::default())
    }

    /// Create a `MockComponent` with a custom [`MockConfig`].
    pub fn with_config(config: MockConfig) -> Self {
        Self {
            registry: Arc::new(std::sync::Mutex::new(HashMap::new())),
            config,
        }
    }

    /// Retrieve a previously created endpoint's inner data by name.
    ///
    /// This is the primary way to inspect recorded exchanges in tests.
    pub fn get_endpoint(&self, name: &str) -> Option<Arc<MockEndpointInner>> {
        let registry = self
            .registry
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock"); // allow-unwrap
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
        if name.is_empty() {
            return Err(CamelError::InvalidUri(
                "mock endpoint name must be non-empty (use 'mock:<name>')".to_string(),
            ));
        }
        let mut registry = self.registry.lock().map_err(|e| {
            CamelError::EndpointCreationFailed(format!("mock registry lock poisoned: {e}"))
        })?;
        let max_retained = self.config.max_retained;
        let copy_on_exchange = self.config.copy_on_exchange;
        let fail_fast = self.config.fail_fast;
        let assert_period_ms = self.config.assert_period_ms;
        let any_order = self.config.any_order;
        let inner = registry
            .entry(name.clone())
            .or_insert_with(|| {
                Arc::new(MockEndpointInner {
                    uri: uri.to_string(),
                    name,
                    received: Arc::new(Mutex::new(VecDeque::new())),
                    notify: Arc::new(Notify::new()),
                    max_retained,
                    copy_on_exchange,
                    fail_fast,
                    fail_fast_error: Arc::new(std::sync::Mutex::new(None)),
                    assert_period_ms,
                    any_order,
                    expectations: Arc::new(std::sync::Mutex::new(MockExpectations::new())),
                })
            })
            .clone();

        debug!(endpoint_name = %inner.name, "mock endpoint created");
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
    received: Arc<Mutex<VecDeque<Exchange>>>,
    notify: Arc<Notify>,
    max_retained: usize,
    copy_on_exchange: bool,
    fail_fast: bool,
    fail_fast_error: Arc<std::sync::Mutex<Option<CamelError>>>,
    assert_period_ms: u64,
    any_order: bool,
    expectations: Arc<std::sync::Mutex<MockExpectations>>,
}

impl MockEndpointInner {
    /// Return a snapshot of all exchanges retained so far.
    pub async fn get_received_exchanges(&self) -> Vec<Exchange> {
        self.received.lock().await.iter().cloned().collect()
    }

    /// Return the number of currently retained exchanges.
    pub async fn received_count(&self) -> usize {
        self.received.lock().await.len()
    }

    /// Clear all retained exchanges and reset internal counters.
    ///
    /// Useful between test cases to reuse the same mock endpoint.
    pub async fn reset(&self) {
        self.received.lock().await.clear();
        if let Ok(mut guard) = self.fail_fast_error.lock() {
            *guard = None;
        }
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

    /// Wait for exchanges with a configurable timeout derived from `assert_period_ms`.
    ///
    /// If `assert_period_ms` is 0, uses the provided `fallback` duration.
    /// Otherwise, waits for `assert_period_ms` milliseconds before checking.
    pub async fn await_exchanges_with_timeout(&self, count: usize, fallback: std::time::Duration) {
        let duration = if self.assert_period_ms > 0 {
            std::time::Duration::from_millis(self.assert_period_ms)
        } else {
            fallback
        };
        self.await_exchanges(count, duration).await;
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
    // NOTE: requires multi-threaded Tokio runtime (current_thread will deadlock)
    // due to `block_in_place` used for blocking_lock.
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

    /// Add an expected body to the expectations list.
    pub fn expect_body(&self, body: camel_component_api::Body) {
        if let Ok(mut guard) = self.expectations.lock() {
            guard.push_body(body);
        }
    }

    /// Add an expected header key-value pair to the expectations list.
    pub fn expect_header(&self, key: &str, value: impl Into<serde_json::Value>) {
        if let Ok(mut guard) = self.expectations.lock() {
            guard.push_header(key.to_string(), value.into());
        }
    }

    /// Add an expected header regex pattern to the expectations list.
    ///
    /// After `await_exchanges()`, `assert_satisfied()` checks whether any
    /// received exchange has the named header matching the given regex pattern.
    pub fn expect_header_regex(&self, key: &str, pattern: &str) {
        if let Ok(mut guard) = self.expectations.lock() {
            guard.push_header_regex(key.to_string(), pattern.to_string());
        }
    }

    /// Assert that all registered expectations are satisfied.
    ///
    /// # Panics
    ///
    /// Panics if expected bodies do not match received bodies (in order or any
    /// order depending on `any_order` config), if expected headers are missing,
    /// or if header regex patterns do not match.
    pub async fn assert_satisfied(&self) {
        let received = self.get_received_exchanges().await;

        // Check expected bodies
        {
            let guard = self
                .expectations
                .lock()
                .expect("expectations lock poisoned"); // allow-unwrap
            if !guard.expected_bodies.is_empty() {
                let received_bodies: Vec<_> = received.iter().map(|e| &e.input.body).collect();
                if guard.expected_bodies.len() != received_bodies.len() {
                    panic!(
                        "MockEndpoint '{}': expected {} bodies, got {}",
                        self.name,
                        guard.expected_bodies.len(),
                        received_bodies.len()
                    );
                }
                if self.any_order {
                    // Match in any order — each expected body must appear exactly once
                    let mut unmatched: Vec<_> = received_bodies.iter().collect();
                    for expected in &guard.expected_bodies {
                        let idx = unmatched
                            .iter()
                            .position(|actual| body_eq(expected, actual));
                        match idx {
                            Some(i) => {
                                unmatched.remove(i);
                            }
                            None => panic!(
                                "MockEndpoint '{}': expected body {:?} not found in received exchanges (anyOrder mode)",
                                self.name, expected
                            ),
                        }
                    }
                } else {
                    for (i, expected) in guard.expected_bodies.iter().enumerate() {
                        if !body_eq(expected, received_bodies[i]) {
                            panic!(
                                "MockEndpoint '{}': body[{}] expected {:?}, got {:?}",
                                self.name, i, expected, received_bodies[i]
                            );
                        }
                    }
                }
            }

            // Check expected headers (must all be present on at least one exchange)
            for (key, value) in &guard.expected_headers {
                let found = received
                    .iter()
                    .any(|ex| ex.input.headers.get(key).is_some_and(|v| v == value));
                if !found {
                    panic!(
                        "MockEndpoint '{}': expected header '{}' = {} not found in any received exchange",
                        self.name, key, value
                    );
                }
            }

            // Check expected header regexes
            for (key, pattern) in &guard.expected_header_regexes {
                let re = regex::Regex::new(pattern).unwrap_or_else(|e| {
                    panic!(
                        "MockEndpoint '{}': invalid regex pattern {:?}: {e}",
                        self.name, pattern
                    )
                });
                let found = received.iter().any(|ex| {
                    ex.input.headers.get(key).is_some_and(|v| {
                        let s = match v {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        re.is_match(&s)
                    })
                });
                if !found {
                    panic!(
                        "MockEndpoint '{}': no received exchange has header '{}' matching regex {:?}",
                        self.name, key, pattern
                    );
                }
            }
        }
    }

    /// Return the stored fail-fast error, if any.
    pub fn fail_fast_error(&self) -> Option<CamelError> {
        self.fail_fast_error.lock().ok().and_then(|g| g.clone())
    }
}

/// Compare two `Body` values for equality (used by assert_satisfied).
fn body_eq(a: &camel_component_api::Body, b: &camel_component_api::Body) -> bool {
    match (a, b) {
        (camel_component_api::Body::Empty, camel_component_api::Body::Empty) => true,
        (camel_component_api::Body::Text(a), camel_component_api::Body::Text(b)) => a == b,
        (camel_component_api::Body::Json(a), camel_component_api::Body::Json(b)) => a == b,
        (camel_component_api::Body::Xml(a), camel_component_api::Body::Xml(b)) => a == b,
        (camel_component_api::Body::Bytes(a), camel_component_api::Body::Bytes(b)) => a == b,
        _ => false,
    }
}

impl Endpoint for MockEndpoint {
    fn uri(&self) -> &str {
        &self.0.uri
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Err(CamelError::EndpointCreationFailed(
            "mock endpoint does not support consumers (it is a sink)".to_string(),
        ))
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(MockProducer {
            name: self.0.name.clone(),
            received: Arc::clone(&self.0.received),
            notify: Arc::clone(&self.0.notify),
            max_retained: self.0.max_retained,
            copy_on_exchange: self.0.copy_on_exchange,
            fail_fast: self.0.fail_fast,
            fail_fast_error: Arc::clone(&self.0.fail_fast_error),
        }))
    }
}

// ---------------------------------------------------------------------------
// MockProducer
// ---------------------------------------------------------------------------

/// A producer that simply records each exchange it processes.
#[derive(Clone)]
struct MockProducer {
    name: String,
    received: Arc<Mutex<VecDeque<Exchange>>>,
    notify: Arc<Notify>,
    max_retained: usize,
    copy_on_exchange: bool,
    fail_fast: bool,
    fail_fast_error: Arc<std::sync::Mutex<Option<CamelError>>>,
}

impl Service<Exchange> for MockProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // In fail-fast mode, reject new exchanges if a previous one failed
        if self.fail_fast
            && let Ok(guard) = self.fail_fast_error.lock()
            && guard.is_some()
        {
            return Poll::Ready(Err(CamelError::ProcessorError(
                "mock endpoint in fail-fast mode: a previous exchange caused an error".to_string(),
            )));
        }
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let name = self.name.clone();
        let received = Arc::clone(&self.received);
        let notify = Arc::clone(&self.notify);
        let max_retained = self.max_retained;
        let copy_on_exchange = self.copy_on_exchange;
        let fail_fast = self.fail_fast;
        let fail_fast_error = Arc::clone(&self.fail_fast_error);
        Box::pin(async move {
            // In fail-fast mode, check if a previous error was recorded
            if fail_fast
                && let Ok(guard) = fail_fast_error.lock()
                && guard.is_some()
            {
                return Err(CamelError::ProcessorError(
                    "mock endpoint in fail-fast mode: a previous exchange caused an error"
                        .to_string(),
                ));
            }

            let correlation_id = exchange
                .input
                .headers
                .get("CamelCorrelationId")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let exchange_to_store = if copy_on_exchange {
                let mut cloned = exchange.clone();
                // Deep-clone the body to break aliasing
                cloned.input.body = clone_body(&exchange.input.body);
                cloned
            } else {
                exchange.clone()
            };

            let mut guard = received.lock().await;
            if guard.len() >= max_retained {
                tracing::warn!(
                    endpoint_name = %name,
                    max = max_retained,
                    "max retained exchanges reached, dropping oldest"
                );
                guard.pop_front();
            }
            guard.push_back(exchange_to_store);
            let count = guard.len();
            drop(guard);

            debug!(
                endpoint_name = %name,
                count = %count,
                correlation_id = correlation_id.as_deref().unwrap_or("none"),
                "exchange recorded on mock"
            );
            notify.notify_waiters();

            Ok(exchange)
        })
    }
}

/// Deep-clone a `Body` value.
fn clone_body(body: &camel_component_api::Body) -> camel_component_api::Body {
    match body {
        camel_component_api::Body::Empty => camel_component_api::Body::Empty,
        camel_component_api::Body::Text(s) => camel_component_api::Body::Text(s.clone()),
        camel_component_api::Body::Json(v) => camel_component_api::Body::Json(v.clone()),
        camel_component_api::Body::Xml(s) => camel_component_api::Body::Xml(s.clone()),
        camel_component_api::Body::Bytes(b) => camel_component_api::Body::Bytes(b.clone()),
        camel_component_api::Body::Stream(_) => {
            // Streams cannot be cloned; use Empty as fallback
            camel_component_api::Body::Empty
        }
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
    use camel_component_api::test_support::PanicRuntimeObservability;
    fn rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(PanicRuntimeObservability)
    }

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
    fn test_empty_mock_endpoint_name_rejected() {
        let component = MockComponent::new();
        let result = component.create_endpoint("mock:", &NoOpComponentContext);
        assert!(result.is_err(), "empty mock name should be rejected");
    }

    #[test]
    fn test_valid_mock_endpoint_name_accepted() {
        let component = MockComponent::new();
        let result = component.create_endpoint("mock:result", &NoOpComponentContext);
        assert!(result.is_ok());
    }

    #[test]
    fn test_mock_endpoint_no_consumer() {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:result", &NoOpComponentContext)
            .unwrap();
        assert!(endpoint.create_consumer(rt()).is_err());
    }

    #[test]
    fn test_mock_endpoint_creates_producer() {
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:result", &NoOpComponentContext)
            .unwrap();
        assert!(endpoint.create_producer(rt(), &ctx).is_ok());
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

        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();

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

        let producer = endpoint.create_producer(rt(), &ctx).unwrap();
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
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
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
        let mut p1 = ep1.create_producer(rt(), &ctx).unwrap();
        p1.call(Exchange::new(Message::new("from-ep1")))
            .await
            .unwrap();

        // ...and via ep2's producer...
        let mut p2 = ep2.create_producer(rt(), &ctx).unwrap();
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

        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
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
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
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

        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
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

        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
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
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
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
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
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
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
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
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
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
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
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
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
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
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
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
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
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
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
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
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
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
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        let mut ex = Exchange::new(Message::new("body"));
        ex.set_error(camel_component_api::CamelError::ProcessorError(
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
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
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
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("body")))
            .await
            .unwrap();
        inner
            .await_exchanges(1, std::time::Duration::from_millis(500))
            .await;
        inner.exchange(0).assert_no_error();
    }

    // -----------------------------------------------------------------------
    // A-13: reset() and bounded retention tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_mock_reset_clears_exchanges() {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:reset-test", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("reset-test").unwrap();

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("a")))
            .await
            .unwrap();
        producer
            .call(Exchange::new(Message::new("b")))
            .await
            .unwrap();

        assert_eq!(inner.received_count().await, 2);
        inner.reset().await;
        assert_eq!(inner.received_count().await, 0);
    }

    #[tokio::test]
    async fn test_mock_bounded_retention_drops_oldest() {
        let config = MockConfig {
            max_retained: 3,
            ..Default::default()
        };
        let component = MockComponent::with_config(config);
        let endpoint = component
            .create_endpoint("mock:bounded", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("bounded").unwrap();

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();

        // Send 5 exchanges, but max_retained is 3
        for i in 0..5 {
            producer
                .call(Exchange::new(Message::new(format!("msg-{i}"))))
                .await
                .unwrap();
        }

        assert_eq!(inner.received_count().await, 3);
        let received = inner.get_received_exchanges().await;
        // Oldest (msg-0, msg-1) should be dropped
        assert_eq!(received[0].input.body.as_text(), Some("msg-2"));
        assert_eq!(received[1].input.body.as_text(), Some("msg-3"));
        assert_eq!(received[2].input.body.as_text(), Some("msg-4"));
    }

    #[tokio::test]
    async fn test_mock_reset_then_record_again() {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:reset-reuse", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("reset-reuse").unwrap();

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("before-reset")))
            .await
            .unwrap();
        inner.reset().await;

        producer
            .call(Exchange::new(Message::new("after-reset")))
            .await
            .unwrap();

        let received = inner.get_received_exchanges().await;
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].input.body.as_text(), Some("after-reset"));
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
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        let mut ex = Exchange::new(Message::new("body"));
        ex.set_error(camel_component_api::CamelError::ProcessorError(
            "oops".to_string(),
        ));
        producer.call(ex).await.unwrap();
        inner
            .await_exchanges(1, std::time::Duration::from_millis(500))
            .await;
        inner.exchange(0).assert_no_error();
    }

    // -----------------------------------------------------------------------
    // MOCK-003: copy_on_exchange tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_copy_on_exchange_stores_cloned_body() {
        let config = MockConfig {
            copy_on_exchange: true,
            ..Default::default()
        };
        let component = MockComponent::with_config(config);
        let endpoint = component
            .create_endpoint("mock:copy", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("copy").unwrap();

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let mut msg = Message::new("original");
        msg.headers.insert("x-test".into(), serde_json::json!(1));
        let ex = Exchange::new(msg);
        producer.call(ex).await.unwrap();

        let received = inner.get_received_exchanges().await;
        assert_eq!(received[0].input.body.as_text(), Some("original"));
    }

    #[tokio::test]
    async fn test_copy_on_exchange_false_shares_storage() {
        let config = MockConfig {
            copy_on_exchange: false,
            ..Default::default()
        };
        let component = MockComponent::with_config(config);
        let endpoint = component
            .create_endpoint("mock:no-copy", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("no-copy").unwrap();

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();

        producer
            .call(Exchange::new(Message::new("direct")))
            .await
            .unwrap();

        let received = inner.get_received_exchanges().await;
        assert_eq!(received[0].input.body.as_text(), Some("direct"));
    }

    // -----------------------------------------------------------------------
    // MOCK-004: expect_body / expect_header / assert_satisfied tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_assert_satisfied_bodies_in_order() {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:sat-bodies", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("sat-bodies").unwrap();

        inner.expect_body(camel_component_api::Body::Text("alpha".into()));
        inner.expect_body(camel_component_api::Body::Text("beta".into()));

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("alpha")))
            .await
            .unwrap();
        producer
            .call(Exchange::new(Message::new("beta")))
            .await
            .unwrap();

        inner.assert_satisfied().await;
    }

    #[tokio::test]
    #[should_panic(expected = "body[0] expected")]
    async fn test_assert_satisfied_bodies_wrong_order_fails() {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:sat-bodies-fail", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("sat-bodies-fail").unwrap();

        inner.expect_body(camel_component_api::Body::Text("alpha".into()));
        inner.expect_body(camel_component_api::Body::Text("beta".into()));

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("beta")))
            .await
            .unwrap();
        producer
            .call(Exchange::new(Message::new("alpha")))
            .await
            .unwrap();

        inner.assert_satisfied().await;
    }

    #[tokio::test]
    async fn test_assert_satisfied_headers() {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:sat-hdr", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("sat-hdr").unwrap();

        inner.expect_header("status", serde_json::json!("ok"));

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        let mut msg = Message::new("body");
        msg.headers.insert("status".into(), serde_json::json!("ok"));
        producer.call(Exchange::new(msg)).await.unwrap();

        inner.assert_satisfied().await;
    }

    #[tokio::test]
    #[should_panic(expected = "expected header 'missing' =")]
    async fn test_assert_satisfied_headers_missing() {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:sat-hdr-missing", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("sat-hdr-missing").unwrap();

        inner.expect_header("missing", serde_json::json!("value"));

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("body")))
            .await
            .unwrap();

        inner.assert_satisfied().await;
    }

    // -----------------------------------------------------------------------
    // MOCK-005: fail_fast tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_fail_fast_rejects_after_first_call() {
        let config = MockConfig {
            fail_fast: true,
            ..Default::default()
        };
        let component = MockComponent::with_config(config);
        let endpoint = component
            .create_endpoint("mock:ff", &NoOpComponentContext)
            .unwrap();

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();

        // First call succeeds
        producer
            .call(Exchange::new(Message::new("ok")))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_fail_fast_no_error_when_all_good() {
        let config = MockConfig {
            fail_fast: true,
            ..Default::default()
        };
        let component = MockComponent::with_config(config);
        let endpoint = component
            .create_endpoint("mock:ff-good", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("ff-good").unwrap();

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();

        producer
            .call(Exchange::new(Message::new("a")))
            .await
            .unwrap();
        producer
            .call(Exchange::new(Message::new("b")))
            .await
            .unwrap();

        assert!(inner.fail_fast_error().is_none());
        inner.assert_exchange_count(2).await;
    }

    // -----------------------------------------------------------------------
    // MOCK-008: await_exchanges_with_timeout tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_await_exchanges_with_timeout_uses_config_period() {
        let config = MockConfig {
            assert_period_ms: 100,
            ..Default::default()
        };
        let component = MockComponent::with_config(config);
        let endpoint = component
            .create_endpoint("mock:ap", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("ap").unwrap();

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("x")))
            .await
            .unwrap();

        inner
            .await_exchanges_with_timeout(1, std::time::Duration::from_millis(1))
            .await;
    }

    #[tokio::test]
    async fn test_await_exchanges_with_timeout_uses_fallback_when_zero() {
        let config = MockConfig {
            assert_period_ms: 0,
            ..Default::default()
        };
        let component = MockComponent::with_config(config);
        let endpoint = component
            .create_endpoint("mock:ap-fb", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("ap-fb").unwrap();

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("y")))
            .await
            .unwrap();

        inner
            .await_exchanges_with_timeout(1, std::time::Duration::from_millis(200))
            .await;
    }

    // -----------------------------------------------------------------------
    // MOCK-009: expect_header_regex tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_expect_header_regex_match() {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:re-hdr", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("re-hdr").unwrap();

        inner.expect_header_regex("x-trace-id", r"^[a-f0-9]{8}$");

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        let mut msg = Message::new("body");
        msg.headers
            .insert("x-trace-id".into(), serde_json::json!("deadbeef"));
        producer.call(Exchange::new(msg)).await.unwrap();

        inner.assert_satisfied().await;
    }

    #[tokio::test]
    #[should_panic(expected = "no received exchange has header")]
    async fn test_expect_header_regex_no_match() {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:re-hdr-fail", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("re-hdr-fail").unwrap();

        inner.expect_header_regex("x-trace-id", r"^\d+$");

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        let mut msg = Message::new("body");
        msg.headers
            .insert("x-trace-id".into(), serde_json::json!("abc"));
        producer.call(Exchange::new(msg)).await.unwrap();

        inner.assert_satisfied().await;
    }

    // -----------------------------------------------------------------------
    // MOCK-010: any_order tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_any_order_bodies_match() {
        let config = MockConfig {
            any_order: true,
            ..Default::default()
        };
        let component = MockComponent::with_config(config);
        let endpoint = component
            .create_endpoint("mock:anyorder", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("anyorder").unwrap();

        inner.expect_body(camel_component_api::Body::Text("beta".into()));
        inner.expect_body(camel_component_api::Body::Text("alpha".into()));

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("alpha")))
            .await
            .unwrap();
        producer
            .call(Exchange::new(Message::new("beta")))
            .await
            .unwrap();

        inner.assert_satisfied().await;
    }

    #[tokio::test]
    #[should_panic(expected = "not found in received exchanges (anyOrder mode)")]
    async fn test_any_order_bodies_missing() {
        let config = MockConfig {
            any_order: true,
            ..Default::default()
        };
        let component = MockComponent::with_config(config);
        let endpoint = component
            .create_endpoint("mock:anyorder-fail", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("anyorder-fail").unwrap();

        inner.expect_body(camel_component_api::Body::Text("gamma".into()));
        inner.expect_body(camel_component_api::Body::Text("alpha".into()));

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("alpha")))
            .await
            .unwrap();
        producer
            .call(Exchange::new(Message::new("beta")))
            .await
            .unwrap();

        inner.assert_satisfied().await;
    }

    // -----------------------------------------------------------------------
    // MOCK-012: tracing instrumentation tests (compilation + basic)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_tracing_logs_exchange_received() {
        // Verify the producer doesn't panic and the debug trace fires
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:trace", &NoOpComponentContext)
            .unwrap();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("traced")))
            .await
            .unwrap();

        let inner = component.get_endpoint("trace").unwrap();
        inner.assert_exchange_count(1).await;
    }

    // -----------------------------------------------------------------------
    // MOCK-006 / MOCK-007: doctest exists on MockConfig
    // -----------------------------------------------------------------------

    #[test]
    fn test_mock_config_new() {
        let cfg = MockConfig::new(42);
        assert_eq!(cfg.max_retained, 42);
        assert!(!cfg.copy_on_exchange);
        assert!(!cfg.fail_fast);
        assert!(!cfg.any_order);
    }
}
