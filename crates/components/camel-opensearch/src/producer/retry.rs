//! Error types and retry helpers for OpenSearch operations.

use std::fmt;

use camel_component_api::CamelError;

/// Classifies an HTTP status code as transient (retryable) or permanent.
///
/// - **5xx** → transient (server-side issue, may self-heal)
/// - **4xx** → permanent (client-side error, retrying won't help)
/// - Network-level failures (no status code) are also transient.
pub(crate) fn is_transient(status: u16) -> bool {
    status >= 500
}

/// Classify a [`ProducerError`] as retryable (only transient errors).
pub(crate) fn is_retryable_producer_error(err: &ProducerError) -> bool {
    matches!(err, ProducerError::Transient(_))
}

// ── ProducerError: private error type for transient/permanent classification ──

/// Private error type that preserves transient vs permanent classification
/// for the retry loop. Converted to [`CamelError`] before surfacing to callers.
#[derive(Debug)]
pub(crate) enum ProducerError {
    /// Retryable error (5xx, network failure, timeout).
    Transient(String),
    /// Non-retryable error (4xx, invalid input, etc.).
    Permanent(String),
}

impl fmt::Display for ProducerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProducerError::Transient(s) | ProducerError::Permanent(s) => write!(f, "{}", s),
        }
    }
}

impl From<ProducerError> for CamelError {
    fn from(e: ProducerError) -> CamelError {
        match e {
            ProducerError::Transient(s) | ProducerError::Permanent(s) => {
                CamelError::ProcessorError(s)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── OS-004: Transient vs permanent failure classification ─────────────────

    #[test]
    fn test_is_transient_classifies_5xx_as_transient() {
        assert!(is_transient(500));
        assert!(is_transient(502));
        assert!(is_transient(503));
        assert!(is_transient(599));
    }

    #[test]
    fn test_is_transient_classifies_4xx_as_permanent() {
        assert!(!is_transient(400));
        assert!(!is_transient(401));
        assert!(!is_transient(403));
        assert!(!is_transient(404));
        assert!(!is_transient(409));
        assert!(!is_transient(422));
    }

    #[test]
    fn test_is_transient_classifies_success_as_permanent() {
        assert!(!is_transient(200));
        assert!(!is_transient(201));
        assert!(!is_transient(204));
        assert!(!is_transient(301));
    }

    // ── OS-014: ProducerError classification & retry behavior ──────────────

    #[test]
    fn producer_error_transient_is_retryable() {
        let err = ProducerError::Transient("server error".to_string());
        assert!(matches!(err, ProducerError::Transient(_)));
    }

    #[test]
    fn producer_error_permanent_is_not_retryable() {
        let err = ProducerError::Permanent("bad request".to_string());
        assert!(!matches!(err, ProducerError::Transient(_)));
    }

    #[test]
    fn producer_error_display_shows_message() {
        let err = ProducerError::Transient("timeout".to_string());
        assert_eq!(format!("{}", err), "timeout");
    }

    #[test]
    fn producer_error_converts_to_camel_error() {
        let err = ProducerError::Transient("test error".to_string());
        let camel: CamelError = err.into();
        assert!(
            format!("{}", camel).contains("test error"),
            "expected error message to contain 'test error', got: {}",
            camel
        );
    }

    // ── Retry loop regression tests ─────────────────────────────────────

    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use camel_component_api::{NetworkRetryPolicy, retry_async};

    #[tokio::test]
    async fn retry_loop_retries_transient_errors() {
        let policy = NetworkRetryPolicy {
            max_attempts: 3,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            ..NetworkRetryPolicy::default()
        };

        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_clone = call_count.clone();

        let result: Result<u32, ProducerError> = retry_async(
            &policy,
            None,
            || {
                let c = call_count_clone.clone();
                async move {
                    let n = c.fetch_add(1, Ordering::SeqCst);
                    if n < 2 {
                        Err(ProducerError::Transient(format!("attempt {n} failed")))
                    } else {
                        Ok(42)
                    }
                }
            },
            is_retryable_producer_error,
        )
        .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_loop_does_not_retry_permanent_errors() {
        let policy = NetworkRetryPolicy {
            max_attempts: 5,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            ..NetworkRetryPolicy::default()
        };

        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_clone = call_count.clone();

        let result: Result<u32, ProducerError> = retry_async(
            &policy,
            None,
            || {
                let c = call_count_clone.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err(ProducerError::Permanent("bad request".to_string()))
                }
            },
            is_retryable_producer_error,
        )
        .await;

        assert!(result.is_err());
        // Only one call — permanent errors are not retried
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retry_loop_invokes_operation_exactly_max_attempts_times() {
        let policy = NetworkRetryPolicy {
            max_attempts: 3,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
            multiplier: 1.0,
            ..NetworkRetryPolicy::default()
        };

        let calls = Arc::new(AtomicU32::new(0));
        let calls_clone = Arc::clone(&calls);

        let result: Result<u32, ProducerError> = retry_async(
            &policy,
            None,
            || {
                let c = calls_clone.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err(ProducerError::Transient("always fails".to_string()))
                }
            },
            is_retryable_producer_error,
        )
        .await;

        assert!(result.is_err());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "max_attempts=3 must yield exactly 3 invocations"
        );
    }
}
