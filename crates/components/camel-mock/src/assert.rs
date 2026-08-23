//! Non-panicking assertion surface for mock endpoints.
//!
//! [`MockAssertionError`] maps every assertion branch of
//! [`crate::MockEndpointInner::assert_satisfied`] to one variant whose
//! `Display` output is byte-identical to the panic message the panicking
//! variant produces for the same condition.

use crate::MockEndpointInner;
use camel_component_api::Exchange;

/// Diagnostic cap for received-value and header-key lists.
const DIAGNOSTIC_LIST_CAP: usize = 8;

/// Received-state snapshot for a failed header expectation.
///
/// `actual_values` holds the `{:?}`-formatted values of the expected key
/// across received exchanges that carry it, and `last_headers` holds the
/// sorted key list of the last received exchange. Both cap at
/// [`DIAGNOSTIC_LIST_CAP`] entries; overflow appends a `+N more` entry.
/// `last_headers` is `None` when no exchange was received.
struct HeaderDiagnostics {
    received_count: usize,
    actual_values: Vec<String>,
    last_headers: Option<String>,
}

/// Collect the received-state diagnostics for the expected `key`.
fn header_diagnostics(received: &[Exchange], key: &str) -> HeaderDiagnostics {
    let mut actual_values: Vec<String> = received
        .iter()
        .filter_map(|ex| ex.input.headers.get(key))
        .map(|v| format!("{v:?}"))
        .collect();
    let overflow = actual_values.len().saturating_sub(DIAGNOSTIC_LIST_CAP);
    actual_values.truncate(DIAGNOSTIC_LIST_CAP);
    if overflow > 0 {
        actual_values.push(format!("+{overflow} more"));
    }
    let last_headers = received.last().map(|ex| {
        let mut keys: Vec<&str> = ex.input.headers.keys().map(String::as_str).collect();
        keys.sort_unstable();
        let keys_overflow = keys.len().saturating_sub(DIAGNOSTIC_LIST_CAP);
        keys.truncate(DIAGNOSTIC_LIST_CAP);
        let mut rendered = keys.join(", ");
        if keys_overflow > 0 {
            rendered.push_str(&format!(", +{keys_overflow} more"));
        }
        rendered
    });
    HeaderDiagnostics {
        received_count: received.len(),
        actual_values,
        last_headers,
    }
}

/// Append the received-state clause shared by the header mismatch variants.
fn write_header_received_clause(
    f: &mut std::fmt::Formatter<'_>,
    received_count: usize,
    actual_values: &[String],
    last_headers: &Option<String>,
    key: &str,
) -> std::fmt::Result {
    if received_count == 0 {
        return write!(f, " (received 0 exchanges)");
    }
    if !actual_values.is_empty() {
        return write!(
            f,
            " (received {received_count} exchanges; '{key}' present with values: [{}])",
            actual_values.join(", ")
        );
    }
    write!(
        f,
        " (received {received_count} exchanges; '{key}' absent from all received exchanges; last exchange headers: [{}])",
        last_headers.as_deref().unwrap_or("")
    )
}

