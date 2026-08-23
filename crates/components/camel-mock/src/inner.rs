//! Endpoint internals for the mock component.
//!
//! Holds the shared per-endpoint state ([`MockEndpointInner`]), the thin
//! [`MockEndpoint`] wrapper, the recording [`MockProducer`], and the
//! synchronous [`ExchangeAssert`] handle. These types are re-exported from the
//! crate root; the public API is unchanged.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::sync::{Mutex, Notify};
use tower::Service;

use camel_component_api::{BoxProcessor, CamelError, Exchange};
use camel_component_api::{Consumer, Endpoint, ProducerContext, RuntimeObservability};
use tracing::debug;

use crate::MockAssertionError;
use crate::MockExpectations;

// ---------------------------------------------------------------------------
// MockEndpoint / MockEndpointInner
// ---------------------------------------------------------------------------

/// A mock endpoint that records all exchanges sent to it.
///
/// This is a thin wrapper around `Arc<MockEndpointInner>`. Multiple
/// `MockEndpoint` instances created with the same name share the same inner
/// storage.
pub struct MockEndpoint(pub(crate) Arc<MockEndpointInner>);

/// The actual data behind a mock endpoint. Shared across all `MockEndpoint`
/// instances created with the same name via `MockComponent`.
///
/// Use `get_received_exchanges` and `assert_exchange_count` to inspect
/// recorded exchanges in tests.
pub struct MockEndpointInner {
    pub(crate) uri: String,
    pub(crate) name: String,
    pub(crate) received: Arc<Mutex<VecDeque<Exchange>>>,
    pub(crate) notify: Arc<Notify>,
    pub(crate) max_retained: usize,
    pub(crate) copy_on_exchange: bool,
    pub(crate) fail_fast: bool,
    pub(crate) fail_fast_error: Arc<std::sync::Mutex<Option<CamelError>>>,
    pub(crate) assert_period_ms: u64,
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
        let mut guard = self
            .fail_fast_error
            .lock()
            .expect("fail_fast_error lock poisoned"); // allow-unwrap
        *guard = None;
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
        let mut guard = self
            .expectations
            .lock()
            .expect("expectations lock poisoned"); // allow-unwrap
        guard.set_expected_count(n);
    }

    /// Set a minimum count expectation: `assert_satisfied` panics unless at
    /// least `n` exchanges are retained.
    pub fn expect_minimum_count(&self, n: usize) {
        let mut guard = self
            .expectations
            .lock()
            .expect("expectations lock poisoned"); // allow-unwrap
        guard.set_minimum_count(n);
    }

    /// Add an expected body to the expectations list.
    pub fn expect_body(&self, body: camel_component_api::Body) {
        let mut guard = self
            .expectations
            .lock()
            .expect("expectations lock poisoned"); // allow-unwrap
        guard.push_body(body);
    }

    /// Add an expected header key-value pair to the expectations list.
    pub fn expect_header(&self, key: &str, value: impl Into<serde_json::Value>) {
        let mut guard = self
            .expectations
            .lock()
            .expect("expectations lock poisoned"); // allow-unwrap
        guard.push_header(key.to_string(), value.into());
    }

    /// Add an expected header regex pattern to the expectations list.
    ///
    /// After `await_exchanges()`, `assert_satisfied()` checks whether any
    /// received exchange has the named header matching the given regex pattern.
    pub fn expect_header_regex(&self, key: &str, pattern: &str) {
        let mut guard = self
            .expectations
            .lock()
            .expect("expectations lock poisoned"); // allow-unwrap
        guard.push_header_regex(key.to_string(), pattern.to_string());
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
    /// to compile). Diagnostic payloads keep the error above the
    /// `result_large_err` size threshold (clippy allow mirrors
    /// `do_try_segment.rs`).
    #[allow(clippy::result_large_err)]
    pub async fn try_assert_satisfied(&self) -> Result<(), MockAssertionError> {
        self.evaluate_expectations().await
    }

    /// Return the stored fail-fast error, if any.
    pub fn fail_fast_error(&self) -> Option<CamelError> {
        let guard = self
            .fail_fast_error
            .lock()
            .expect("fail_fast_error lock poisoned"); // allow-unwrap
        guard.clone()
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
        let mut guard = self
            .fail_fast_error
            .lock()
            .expect("fail_fast_error lock poisoned"); // allow-unwrap
        *guard = Some(error);
    }

    /// When `fail_fast` is enabled, record the assertion-mismatch sentinel
    /// before panicking. This ensures any concurrent or subsequent
    /// `MockProducer::poll_ready` / `call` invocation rejects with the fixed
    /// "fail-fast mode" message instead of being blocked on a panic-orphaned
    /// lock or a stale `None` sentinel.
    pub(crate) fn set_fail_fast_on_mismatch(&self) {
        if self.fail_fast {
            let mut guard = self
                .fail_fast_error
                .lock()
                .expect("fail_fast_error lock poisoned"); // allow-unwrap
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
pub(crate) fn clone_body(body: &camel_component_api::Body) -> camel_component_api::Body {
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
