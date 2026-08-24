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
use std::sync::Arc;

use tokio::sync::{Mutex, Notify};

use camel_api::component_metadata::ComponentMetadata;
use camel_component_api::UriConfig;
use camel_component_api::parse_uri;
use camel_component_api::{CamelError, Component, Endpoint};
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
// MockExpectations / MockEndpoint internals
// ---------------------------------------------------------------------------

mod assert;
mod expectations;
mod inner;
pub mod matcher;

pub use assert::MockAssertionError;
pub use expectations::MockExpectations;
pub use inner::{ExchangeAssert, MockEndpoint, MockEndpointInner};
pub use matcher::{BodyMatcher, HeaderMatcher};

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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