/// Error returned by [`MockEndpointInner::try_assert_satisfied`] when a
/// recorded expectation is not satisfied (or is malformed).
///
/// Every assertion branch of [`MockEndpointInner::assert_satisfied`]
/// corresponds to exactly one variant. The `Display` output of each variant
/// equals the panic message the panicking variant produces for the same
/// condition. Body detail fields are pre-formatted (`{:?}`) strings.
#[non_exhaustive]
#[derive(Debug)]
pub enum MockAssertionError {
    /// Exact count expectation (`expect_count`) not met.
    CountMismatch {
        /// Endpoint name.
        endpoint: String,
        /// Expected number of exchanges.
        expected: usize,
        /// Actual number of retained exchanges.
        actual: usize,
    },
    /// Minimum count expectation (`expect_minimum_count`) not met.
    MinimumCountNotMet {
        /// Endpoint name.
        endpoint: String,
        /// Minimum number of exchanges expected.
        minimum: usize,
        /// Actual number of retained exchanges.
        actual: usize,
    },
    /// Number of expected bodies differs from the number of received bodies.
    BodyCountMismatch {
        /// Endpoint name.
        endpoint: String,
        /// Expected number of bodies.
        expected: usize,
        /// Actual number of bodies.
        actual: usize,
    },
    /// Ordered body at `index` does not match the expected body.
    BodyMismatch {
        /// Endpoint name.
        endpoint: String,
        /// Index of the mismatching body.
        index: usize,
        /// `{:?}`-formatted expected body.
        expected: String,
        /// `{:?}`-formatted actual body.
        actual: String,
    },
    /// Expected body not found in any received exchange (anyOrder mode).
    BodyNotFound {
        /// Endpoint name.
        endpoint: String,
        /// `{:?}`-formatted expected body.
        expected: String,
        /// Number of received exchanges at evaluation.
        received_count: usize,
    },
    /// Expected header key/value pair not found in any received exchange.
    HeaderNotFound {
        /// Endpoint name.
        endpoint: String,
        /// Header key.
        key: String,
        /// Expected header value.
        value: serde_json::Value,
        /// Number of received exchanges at evaluation.
        received_count: usize,
        /// `{:?}`-formatted values of `key` across received exchanges that
        /// carry it: up to [`DIAGNOSTIC_LIST_CAP`] values plus a final
        /// `+N more` entry on overflow.
        actual_values: Vec<String>,
        /// Sorted key list of the last received exchange: up to
        /// [`DIAGNOSTIC_LIST_CAP`] keys plus a `+N more` suffix on
        /// overflow; `None` when no exchange was received.
        last_headers: Option<String>,
    },
    /// No received exchange has the named header matching the regex pattern.
    HeaderRegexNotMatched {
        /// Endpoint name.
        endpoint: String,
        /// Header key.
        key: String,
        /// Regex pattern.
        pattern: String,
        /// Number of received exchanges at evaluation.
        received_count: usize,
        /// `{:?}`-formatted values of `key` across received exchanges that
        /// carry it: up to [`DIAGNOSTIC_LIST_CAP`] values plus a final
        /// `+N more` entry on overflow.
        actual_values: Vec<String>,
        /// Sorted key list of the last received exchange: up to
        /// [`DIAGNOSTIC_LIST_CAP`] keys plus a `+N more` suffix on
        /// overflow; `None` when no exchange was received.
        last_headers: Option<String>,
    },
    /// Header regex pattern failed to compile.
    ///
    /// A malformed expectation is a caller programming error, not an
    /// expectation mismatch: it does not trip the fail-fast latch.
    InvalidHeaderPattern {
        /// Endpoint name.
        endpoint: String,
        /// Header key.
        key: String,
        /// Regex pattern that failed to compile.
        pattern: String,
        /// Underlying regex compile error.
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl std::fmt::Display for MockAssertionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MockAssertionError::CountMismatch {
                endpoint,
                expected,
                actual,
            } => write!(
                f,
                "MockEndpoint '{endpoint}': expected {expected} exchanges, got {actual}"
            ),
            MockAssertionError::MinimumCountNotMet {
                endpoint,
                minimum,
                actual,
            } => write!(
                f,
                "MockEndpoint '{endpoint}': expected at least {minimum} exchanges, got {actual}"
            ),
            MockAssertionError::BodyCountMismatch {
                endpoint,
                expected,
                actual,
            } => write!(
                f,
                "MockEndpoint '{endpoint}': expected {expected} bodies, got {actual}"
            ),
            MockAssertionError::BodyMismatch {
                endpoint,
                index,
                expected,
                actual,
            } => write!(
                f,
                "MockEndpoint '{endpoint}': body[{index}] expected {expected}, got {actual}"
            ),
            MockAssertionError::BodyNotFound {
                endpoint,
                expected,
                received_count,
            } => write!(
                f,
                "MockEndpoint '{endpoint}': expected body {expected} not found in received exchanges (anyOrder mode) (received {received_count} exchanges)"
            ),
            MockAssertionError::HeaderNotFound {
                endpoint,
                key,
                value,
                received_count,
                actual_values,
                last_headers,
            } => {
                write!(
                    f,
                    "MockEndpoint '{endpoint}': expected header '{key}' = {value} not found in any received exchange"
                )?;
                write_header_received_clause(f, *received_count, actual_values, last_headers, key)
            }
            MockAssertionError::HeaderRegexNotMatched {
                endpoint,
                key,
                pattern,
                received_count,
                actual_values,
                last_headers,
            } => {
                write!(
                    f,
                    "MockEndpoint '{endpoint}': no received exchange has header '{key}' matching regex {pattern:?}"
                )?;
                write_header_received_clause(f, *received_count, actual_values, last_headers, key)
            }
            MockAssertionError::InvalidHeaderPattern {
                endpoint,
                pattern,
                source,
                ..
            } => write!(
                f,
                "MockEndpoint '{endpoint}': invalid regex pattern {pattern:?}: {source}"
            ),
        }
    }
}

impl std::error::Error for MockAssertionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            MockAssertionError::InvalidHeaderPattern { source, .. } => Some(&**source),
            _ => None,
        }
    }
}

