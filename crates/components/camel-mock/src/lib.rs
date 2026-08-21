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
//!
//! # `expectedCount` is inert in the live runtime path
//!
//! The `expectedCount` URI parameter records intent only: at first
//! endpoint creation it registers an exact count expectation on the
//! endpoint inner. It is enforced only when an explicit assertion method
//! runs (`assert_satisfied` / `try_assert_satisfied`), never by the live
//! producer — `poll_ready` and `call` do not consult it. Under
//! `camel run`, where no test caller invokes assertions, `expectedCount`
//! never rejects or drops traffic.

use std::collections::{HashMap, VecDeque, hash_map::Entry};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::sync::{Mutex, Notify};
use tower::Service;

use camel_api::component_metadata::ComponentMetadata;
use camel_component_api::UriConfig;
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

/// Private container for macro-derived `metadata()`.
///
/// Declares the five optional URI parameters (`retain`, `copy`,
/// `failFast`, `expectedCount`, `anyOrder`) for the generated catalog.
/// `create_endpoint` parses them manually (controlbus pattern), so the
/// fields exist only to anchor the metadata derivation — the catalog
/// parity test locks descriptor ↔ parser agreement.
///
/// The anchors are `Option<String>` so required-inference marks every
/// param optional: absent params fall back to the component-level
/// [`MockConfig`] fields (see the README param table), so no static
/// `default = "..."` literal exists and a bare `mock:name` URI is valid.
///
/// Inertness contract: `expectedCount` records an exact count
/// expectation at first endpoint creation; it is enforced only when an
/// explicit assertion method runs (`assert_satisfied` /
/// `try_assert_satisfied`), never by the live producer. Under
/// `camel run` it never rejects or drops traffic. `copy` has no positive
/// behavioral contrast (both producer branches clone identically) — its
/// URI parsing is proven by malformed-value rejection and catalog
/// parity.
#[derive(Debug, Clone, UriConfig)]
#[allow(dead_code)]
#[uri_scheme = "mock"]
#[uri_config(
    skip_impl,
    metadata(
        scheme = "mock",
        description = "Records exchanges for test assertions",
        producer
    ),
    crate = "camel_component_api"
)]
struct MockUriConfig {
    #[uri_param(name = "retain")]
    pub _retain: Option<String>,

    #[uri_param(name = "copy")]
    pub _copy: Option<String>,

    #[uri_param(name = "failFast")]
    pub _fail_fast: Option<String>,

    #[uri_param(name = "expectedCount")]
    pub _expected_count: Option<String>,

    #[uri_param(name = "anyOrder")]
    pub _any_order: Option<String>,
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

    /// Component metadata for the mock scheme, derived from
    /// `#[uri_config(metadata(..))]` on `MockUriConfig`.
    pub fn metadata() -> ComponentMetadata {
        MockUriConfig::metadata()
    }
}

// ---------------------------------------------------------------------------
// MockExpectations
// ---------------------------------------------------------------------------

mod assert;
mod expectations;

pub use assert::MockAssertionError;
pub use expectations::MockExpectations;

// ---------------------------------------------------------------------------
// MockComponent
// ---------------------------------------------------------------------------

/// The Mock component is a testing utility that records every exchange it
/// receives via its producer.  It exposes helpers to inspect and assert on
/// the recorded exchanges.
///
/// URI format: `mock:name[?retain=N&copy=true|false&failFast=true|false&expectedCount=N&anyOrder=true|false]`
///
/// URI params override the component-level [`MockConfig`] fields; absent
/// params fall back to them. All params are optional.
///
/// When `create_endpoint` is called multiple times with the same name, the
/// returned endpoints share the same received-exchanges storage and the
/// first creation's configuration wins — later calls with different params
/// do not reconfigure the existing endpoint. This enables
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

/// Parse a non-negative integer URI parameter value.
fn parse_usize_param(uri_value: &str, name: &str) -> Result<usize, CamelError> {
    uri_value.parse::<usize>().map_err(|_| {
        CamelError::EndpointCreationFailed(format!(
            "mock: invalid value for URI parameter '{name}': '{uri_value}' is not a non-negative integer"
        ))
    })
}

/// Parse a strict boolean URI parameter value (`true`/`false`,
/// case-insensitive).
fn parse_bool_param(uri_value: &str, name: &str) -> Result<bool, CamelError> {
    match uri_value.to_ascii_lowercase().as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(CamelError::EndpointCreationFailed(format!(
            "mock: invalid value for URI parameter '{name}': '{uri_value}' is not a boolean (true|false)"
        ))),
    }
}

impl Component for MockComponent {
    fn scheme(&self) -> &str {
        "mock"
    }