impl MockEndpointInner {
    /// Evaluate all recorded expectations against the received snapshot.
    ///
    /// Single evaluation path shared by
    /// [`assert_satisfied`](crate::MockEndpointInner::assert_satisfied) and
    /// [`try_assert_satisfied`](crate::MockEndpointInner::try_assert_satisfied):
    /// exact count, minimum count, then — only when expected bodies are
    /// registered — body-count and per-body checks, then header and
    /// header-regex checks (independent of the body gate).
    ///
    /// On a mismatch-class error the fail-fast latch is tripped first (when
    /// `fail_fast` is enabled), then the error is returned. A malformed
    /// expectation ([`MockAssertionError::InvalidHeaderPattern`]) is returned
    /// without touching the latch.
    /// Diagnostic payloads keep `MockAssertionError` above the
    /// `result_large_err` size threshold (clippy allow mirrors
    /// `do_try_segment.rs`).
    #[allow(clippy::result_large_err)]
    pub(crate) async fn evaluate_expectations(&self) -> Result<(), MockAssertionError> {
        let received = self.get_received_exchanges().await;

        let guard = self
            .expectations
            .lock()
            .expect("expectations lock poisoned"); // allow-unwrap

        // Exact count expectation — checked before bodies; a mismatch
        // short-circuits all later checks.
        if let Some(n) = guard.expected_count
            && received.len() != n
        {
            return self.latch_err(MockAssertionError::CountMismatch {
                endpoint: self.name.clone(),
                expected: n,
                actual: received.len(),
            });
        }

        // Minimum count expectation.
        if let Some(m) = guard.minimum_count
            && received.len() < m
        {
            return self.latch_err(MockAssertionError::MinimumCountNotMet {
                endpoint: self.name.clone(),
                minimum: m,
                actual: received.len(),
            });
        }

        // Body expectations — gated: no expected bodies ⇒ body-count and
        // per-body checks are skipped.
        if !guard.expected_bodies.is_empty() {
            let received_bodies: Vec<_> = received.iter().map(|e| &e.input.body).collect();
            if guard.expected_bodies.len() != received_bodies.len() {
                return self.latch_err(MockAssertionError::BodyCountMismatch {
                    endpoint: self.name.clone(),
                    expected: guard.expected_bodies.len(),
                    actual: received_bodies.len(),
                });
            }
            if self.any_order {
                // Match in any order — each expected body must appear exactly once.
                let mut unmatched: Vec<_> = received_bodies.iter().collect();
                for expected in &guard.expected_bodies {
                    let idx = unmatched
                        .iter()
                        .position(|actual| body_eq(expected, actual));
                    match idx {
                        Some(i) => {
                            unmatched.remove(i);
                        }
                        None => {
                            return self.latch_err(MockAssertionError::BodyNotFound {
                                endpoint: self.name.clone(),
                                expected: format!("{expected:?}"),
                                received_count: received.len(),
                            });
                        }
                    }
                }
            } else {
                for (i, expected) in guard.expected_bodies.iter().enumerate() {
                    if !body_eq(expected, received_bodies[i]) {
                        return self.latch_err(MockAssertionError::BodyMismatch {
                            endpoint: self.name.clone(),
                            index: i,
                            expected: format!("{expected:?}"),
                            actual: format!("{:?}", received_bodies[i]),
                        });
                    }
                }
            }
        }

        // Expected headers (must all be present on at least one exchange).
        for (key, value) in &guard.expected_headers {
            let found = received
                .iter()
                .any(|ex| ex.input.headers.get(key).is_some_and(|v| v == value));
            if !found {
                let diag = header_diagnostics(&received, key);
                return self.latch_err(MockAssertionError::HeaderNotFound {
                    endpoint: self.name.clone(),
                    key: key.clone(),
                    value: value.clone(),
                    received_count: diag.received_count,
                    actual_values: diag.actual_values,
                    last_headers: diag.last_headers,
                });
            }
        }

        // Expected header regexes.
        for (key, pattern) in &guard.expected_header_regexes {
            let re = match regex::Regex::new(pattern) {
                Ok(re) => re,
                // Malformed expectation: caller programming error, not a
                // mismatch — the latch is not tripped.
                Err(e) => {
                    return Err(MockAssertionError::InvalidHeaderPattern {
                        endpoint: self.name.clone(),
                        key: key.clone(),
                        pattern: pattern.clone(),
                        source: Box::new(e),
                    });
                }
            };
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
                let diag = header_diagnostics(&received, key);
                return self.latch_err(MockAssertionError::HeaderRegexNotMatched {
                    endpoint: self.name.clone(),
                    key: key.clone(),
                    pattern: pattern.clone(),
                    received_count: diag.received_count,
                    actual_values: diag.actual_values,
                    last_headers: diag.last_headers,
                });
            }
        }

        Ok(())
    }

    /// Trip the fail-fast latch (when enabled) and wrap `err` for return.
    ///
    /// Single latch call site for every expectation-mismatch branch;
    /// [`MockAssertionError::InvalidHeaderPattern`] deliberately bypasses it.
    #[allow(clippy::result_large_err)]
    fn latch_err(&self, err: MockAssertionError) -> Result<(), MockAssertionError> {
        self.set_fail_fast_on_mismatch();
        Err(err)
    }
}

/// Compare two `Body` values for equality (used by expectation evaluation).
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