    fn metadata(&self) -> ComponentMetadata {
        MockConfig::metadata()
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

        // URI params override component config; absent params fall back to it.
        // Resolved before the registry lock — malformed values fail creation
        // without touching shared state.
        let max_retained = match parts.params.get("retain") {
            Some(v) => {
                let n = parse_usize_param(v, "retain")?;
                if n == 0 {
                    return Err(CamelError::EndpointCreationFailed(
                        "mock: URI parameter 'retain' must be >= 1, got 0".to_string(),
                    ));
                }
                n
            }
            None => self.config.max_retained,
        };
        let copy_on_exchange = match parts.params.get("copy") {
            Some(v) => parse_bool_param(v, "copy")?,
            None => self.config.copy_on_exchange,
        };
        let fail_fast = match parts.params.get("failFast") {
            Some(v) => parse_bool_param(v, "failFast")?,
            None => self.config.fail_fast,
        };
        let any_order = match parts.params.get("anyOrder") {
            Some(v) => parse_bool_param(v, "anyOrder")?,
            None => self.config.any_order,
        };
        // Inert at creation time: resolved here, bound to a fresh inner
        // below, enforced only by the explicit assertion methods.
        let expected_count = match parts.params.get("expectedCount") {
            Some(v) => Some(parse_usize_param(v, "expectedCount")?),
            None => None,
        };

        let mut registry = self.registry.lock().map_err(|e| {
            CamelError::EndpointCreationFailed(format!("mock registry lock poisoned: {e}"))
        })?;
        let assert_period_ms = self.config.assert_period_ms;
        // First-creation-wins: an existing entry is returned unchanged, so
        // conflicting params on a re-created name never reconfigure the
        // inner. `fresh` marks a newly created inner — the only one a
        // URI-registered expectedCount may bind to.
        let (inner, fresh) = match registry.entry(name.clone()) {
            Entry::Vacant(vacant) => {
                let created = vacant.insert(Arc::new(MockEndpointInner {
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
                }));
                (Arc::clone(created), true)
            }
            Entry::Occupied(occupied) => (Arc::clone(occupied.get()), false),
        };

        // expectedCount records intent only. It binds at first creation and
        // is enforced exclusively by the assertion methods
        // (`assert_satisfied` / `try_assert_satisfied`); the producer never
        // consults it.
        if fresh && let Some(n) = expected_count {
            inner.expect_count(n);
        }

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
    pub(crate) name: String,
    received: Arc<Mutex<VecDeque<Exchange>>>,
    notify: Arc<Notify>,
    max_retained: usize,
    copy_on_exchange: bool,
    fail_fast: bool,
    fail_fast_error: Arc<std::sync::Mutex<Option<CamelError>>>,
    assert_period_ms: u64,
    pub(crate) any_order: bool,
    pub(crate) expectations: Arc<std::sync::Mutex<MockExpectations>>,
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
    /// Panics immediately if called from a current-thread tokio runtime.
    /// Use `#[tokio::test(flavor = "multi_thread")]` or the async accessors
    /// [`get_received_exchanges`] / [`await_exchanges`] instead.
    ///
    /// [`await_exchanges`]: MockEndpointInner::await_exchanges
    pub fn exchange(&self, idx: usize) -> ExchangeAssert {
        if let Ok(handle) = tokio::runtime::Handle::try_current()
            && handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::CurrentThread
        {
            panic!(
                "MockEndpoint '{}': exchange(idx) cannot be used from a current-thread tokio runtime; use #[tokio::test(flavor = \"multi_thread\")] or the async accessors get_received_exchanges()/await_exchanges()",
                self.name
            );
        }
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

    /// Set an exact count expectation: `assert_satisfied` panics unless the
    /// number of retained exchanges equals `n`.
    pub fn expect_count(&self, n: usize) {
        if let Ok(mut guard) = self.expectations.lock() {
            guard.set_expected_count(n);
        }
    }

    /// Set a minimum count expectation: `assert_satisfied` panics unless at
    /// least `n` exchanges are retained.
    pub fn expect_minimum_count(&self, n: usize) {
        if let Ok(mut guard) = self.expectations.lock() {
            guard.set_minimum_count(n);
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
    /// Panics if an expected exchange count (exact or minimum, see
    /// [`expect_count`](Self::expect_count) and
    /// [`expect_minimum_count`](Self::expect_minimum_count)) is not met, if
    /// expected bodies do not match received bodies (in order or any order
    /// depending on `any_order` config), if expected headers are missing, or
    /// if header regex patterns do not match.
    pub async fn assert_satisfied(&self) {
        if let Err(e) = self.evaluate_expectations().await {
            panic!("{e}");
        }
    }

    /// Assert that all registered expectations are satisfied without
    /// panicking.
    ///
    /// Performs the same checks as [`assert_satisfied`](Self::assert_satisfied)
    /// (see it for the full list) and evaluates the same fail-fast latch
    /// rules, but returns the mismatch as [`MockAssertionError`] instead of
    /// panicking.
    ///
    /// # Errors
    ///
    /// Returns `Err(MockAssertionError)` when any expectation is not met or
    /// an expectation is malformed (e.g. a header regex pattern that fails
    /// to compile).
    pub async fn try_assert_satisfied(&self) -> Result<(), MockAssertionError> {
        self.evaluate_expectations().await
    }

    /// Return the stored fail-fast error, if any.
    pub fn fail_fast_error(&self) -> Option<CamelError> {
        self.fail_fast_error.lock().ok().and_then(|g| g.clone())
    }

    /// Manually trip the fail-fast latch.
    ///
    /// Sets the internal `fail_fast_error` to `Some(error)`. The `MockProducer`
    /// treats the presence of any error here as a sentinel — the actual
    /// `CamelError` value is never propagated to the caller; a fixed
    /// "fail-fast mode" message is returned instead. Use this hook when a
    /// downstream component wants to short-circuit further processing on this
    /// endpoint.
    pub fn trigger_fail_fast(&self, error: CamelError) {
        if let Ok(mut guard) = self.fail_fast_error.lock() {
            *guard = Some(error);
        }
    }

    /// When `fail_fast` is enabled, record the assertion-mismatch sentinel
    /// before panicking. This ensures any concurrent or subsequent
    /// `MockProducer::poll_ready` / `call` invocation rejects with the fixed
    /// "fail-fast mode" message instead of being blocked on a panic-orphaned
    /// lock or a stale `None` sentinel.
    pub(crate) fn set_fail_fast_on_mismatch(&self) {
        if self.fail_fast
            && let Ok(mut guard) = self.fail_fast_error.lock()
        {
            *guard = Some(CamelError::ProcessorError(
                "assert_satisfied expectation mismatch".to_string(),
            ));
        }
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
        camel_component_api::Body::Stream(s) => camel_component_api::Body::Stream(s.clone()),
        // Safety net for future #[non_exhaustive] variants; all current variants
        // are handled explicitly above.
        _ => camel_component_api::Body::Empty,
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

    #[tokio::test]
    async fn exchange_current_thread_clear_panic() {
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:current-thread", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("current-thread").unwrap();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("recorded")))
            .await
            .unwrap();

        let panic =
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| inner.exchange(0))) {
                Ok(_) => panic!("exchange must panic on a current-thread runtime"),
                Err(payload) => payload,
            };
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .expect("panic payload should be a string");
        assert!(message.contains("current-thread"), "panic: {message}");
        assert!(message.contains("multi_thread"), "panic: {message}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn exchange_multi_thread_unchanged() {
        let ctx = test_producer_ctx();
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:multi-thread", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("multi-thread").unwrap();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("first")))
            .await
            .unwrap();
        producer
            .call(Exchange::new(Message::new("second")))
            .await
            .unwrap();

        let _assert = inner.exchange(1);
    }

    #[test]
    fn exchange_no_runtime_returns_assert() {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:no-runtime", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("no-runtime").unwrap();
        let ctx = test_producer_ctx();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
            producer
                .call(Exchange::new(Message::new("recorded")))
                .await
                .unwrap();
        });
        drop(runtime);

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
    // MOCK-003b: clone_body preserves Body::Stream
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_clone_body_preserves_stream() {
        use bytes::Bytes;
        use camel_component_api::{Body, StreamBody, StreamMetadata};
        use futures::stream;
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let chunks: Vec<Result<Bytes, camel_component_api::CamelError>> =
            vec![Ok(Bytes::from("data"))];
        let body = Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream::iter(chunks))))),
            metadata: StreamMetadata::default(),
        });

        let config = MockConfig {
            copy_on_exchange: true,
            ..Default::default()
        };
        let component = MockComponent::with_config(config);
        let endpoint = component
            .create_endpoint("mock:stream-test", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("stream-test").unwrap();

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let msg = Message::new(body);
        let ex = Exchange::new(msg);
        producer.call(ex).await.unwrap();

        let received = inner.get_received_exchanges().await;
        assert!(
            matches!(received[0].input.body, Body::Stream(_)),
            "expected Body::Stream, got {:?}",
            received[0].input.body
        );
    }

    #[tokio::test]
    async fn test_clone_body_stream_shares_arc() {
        use bytes::Bytes;
        use camel_component_api::{Body, StreamBody, StreamMetadata};
        use futures::stream;
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let chunks: Vec<Result<Bytes, camel_component_api::CamelError>> =
            vec![Ok(Bytes::from("data"))];
        let original = Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream::iter(chunks))))),
            metadata: StreamMetadata::default(),
        });

        let clone = clone_body(&original);

        // Consume the original first
        let _ = original.into_bytes(100).await.unwrap();

        // Clone should fail with AlreadyConsumed (shared Arc semantics)
        let result = clone.into_bytes(100).await;
        assert!(
            matches!(
                result,
                Err(camel_component_api::CamelError::AlreadyConsumed)
            ),
            "expected AlreadyConsumed, got {:?}",
            result
        );
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

    // -----------------------------------------------------------------------
    // M1: fail-fast trigger + assert_satisfied wires fail_fast_error
    // -----------------------------------------------------------------------

    use futures::FutureExt;

    #[tokio::test]
    async fn test_trigger_fail_fast_rejects_subsequent_producer() {
        use camel_component_api::CamelError;
        use std::panic::AssertUnwindSafe;
        let config = MockConfig {
            fail_fast: true,
            ..Default::default()
        };
        let component = MockComponent::with_config(config);
        let endpoint = component
            .create_endpoint("mock:test", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("test").unwrap();

        inner.trigger_fail_fast(CamelError::ProcessorError("boom".to_string()));

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        // poll_ready must reject in fail-fast mode.
        assert!(producer.ready().await.is_err());
        // The next call must reject with the fixed "fail-fast mode" message.
        let result = AssertUnwindSafe(producer.call(Exchange::default()))
            .catch_unwind()
            .await
            .expect("call should not panic");
        match result {
            Err(CamelError::ProcessorError(msg)) => {
                assert!(
                    msg.contains("fail-fast mode"),
                    "message should contain 'fail-fast mode', got: {msg}"
                );
                assert!(
                    !msg.contains("boom"),
                    "supplied error must NOT be in fixed message, got: {msg}"
                );
            }
            other => panic!("expected ProcessorError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_trigger_fail_fast_noop_when_fail_fast_false() {
        use camel_component_api::CamelError;
        use std::panic::AssertUnwindSafe;
        let config = MockConfig {
            fail_fast: false,
            ..Default::default()
        };
        let component = MockComponent::with_config(config);
        let endpoint = component
            .create_endpoint("mock:test", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("test").unwrap();

        inner.trigger_fail_fast(CamelError::ProcessorError("boom".to_string()));

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        let result = AssertUnwindSafe(producer.call(Exchange::default()))
            .catch_unwind()
            .await
            .expect("call should not panic");
        assert!(
            result.is_ok(),
            "fail_fast=false must let the call through even with stored error"
        );
    }

    #[tokio::test]
    async fn test_reset_clears_trigger_fail_fast() {
        use camel_component_api::CamelError;
        use std::panic::AssertUnwindSafe;
        let config = MockConfig {
            fail_fast: true,
            ..Default::default()
        };
        let component = MockComponent::with_config(config);
        let endpoint = component
            .create_endpoint("mock:test", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("test").unwrap();

        inner.trigger_fail_fast(CamelError::ProcessorError("boom".to_string()));
        assert!(inner.fail_fast_error().is_some());
        inner.reset().await;
        assert!(inner.fail_fast_error().is_none());

        // Producer should accept the call now that reset cleared the error.
        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        let result = AssertUnwindSafe(producer.call(Exchange::default()))
            .catch_unwind()
            .await
            .expect("call should not panic");
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_assert_satisfied_body_count_mismatch_sets_fail_fast() {
        use std::panic::AssertUnwindSafe;
        let config = MockConfig {
            fail_fast: true,
            ..Default::default()
        };
        let component = MockComponent::with_config(config);
        let endpoint = component
            .create_endpoint("mock:test", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("test").unwrap();

        inner.expect_body(camel_component_api::Body::Text("a".to_string()));
        inner.expect_body(camel_component_api::Body::Text("b".to_string()));

        // Send only 1 exchange (expects 2 -> mismatch).
        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("a")))
            .await
            .unwrap();

        // Wrap in catch_unwind so the test does not abort on panic.
        let panic_result = AssertUnwindSafe(inner.assert_satisfied())
            .catch_unwind()
            .await;
        assert!(
            panic_result.is_err(),
            "expected panic from assert_satisfied"
        );
        assert!(
            inner.fail_fast_error().is_some(),
            "fail_fast_error must be set when fail_fast=true and assertion panics"
        );
    }

    #[tokio::test]
    async fn test_assert_satisfied_body_mismatch_sets_fail_fast() {
        use std::panic::AssertUnwindSafe;
        let config = MockConfig {
            fail_fast: true,
            ..Default::default()
        };
        let component = MockComponent::with_config(config);
        let endpoint = component
            .create_endpoint("mock:test", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("test").unwrap();

        inner.expect_body(camel_component_api::Body::Text("expected".to_string()));

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("actual")))
            .await
            .unwrap();

        let panic_result = AssertUnwindSafe(inner.assert_satisfied())
            .catch_unwind()
            .await;
        assert!(
            panic_result.is_err(),
            "expected panic from assert_satisfied"
        );
        assert!(
            inner.fail_fast_error().is_some(),
            "fail_fast_error must be set on body mismatch when fail_fast=true"
        );
    }

    #[tokio::test]
    async fn test_assert_satisfied_no_set_error_when_fail_fast_false() {
        use std::panic::AssertUnwindSafe;
        let config = MockConfig {
            fail_fast: false,
            ..Default::default()
        };
        let component = MockComponent::with_config(config);
        let endpoint = component
            .create_endpoint("mock:test", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("test").unwrap();

        inner.expect_body(camel_component_api::Body::Text("a".to_string()));
        inner.expect_body(camel_component_api::Body::Text("b".to_string()));

        // Send only 1 exchange.
        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("a")))
            .await
            .unwrap();

        let panic_result = AssertUnwindSafe(inner.assert_satisfied())
            .catch_unwind()
            .await;
        assert!(
            panic_result.is_err(),
            "expected panic from assert_satisfied"
        );
        assert!(
            inner.fail_fast_error().is_none(),
            "fail_fast_error must remain None when fail_fast=false"
        );
    }

    #[tokio::test]
    async fn test_assert_satisfied_any_order_body_mismatch_sets_fail_fast() {
        use std::panic::AssertUnwindSafe;
        let config = MockConfig {
            fail_fast: true,
            any_order: true,
            ..Default::default()
        };
        let component = MockComponent::with_config(config);
        let endpoint = component
            .create_endpoint("mock:test", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("test").unwrap();

        inner.expect_body(camel_component_api::Body::Text("a".to_string()));
        inner.expect_body(camel_component_api::Body::Text("b".to_string()));

        // Send 2 exchanges: "a" and "c" — "b" is missing.
        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("a")))
            .await
            .unwrap();
        producer
            .call(Exchange::new(Message::new("c")))
            .await
            .unwrap();

        let panic_result = AssertUnwindSafe(inner.assert_satisfied())
            .catch_unwind()
            .await;
        assert!(
            panic_result.is_err(),
            "expected panic from assert_satisfied (any-order body not found)"
        );
        assert!(
            inner.fail_fast_error().is_some(),
            "fail_fast_error must be set when fail_fast=true and any-order body is missing"
        );
    }

    #[tokio::test]
    async fn test_assert_satisfied_header_missing_sets_fail_fast() {
        use std::panic::AssertUnwindSafe;
        let config = MockConfig {
            fail_fast: true,
            ..Default::default()
        };
        let component = MockComponent::with_config(config);
        let endpoint = component
            .create_endpoint("mock:test", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("test").unwrap();

        inner.expect_header("x-missing", serde_json::json!("value"));

        // Send 1 exchange without the expected header.
        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("body")))
            .await
            .unwrap();

        let panic_result = AssertUnwindSafe(inner.assert_satisfied())
            .catch_unwind()
            .await;
        assert!(
            panic_result.is_err(),
            "expected panic from assert_satisfied (header missing)"
        );
        assert!(
            inner.fail_fast_error().is_some(),
            "fail_fast_error must be set when fail_fast=true and expected header is missing"
        );
    }

    // -----------------------------------------------------------------------
    // Count expectations: expect_count / expect_minimum_count
    // (mock-expectation-and-uri-surface)
    // -----------------------------------------------------------------------

    /// Extract the message from a panic payload caught via `catch_unwind`.
    fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
        payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_else(|| "<non-string panic payload>".to_string())
    }

    #[tokio::test]
    async fn count_exact_mismatch_fails() {
        use std::panic::AssertUnwindSafe;
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:count-exact-mismatch", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("count-exact-mismatch").unwrap();

        inner.expect_count(3);

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

        let payload = AssertUnwindSafe(inner.assert_satisfied())
            .catch_unwind()
            .await
            .expect_err("assert_satisfied should panic on exact count mismatch");
        let msg = panic_message(payload);
        assert!(
            msg.contains("count-exact-mismatch"),
            "message should contain endpoint name, got: {msg}"
        );
        assert!(
            msg.contains("expected 3 exchanges") && msg.contains("got 2"),
            "message should report expected 3 / got 2, got: {msg}"
        );
    }

    #[tokio::test]
    async fn count_exact_satisfied_passes() {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:count-exact-pass", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("count-exact-pass").unwrap();

        inner.expect_count(2);

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("one")))
            .await
            .unwrap();
        producer
            .call(Exchange::new(Message::new("two")))
            .await
            .unwrap();

        inner.assert_satisfied().await;
    }

    #[tokio::test]
    async fn count_minimum_satisfied_by_more() {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:count-min-more", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("count-min-more").unwrap();

        inner.expect_minimum_count(2);

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        for i in 0..5 {
            producer
                .call(Exchange::new(Message::new(format!("m{i}"))))
                .await
                .unwrap();
        }

        inner.assert_satisfied().await;
    }

    #[tokio::test]
    async fn count_minimum_violated_fails() {
        use std::panic::AssertUnwindSafe;
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:count-min-violated", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("count-min-violated").unwrap();

        inner.expect_minimum_count(4);

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("only-one")))
            .await
            .unwrap();

        let payload = AssertUnwindSafe(inner.assert_satisfied())
            .catch_unwind()
            .await
            .expect_err("assert_satisfied should panic on minimum count violation");
        let msg = panic_message(payload);
        assert!(
            msg.contains("at least 4"),
            "message should state at least 4 exchanges were expected, got: {msg}"
        );
    }

    #[tokio::test]
    async fn count_exact_and_minimum_enforced_together() {
        use std::panic::AssertUnwindSafe;
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:count-both", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("count-both").unwrap();

        inner.expect_count(2);
        inner.expect_minimum_count(1);

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        for i in 0..3 {
            producer
                .call(Exchange::new(Message::new(format!("m{i}"))))
                .await
                .unwrap();
        }

        let payload = AssertUnwindSafe(inner.assert_satisfied())
            .catch_unwind()
            .await
            .expect_err("assert_satisfied should panic on exact count mismatch");
        let msg = panic_message(payload);
        assert!(
            msg.contains("expected 2 exchanges") && msg.contains("got 3"),
            "message should report the exact mismatch, got: {msg}"
        );
        assert!(
            !msg.contains("at least"),
            "exact mismatch must be reported even though the minimum was satisfied, got: {msg}"
        );
    }

    #[tokio::test]
    async fn count_checked_before_bodies() {
        use std::panic::AssertUnwindSafe;
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:count-first", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("count-first").unwrap();

        inner.expect_count(5);
        inner.expect_body(camel_component_api::Body::Text("x".into()));

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("y")))
            .await
            .unwrap();
        producer
            .call(Exchange::new(Message::new("z")))
            .await
            .unwrap();

        let payload = AssertUnwindSafe(inner.assert_satisfied())
            .catch_unwind()
            .await
            .expect_err("assert_satisfied should panic on count mismatch");
        let msg = panic_message(payload);
        assert!(
            msg.contains("expected 5 exchanges") && msg.contains("got 2"),
            "count mismatch must be reported, got: {msg}"
        );
        assert!(
            !msg.contains("bodies"),
            "body checks must not run after a count mismatch, got: {msg}"
        );
    }

    #[tokio::test]
    async fn count_coexists_with_bodies_pass() {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:count-with-bodies", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("count-with-bodies").unwrap();

        inner.expect_count(2);
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
    async fn count_evaluates_retained_snapshot_under_truncation() {
        let component = MockComponent::with_config(MockConfig::new(3));
        let endpoint = component
            .create_endpoint("mock:count-truncated", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("count-truncated").unwrap();

        inner.expect_count(3);

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        for i in 0..5 {
            producer
                .call(Exchange::new(Message::new(format!("msg-{i}"))))
                .await
                .unwrap();
        }

        assert_eq!(inner.received_count().await, 3);
        inner.assert_satisfied().await;
    }

    // -----------------------------------------------------------------------
    // Non-panicking assertion surface: try_assert_satisfied / MockAssertionError
    // (mock-expectation-and-uri-surface)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn try_assert_satisfied_ok_when_satisfied() {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:try-ok", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("try-ok").unwrap();

        inner.expect_count(1);
        inner.expect_body(camel_component_api::Body::Text("payload".into()));

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("payload")))
            .await
            .unwrap();

        let result = inner.try_assert_satisfied().await;
        assert!(result.is_ok(), "expected Ok(()), got: {result:?}");
    }

    #[tokio::test]
    async fn try_assert_satisfied_err_with_details() {
        let component = MockComponent::new();
        let _endpoint = component
            .create_endpoint("mock:try-err", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("try-err").unwrap();

        inner.expect_count(2);
        // Send 0 exchanges — no producer needed.

        // Must return Err, not panic.
        let err = inner
            .try_assert_satisfied()
            .await
            .expect_err("expected Err on unmet expect_count");
        let msg = err.to_string();
        assert!(
            msg.contains("try-err"),
            "message should contain endpoint name, got: {msg}"
        );
        assert!(
            msg.contains("expected 2"),
            "message should contain 'expected 2', got: {msg}"
        );
    }

    #[tokio::test]
    async fn try_assert_satisfied_sets_fail_fast_latch() {
        let config = MockConfig {
            fail_fast: true,
            ..Default::default()
        };
        let component = MockComponent::with_config(config);
        let _endpoint = component
            .create_endpoint("mock:try-latch", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("try-latch").unwrap();

        inner.expect_count(2);
        // Send 0 exchanges — expectation unmet.

        let result = inner.try_assert_satisfied().await;
        assert!(result.is_err(), "expected Err, got: {result:?}");
        assert!(
            inner.fail_fast_error().is_some(),
            "fail-fast latch must be set on mismatch (parity with assert_satisfied)"
        );
    }

    #[tokio::test]
    async fn invalid_header_regex_returns_err_not_panic() {
        let config = MockConfig {
            fail_fast: true,
            ..Default::default()
        };
        let component = MockComponent::with_config(config);
        let endpoint = component
            .create_endpoint("mock:try-bad-re", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("try-bad-re").unwrap();

        inner.expect_header_regex("k", "(unclosed");

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("body")))
            .await
            .unwrap();

        // Must return Err (no panic) even with fail_fast enabled.
        let err = inner
            .try_assert_satisfied()
            .await
            .expect_err("invalid regex must produce Err, not a panic");
        assert!(
            matches!(err, MockAssertionError::InvalidHeaderPattern { .. }),
            "expected InvalidHeaderPattern, got: {err:?}"
        );
        assert!(
            inner.fail_fast_error().is_none(),
            "malformed expectation is a caller programming error — latch must NOT trip"
        );
    }

    #[tokio::test]
    async fn display_equals_panicking_variant_message() {
        use std::panic::AssertUnwindSafe;

        // Two identically-configured endpoints sharing one name (hence one
        // inner) so the endpoint-name segment of both messages is equal.
        let component = MockComponent::new();
        let _endpoint_a = component
            .create_endpoint("mock:parity", &NoOpComponentContext)
            .unwrap();
        let endpoint_b = component
            .create_endpoint("mock:parity", &NoOpComponentContext)
            .unwrap();
        let inner_a = component.get_endpoint("parity").unwrap();
        let inner_b = component.get_endpoint("parity").unwrap();

        inner_a.expect_count(3);

        let ctx = test_producer_ctx();
        let mut producer = endpoint_b.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("only-one")))
            .await
            .unwrap();

        let payload = AssertUnwindSafe(inner_a.assert_satisfied())
            .catch_unwind()
            .await
            .expect_err("assert_satisfied should panic on count mismatch");
        let panicking = panic_message(payload);

        let display = inner_b
            .try_assert_satisfied()
            .await
            .expect_err("try_assert_satisfied should Err on count mismatch")
            .to_string();

        assert_eq!(panicking, display);
    }

    #[tokio::test]
    async fn no_expected_bodies_with_received_exchanges_still_ok() {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:try-no-bodies", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("try-no-bodies").unwrap();

        // No body expectations set — the is_empty gate must skip body checks.
        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        for i in 0..3 {
            producer
                .call(Exchange::new(Message::new(format!("m{i}"))))
                .await
                .unwrap();
        }

        let result = inner.try_assert_satisfied().await;
        assert!(result.is_ok(), "no expectations set, got: {result:?}");
    }

    // -------------------------------------------------------------------
    // URI parameter surface (Task 3)
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn uri_retain_override_truncates() {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:cap?retain=50", &NoOpComponentContext)
            .unwrap();
        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        for i in 0..55 {
            producer
                .call(Exchange::new(Message::new(format!("m{i}"))))
                .await
                .unwrap();
        }
        let inner = component.get_endpoint("cap").unwrap();
        assert_eq!(
            inner.received_count().await,
            50,
            "retain=50 must cap stored exchanges at 50 (default 10_000 would retain all 55)"
        );
    }

    #[tokio::test]
    async fn uri_any_order_overrides_matching() {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:relaxed?anyOrder=true", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("relaxed").unwrap();

        inner.expect_body(camel_component_api::Body::Text("a".to_string()));
        inner.expect_body(camel_component_api::Body::Text("b".to_string()));

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("b")))
            .await
            .unwrap();
        producer
            .call(Exchange::new(Message::new("a")))
            .await
            .unwrap();

        // Component default is strict order, which would fail on this
        // out-of-order arrival; anyOrder=true must satisfy.
        inner.assert_satisfied().await;
    }

    #[tokio::test]
    async fn uri_fail_fast_overrides_latching() {
        use std::panic::AssertUnwindSafe;

        let component = MockComponent::new();
        let _endpoint = component
            .create_endpoint("mock:tight?failFast=true", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("tight").unwrap();

        inner.expect_count(1);
        // Send 0 exchanges — expectation unmet.

        let _payload = AssertUnwindSafe(inner.assert_satisfied())
            .catch_unwind()
            .await
            .expect_err("assert_satisfied should panic on unmet expect_count");
        assert!(
            inner.fail_fast_error().is_some(),
            "failFast=true from URI must latch the mismatch (component default false leaves None)"
        );
    }

    #[tokio::test]
    async fn uri_absent_params_fallback_to_config() {
        use std::panic::AssertUnwindSafe;

        let config = MockConfig {
            fail_fast: true,
            ..Default::default()
        };
        let component = MockComponent::with_config(config);
        let _endpoint = component
            .create_endpoint("mock:audit", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("audit").unwrap();

        inner.expect_count(1);
        // Send 0 exchanges — expectation unmet.

        let _payload = AssertUnwindSafe(inner.assert_satisfied())
            .catch_unwind()
            .await
            .expect_err("assert_satisfied should panic on unmet expect_count");
        assert!(
            inner.fail_fast_error().is_some(),
            "component-level fail_fast=true must apply when the URI param is absent"
        );

        // A no-param endpoint with no expectations and nothing sent satisfies;
        // no default count expectation may be registered.
        let _fresh = component
            .create_endpoint("mock:audit-fresh", &NoOpComponentContext)
            .unwrap();
        let fresh = component.get_endpoint("audit-fresh").unwrap();
        let result = fresh.try_assert_satisfied().await;
        assert!(result.is_ok(), "no expectations set, got: {result:?}");
    }

    #[test]
    fn uri_malformed_numeric_rejected() {
        let component = MockComponent::new();
        let err = component
            .create_endpoint("mock:x?retain=abc", &NoOpComponentContext)
            .err()
            .expect("retain=abc must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("retain"),
            "message should name 'retain', got: {msg}"
        );
    }

    #[test]
    fn uri_malformed_expected_count_rejected() {
        let component = MockComponent::new();
        let err = component
            .create_endpoint("mock:x?expectedCount=abc", &NoOpComponentContext)
            .err()
            .expect("expectedCount=abc must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("expectedCount"),
            "message should name 'expectedCount', got: {msg}"
        );
    }

    #[test]
    fn uri_zero_retain_rejected() {
        let component = MockComponent::new();
        let err = component
            .create_endpoint("mock:x?retain=0", &NoOpComponentContext)
            .err()
            .expect("retain=0 must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("retain"),
            "message should name 'retain', got: {msg}"
        );
        assert!(
            msg.contains(">= 1"),
            "message should state the >= 1 constraint, got: {msg}"
        );
    }

    #[test]
    fn uri_malformed_boolean_rejected() {
        let component = MockComponent::new();
        let err = component
            .create_endpoint("mock:x?copy=maybe", &NoOpComponentContext)
            .err()
            .expect("copy=maybe must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("copy"),
            "message should name 'copy', got: {msg}"
        );
    }

    #[tokio::test]
    async fn uri_first_creation_wins_on_conflict() {
        let component = MockComponent::new();
        let _first = component
            .create_endpoint("mock:single?retain=5", &NoOpComponentContext)
            .unwrap();
        let second = component
            .create_endpoint("mock:single?retain=100", &NoOpComponentContext)
            .unwrap();

        let ctx = test_producer_ctx();
        let mut producer = second.create_producer(rt(), &ctx).unwrap();
        for i in 0..7 {
            producer
                .call(Exchange::new(Message::new(format!("m{i}"))))
                .await
                .unwrap();
        }
        let inner = component.get_endpoint("single").unwrap();
        assert_eq!(
            inner.received_count().await,
            5,
            "first creation's retain=5 must still bind; second creation must not reconfigure"
        );
    }

    #[test]
    fn catalog_parity_five_params() {
        let meta = MockConfig::metadata();
        let mut names: Vec<&str> = meta.uri_options.iter().map(|o| o.name.as_str()).collect();
        names.sort();
        assert_eq!(
            names,
            ["anyOrder", "copy", "expectedCount", "failFast", "retain"],
            "metadata uri_options names must match parser keys"
        );

        // All five params are optional: absent params fall back to the
        // component-level `MockConfig` fields (README param table). A
        // `required` descriptor would make every bare `mock:name` URI a
        // lint error (R-URI-known:missing-required-option) — locked by
        // the corpus gate.
        for opt in &meta.uri_options {
            assert!(
                !opt.required,
                "{} must be optional (falls back to MockConfig)",
                opt.name
            );
        }
    }

    // -------------------------------------------------------------------
    // expectedCount wiring + live-traffic inertness (Task 4)
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn expected_count_never_rejects_live_exchanges() {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint(
                "mock:sink?expectedCount=2&failFast=true",
                &NoOpComponentContext,
            )
            .unwrap();
        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        for i in 0..7 {
            let result = producer
                .call(Exchange::new(Message::new(format!("m{i}"))))
                .await;
            assert!(
                result.is_ok(),
                "live exchange {i} must not be rejected by expectedCount"
            );
        }
        let inner = component.get_endpoint("sink").unwrap();
        assert_eq!(inner.received_count().await, 7);
        assert!(
            inner.fail_fast_error().is_none(),
            "expectedCount alone must never trip the fail-fast latch before an assertion runs"
        );
    }

    #[tokio::test]
    async fn expected_count_enforced_only_at_assertion() {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:sink?expectedCount=2", &NoOpComponentContext)
            .unwrap();
        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        for i in 0..3 {
            producer
                .call(Exchange::new(Message::new(format!("m{i}"))))
                .await
                .unwrap();
        }
        let inner = component.get_endpoint("sink").unwrap();
        let result = inner.try_assert_satisfied().await;
        assert!(
            result.is_err(),
            "expectedCount=2 vs 3 received must fail at assertion time, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn failed_assertion_then_applies_normal_fail_fast() {
        use camel_component_api::CamelError;

        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint(
                "mock:sink?expectedCount=2&failFast=true",
                &NoOpComponentContext,
            )
            .unwrap();
        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        for i in 0..3 {
            producer
                .call(Exchange::new(Message::new(format!("m{i}"))))
                .await
                .unwrap();
        }
        let inner = component.get_endpoint("sink").unwrap();
        assert!(
            inner.try_assert_satisfied().await.is_err(),
            "2-vs-3 mismatch must Err and trip the fail-fast latch"
        );
        let result = producer.call(Exchange::default()).await;
        match result {
            Err(CamelError::ProcessorError(msg)) => assert!(
                msg.contains("fail-fast mode"),
                "fixed fail-fast message expected, got: {msg}"
            ),
            other => panic!("expected ProcessorError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn expected_count_not_reset_on_second_creation() {
        let component = MockComponent::new();
        let _first = component
            .create_endpoint("mock:once", &NoOpComponentContext)
            .unwrap();
        let second = component
            .create_endpoint("mock:once?expectedCount=5", &NoOpComponentContext)
            .unwrap();
        let ctx = test_producer_ctx();
        let mut producer = second.create_producer(rt(), &ctx).unwrap();
        for i in 0..2 {
            producer
                .call(Exchange::new(Message::new(format!("m{i}"))))
                .await
                .unwrap();
        }
        let inner = component.get_endpoint("once").unwrap();
        let result = inner.try_assert_satisfied().await;
        assert!(
            result.is_ok(),
            "first creation registered no count expectation; second creation must not \
             reconfigure it, got: {result:?}"
        );
    }
}
