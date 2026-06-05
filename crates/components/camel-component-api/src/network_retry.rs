//! Shared reconnection/backoff policy for networked components.
//!
//! Provides [`NetworkRetryPolicy`] (config struct), [`retry_async`] (execution helper),
//! and [`retry_async_cancelable`] (cancellation-aware variant). Components that
//! supervise external processes (JMS, xj, xslt) should use only
//! [`NetworkRetryPolicy::delay_for`] inside their own supervision loops.
//!
//! Both [`retry_async`] and [`retry_async_cancelable`] accept an optional
//! `label` for component identity in retry logs:
//!
//! ```rust,ignore
//! use camel_component_api::retry_async;
//!
//! retry_async(&config.reconnect, Some("ws-producer"), op, is_retryable).await?;
//! ```
//!
//! When a label is set, log messages include `"ws-producer: transient error
//! — retrying"` with a `component` structured field that operators can filter
//! with `component=ws-producer`.
//!
//! For location-specific context (URLs, endpoints), wrap the retry call in a
//! [`tracing::span`](https://docs.rs/tracing/latest/tracing/macro.span.html)
//! whose fields are inherited by all log events inside the retry loop:
//!
//! ```rust,ignore
//! let span = tracing::info_span!("ws_connect", url = %url);
//! let _guard = span.enter();
//! retry_async(&config.reconnect, Some("ws-producer"), op, is_retryable).await?;
//! ```

use std::{future::Future, time::Duration};

use rand::RngExt;
use rand::distr::Uniform;
use serde::{Deserialize, Serialize};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use crate::CamelError;

// Default-value helpers — must return the field type (Duration), not u64,
// because serde calls these when the key is absent and deserialize_with is
// NOT invoked in that case; the value must already be the target type.
fn default_enabled() -> bool {
    true
}
fn default_max_attempts() -> u32 {
    10
}
fn default_initial_delay() -> Duration {
    Duration::from_millis(100)
}
fn default_multiplier() -> f64 {
    2.0
}
fn default_max_delay() -> Duration {
    Duration::from_millis(30_000)
}
fn default_jitter_factor() -> f64 {
    0.2
}

fn deserialize_duration_ms<'de, D>(d: D) -> Result<Duration, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let ms = u64::deserialize(d)?;
    Ok(Duration::from_millis(ms))
}

/// Reconnection and backoff policy for networked components.
///
/// Used in component config structs as a `reconnect` field:
/// ```toml
/// [default.components.redis.reconnect]
/// max_attempts = 10
/// initial_delay_ms = 100
/// max_delay_ms = 30000
/// ```
// Derive Serialize as well: several component configs (e.g. Kafka) derive
// Serialize, and a Deserialize-only nested struct breaks the derive on the
// host struct.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[must_use]
pub struct NetworkRetryPolicy {
    /// Whether reconnection is enabled at all.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Maximum number of attempts before giving up. 0 means unlimited.
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,

    /// Base delay for the first retry.
    #[serde(
        default = "default_initial_delay",
        rename = "initial_delay_ms",
        deserialize_with = "deserialize_duration_ms"
    )]
    pub initial_delay: Duration,

    /// Exponential backoff multiplier applied to each successive attempt.
    #[serde(default = "default_multiplier")]
    pub multiplier: f64,

    /// Maximum delay cap regardless of computed backoff.
    #[serde(
        default = "default_max_delay",
        rename = "max_delay_ms",
        deserialize_with = "deserialize_duration_ms"
    )]
    pub max_delay: Duration,

    /// Jitter factor in [0.0, 1.0]. Actual delay is `base ± (base * jitter_factor / 2)`.
    #[serde(default = "default_jitter_factor")]
    pub jitter_factor: f64,
}

impl Default for NetworkRetryPolicy {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            max_attempts: default_max_attempts(),
            initial_delay: default_initial_delay(),
            multiplier: default_multiplier(),
            max_delay: default_max_delay(),
            jitter_factor: default_jitter_factor(),
        }
    }
}

impl NetworkRetryPolicy {
    /// Returns a disabled policy — no retries will be attempted.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }

    /// Computes the sleep duration for a given zero-based attempt number.
    ///
    /// Formula: `clamp(initial * multiplier^attempt, 0, max_delay)` with random jitter.
    #[must_use]
    pub fn delay_for(&self, attempt: u32) -> Duration {
        let base_ms = self.initial_delay.as_millis() as f64;
        let exp = self.multiplier.powi(attempt as i32);
        let computed_ms = (base_ms * exp).min(self.max_delay.as_millis() as f64);

        // Apply random jitter: uniform in [-jitter_range/2, +jitter_range/2].
        // Uses rand to avoid thundering herd when multiple consumers reconnect.
        let jitter_range = computed_ms * self.jitter_factor;
        let jitter = if jitter_range > 0.0 {
            let mut rng = rand::rng();
            let lo = -jitter_range / 2.0;
            let hi = jitter_range / 2.0;
            debug_assert!(lo < hi, "jitter bounds are valid when jitter_range > 0");
            let dist = Uniform::new(lo, hi).unwrap(); // allow-unwrap
            rng.sample(dist)
        } else {
            0.0
        };

        let final_ms = (computed_ms + jitter).max(0.0) as u64;
        let max_delay_ms = u64::try_from(self.max_delay.as_millis()).unwrap_or(u64::MAX);
        Duration::from_millis(final_ms.min(max_delay_ms))
    }

    /// Returns `true` if another retry should be attempted.
    ///
    /// `attempt` is zero-based: 0 = first attempt, 1 = first retry, etc.
    #[must_use]
    pub fn should_retry(&self, attempt: u32) -> bool {
        self.enabled && (self.max_attempts == 0 || attempt < self.max_attempts)
    }
}

/// Executes `op` with reconnect/backoff according to `policy`.
///
/// `label`, if `Some`, is emitted as a structured `component` tracing field
/// and in the retry log message text so operators can identify which component
/// is retrying (e.g., `ws-producer: transient error — retrying`). Pass `None`
/// for the pre-0.14 backwards-compatible unlabeled path.
///
/// `is_retryable` classifies errors: retryable errors are retried, permanent
/// errors are not.
///
/// # Security note: error Display is logged at WARN level
///
/// On every retry, the error's [`Display`](std::fmt::Display) representation
/// is emitted via `tracing::warn!` for operator visibility during connection
/// retries. Callers MUST sanitize errors before returning them from `op` if
/// they may contain sensitive content such as connection strings, embedded
/// credentials, or host‑port pairs. Sanitization belongs at the source — in
/// the IO call whose error is wrapped — not here.
///
/// This log call is intentional and should not be removed: it provides the
/// only diagnostic signal that a networked component is retrying and why.
///
/// # Example
/// ```rust,ignore
/// let result = retry_async(
///     &config.reconnect,
///     Some("ws-producer"),
///     || async move { connect_to_server().await },
///     |err: &CamelError| matches!(err, CamelError::Io(_)),
/// ).await?;
/// ```
pub async fn retry_async<T, Op, Fut, IsRetryable, E>(
    policy: &NetworkRetryPolicy,
    label: Option<&'static str>,
    op: Op,
    is_retryable: IsRetryable,
) -> Result<T, E>
where
    Op: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    IsRetryable: Fn(&E) -> bool,
    E: std::fmt::Display,
{
    retry_async_inner(policy, op, is_retryable, None, label).await
}

/// Shared private implementation used by both [`retry_async`] and
/// [`retry_async_cancelable`].
async fn retry_async_inner<T, Op, Fut, IsRetryable, E>(
    policy: &NetworkRetryPolicy,
    mut op: Op,
    is_retryable: IsRetryable,
    cancel: Option<&CancellationToken>,
    label: Option<&'static str>,
) -> Result<T, E>
where
    Op: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    IsRetryable: Fn(&E) -> bool,
    E: std::fmt::Display,
{
    let mut attempt = 0u32;
    loop {
        match op().await {
            Ok(val) => return Ok(val),
            Err(err) => {
                if !is_retryable(&err) || !policy.should_retry(attempt + 1) {
                    return Err(err);
                }
                let delay = policy.delay_for(attempt);
                if let Some(component) = label {
                    tracing::warn!(
                        component,
                        attempt,
                        delay_ms = delay.as_millis(),
                        error = %err,
                        "{component}: transient error — retrying"
                    );
                } else {
                    tracing::warn!(
                        attempt,
                        delay_ms = delay.as_millis(),
                        error = %err,
                        "transient error — retrying"
                    );
                }
                // Honour cancellation only during inter-retry sleep, not
                // during the operation itself (that is the caller's
                // responsibility).
                if let Some(token) = cancel {
                    tokio::select! {
                        biased;
                        _ = token.cancelled() => return Err(err),
                        _ = sleep(delay) => {}
                    }
                } else {
                    sleep(delay).await;
                }
                attempt += 1;
            }
        }
    }
}

/// Like [`retry_async`] but honours a [`CancellationToken`] during inter-retry sleep.
///
/// Same semantics as [`retry_async`] except that if `cancel` fires while waiting
/// between attempts, the function returns the last operation error immediately.
/// Cancellation is **not** checked during the operation itself — the caller is
/// responsible for making the operation itself cancellation-aware if needed.
///
/// `label`, if `Some`, is emitted as a structured `component` tracing field
/// (see [`retry_async`] for details).
///
/// # Example
/// ```rust,ignore
/// let cancel = CancellationToken::new();
/// let result = retry_async_cancelable(
///     &config.reconnect,
///     Some("container-events"),
///     || async move { make_network_call().await },
///     |err| is_transient(err),
///     &cancel,
/// ).await;
/// ```
pub async fn retry_async_cancelable<T, Op, Fut, IsRetryable, E>(
    policy: &NetworkRetryPolicy,
    label: Option<&'static str>,
    op: Op,
    is_retryable: IsRetryable,
    cancel: &CancellationToken,
) -> Result<T, E>
where
    Op: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    IsRetryable: Fn(&E) -> bool,
    E: std::fmt::Display,
{
    retry_async_inner(policy, op, is_retryable, Some(cancel), label).await
}

/// Classify a [`CamelError`] as retryable (transient network/IO errors).
///
/// Retryable variants:
/// - [`CamelError::Io`] — I/O errors (connection refused, DNS, etc.)
/// - [`CamelError::ProcessorError`] whose message contains the literal
///   `[TRANSIENT]` marker (used by gRPC and other components to flag
///   retryable-by-classification errors).
/// - [`CamelError::ProcessorErrorWithSource`] whose message contains the
///   `[TRANSIENT]` marker (same semantics, preserves source error chain).
///
/// Non-retryable variants:
/// - [`CamelError::Config`], [`CamelError::TypeConversionFailed`],
///   [`CamelError::Stopped`], [`CamelError::EndpointCreationFailed`],
///   [`CamelError::ChannelClosed`] — permanent failures
pub fn is_retryable_camel_error(err: &CamelError) -> bool {
    matches!(err, CamelError::Io(_))
        || matches!(err, CamelError::ProcessorError(s) if s.contains("[TRANSIENT]"))
        || matches!(err, CamelError::ProcessorErrorWithSource(s, _) if s.contains("[TRANSIENT]"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn delay_for_grows_exponentially() {
        let p = NetworkRetryPolicy::default();
        let d0 = p.delay_for(0);
        let d1 = p.delay_for(1);
        let d2 = p.delay_for(2);
        // jitter makes exact values non-deterministic; verify ordering and bounds
        assert!(d0 >= Duration::from_millis(80)); // 100ms - 20% jitter floor
        assert!(d1 > d0);
        assert!(d2 > d1);
    }

    #[test]
    fn delay_for_is_capped_at_max_delay() {
        let p = NetworkRetryPolicy::default();
        let d_high = p.delay_for(100);
        assert!(d_high <= p.max_delay);
    }

    #[test]
    fn should_retry_false_when_disabled() {
        let p = NetworkRetryPolicy::disabled();
        assert!(!p.should_retry(0));
    }

    #[test]
    fn should_retry_false_when_max_attempts_reached() {
        let p = NetworkRetryPolicy {
            max_attempts: 3,
            ..NetworkRetryPolicy::default()
        };
        assert!(p.should_retry(2));
        assert!(!p.should_retry(3));
    }

    #[test]
    fn disabled_returns_disabled_policy() {
        let p = NetworkRetryPolicy::disabled();
        assert!(!p.enabled);
    }

    #[tokio::test]
    async fn retry_async_succeeds_on_first_attempt() {
        let policy = NetworkRetryPolicy::default();
        let count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let count_clone = count.clone();
        let result = retry_async::<u32, _, _, _, CamelError>(
            &policy,
            None,
            || {
                let c = count_clone.clone();
                async move {
                    c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok::<_, CamelError>(42u32)
                }
            },
            |_| false,
        )
        .await;
        assert_eq!(result.unwrap(), 42);
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retry_async_retries_on_transient_error() {
        let policy = NetworkRetryPolicy {
            max_attempts: 3,
            ..NetworkRetryPolicy::default()
        };
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempts_clone = attempts.clone();
        let result = retry_async::<u32, _, _, _, CamelError>(
            &policy,
            None,
            || {
                let c = attempts_clone.clone();
                async move {
                    let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if n < 2 {
                        Err(CamelError::ProcessorError("transient".to_string()))
                    } else {
                        Ok(99u32)
                    }
                }
            },
            |_| true, // all errors transient
        )
        .await;
        assert_eq!(result.unwrap(), 99);
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_async_does_not_retry_permanent_error() {
        let policy = NetworkRetryPolicy {
            max_attempts: 5,
            ..NetworkRetryPolicy::default()
        };
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempts_clone = attempts.clone();
        let result = retry_async::<u32, _, _, _, CamelError>(
            &policy,
            None,
            || {
                let c = attempts_clone.clone();
                async move {
                    c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Err(CamelError::ProcessorError("permanent".to_string()))
                }
            },
            |_| false, // no errors transient
        )
        .await;
        assert!(result.is_err());
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retry_async_exhausts_max_attempts() {
        let policy = NetworkRetryPolicy {
            max_attempts: 3,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            ..NetworkRetryPolicy::default()
        };
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempts_clone = attempts.clone();
        let result = retry_async::<u32, _, _, _, CamelError>(
            &policy,
            None,
            || {
                let c = attempts_clone.clone();
                async move {
                    c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Err(CamelError::ProcessorError("always fails".to_string()))
                }
            },
            |_| true,
        )
        .await;
        assert!(result.is_err());
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[test]
    fn deserializes_from_toml_with_ms_fields() {
        let toml_str = r#"
            max_attempts = 5
            initial_delay_ms = 200
            max_delay_ms = 60000
            multiplier = 1.5
            jitter_factor = 0.1
        "#;
        let p: NetworkRetryPolicy = toml::from_str(toml_str).unwrap();
        assert_eq!(p.max_attempts, 5);
        assert_eq!(p.initial_delay, Duration::from_millis(200));
        assert_eq!(p.max_delay, Duration::from_millis(60_000));
        assert!((p.multiplier - 1.5).abs() < f64::EPSILON);
        assert!((p.jitter_factor - 0.1).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn retry_async_does_not_retry_when_disabled() {
        let policy = NetworkRetryPolicy::disabled();
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempts_clone = attempts.clone();
        let result = retry_async::<u32, _, _, _, CamelError>(
            &policy,
            None,
            || {
                let c = attempts_clone.clone();
                async move {
                    c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Err(CamelError::ProcessorError("always fails".to_string()))
                }
            },
            |_| true,
        )
        .await;
        assert!(result.is_err());
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn should_retry_with_max_attempts_zero_is_unlimited() {
        let p = NetworkRetryPolicy {
            max_attempts: 0,
            ..NetworkRetryPolicy::default()
        };
        assert!(p.should_retry(0));
        assert!(p.should_retry(100));
        assert!(p.should_retry(10_000));
    }

    #[test]
    fn delay_for_with_zero_jitter_is_exact_exponential() {
        let p = NetworkRetryPolicy {
            initial_delay: Duration::from_millis(10),
            multiplier: 2.0,
            max_delay: Duration::from_millis(100),
            jitter_factor: 0.0,
            ..NetworkRetryPolicy::default()
        };
        assert_eq!(p.delay_for(0), Duration::from_millis(10));
        assert_eq!(p.delay_for(1), Duration::from_millis(20));
        assert_eq!(p.delay_for(2), Duration::from_millis(40));
        assert_eq!(p.delay_for(3), Duration::from_millis(80));
        assert_eq!(p.delay_for(4), Duration::from_millis(100));
    }

    // ── Generic retry_async with custom error type ──────────────────────

    #[derive(Debug, PartialEq)]
    struct TestError(bool); // bool = is_retryable

    impl std::fmt::Display for TestError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "TestError({})", self.0)
        }
    }

    #[tokio::test]
    async fn retry_async_works_with_custom_error_type() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let policy = NetworkRetryPolicy {
            max_attempts: 2,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            ..NetworkRetryPolicy::default()
        };

        let calls = std::sync::Arc::new(AtomicU32::new(0));
        let calls_clone = calls.clone();
        let result: Result<(), TestError> = retry_async(
            &policy,
            None,
            || {
                let c = calls_clone.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err(TestError(true))
                }
            },
            |e: &TestError| e.0,
        )
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn retry_async_custom_error_permanent_stops_immediately() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let policy = NetworkRetryPolicy {
            max_attempts: 5,
            initial_delay: Duration::from_millis(1),
            ..NetworkRetryPolicy::default()
        };

        let calls = std::sync::Arc::new(AtomicU32::new(0));
        let calls_clone = calls.clone();
        let result: Result<(), TestError> = retry_async(
            &policy,
            None,
            || {
                let c = calls_clone.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err(TestError(false)) // permanent
                }
            },
            |e: &TestError| e.0,
        )
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    // ── is_retryable_camel_error ────────────────────────────────────────

    #[test]
    fn is_retryable_camel_error_io_is_retryable() {
        let err = CamelError::Io("connection refused".to_string());
        assert!(is_retryable_camel_error(&err));
    }

    #[test]
    fn is_retryable_camel_error_transient_marker_is_retryable() {
        let err = CamelError::ProcessorError("grpc[TRANSIENT][UNAVAILABLE]: down".to_string());
        assert!(is_retryable_camel_error(&err));
    }

    #[test]
    fn is_retryable_camel_error_config_is_not_retryable() {
        let err = CamelError::Config("bad config".to_string());
        assert!(!is_retryable_camel_error(&err));
    }

    #[test]
    fn is_retryable_camel_error_processor_error_no_marker_is_not_retryable() {
        let err = CamelError::ProcessorError("something went wrong".to_string());
        assert!(!is_retryable_camel_error(&err));
    }

    #[test]
    fn is_retryable_camel_error_processor_with_source_transient_marker_is_retryable() {
        let err = CamelError::ProcessorErrorWithSource(
            "grpc[TRANSIENT][UNAVAILABLE]: down".to_string(),
            std::sync::Arc::new(std::io::Error::other("underlying cause")),
        );
        assert!(is_retryable_camel_error(&err));
    }

    #[test]
    fn is_retryable_camel_error_processor_with_source_no_marker_is_not_retryable() {
        let err = CamelError::ProcessorErrorWithSource(
            "some non-transient error".to_string(),
            std::sync::Arc::new(std::io::Error::other("underlying cause")),
        );
        assert!(!is_retryable_camel_error(&err));
    }

    // ── is_retryable_camel_error via retry_async ─────────────────────────

    #[tokio::test]
    async fn retry_async_with_is_retryable_camel_error_retries_on_io_error() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let policy = NetworkRetryPolicy {
            max_attempts: 3,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            ..NetworkRetryPolicy::default()
        };

        let calls = std::sync::Arc::new(AtomicU32::new(0));
        let calls_clone = calls.clone();
        let result = retry_async(
            &policy,
            None,
            || {
                let c = calls_clone.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err::<u32, _>(CamelError::Io("connection refused".to_string()))
                }
            },
            is_retryable_camel_error,
        )
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_async_with_is_retryable_camel_error_stops_on_permanent_camel_error() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let policy = NetworkRetryPolicy {
            max_attempts: 5,
            initial_delay: Duration::from_millis(1),
            ..NetworkRetryPolicy::default()
        };

        let calls = std::sync::Arc::new(AtomicU32::new(0));
        let calls_clone = calls.clone();
        let result = retry_async(
            &policy,
            None,
            || {
                let c = calls_clone.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err::<u32, _>(CamelError::Config("permanent".to_string()))
                }
            },
            is_retryable_camel_error,
        )
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    // ── retry_async_cancelable tests ─────────────────────────────────────

    #[tokio::test]
    async fn retry_async_cancelable_succeeds_without_cancellation() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let policy = NetworkRetryPolicy {
            max_attempts: 5,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            ..NetworkRetryPolicy::default()
        };
        let cancel = tokio_util::sync::CancellationToken::new();

        let attempts = std::sync::Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();
        let result: Result<u32, CamelError> = retry_async_cancelable(
            &policy,
            None,
            || {
                let c = attempts_clone.clone();
                async move {
                    let n = c.fetch_add(1, Ordering::SeqCst);
                    if n < 2 {
                        Err(CamelError::ProcessorError("transient".to_string()))
                    } else {
                        Ok(99u32)
                    }
                }
            },
            |_| true,
            &cancel,
        )
        .await;
        assert_eq!(result.unwrap(), 99);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_async_cancelable_exhausts_attempts_when_cancel_not_signaled() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let policy = NetworkRetryPolicy {
            max_attempts: 2,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            ..NetworkRetryPolicy::default()
        };
        // Token created but never cancelled — should behave like regular retry_async
        let cancel = tokio_util::sync::CancellationToken::new();

        let calls = std::sync::Arc::new(AtomicU32::new(0));
        let calls_clone = calls.clone();
        let result: Result<u32, CamelError> = retry_async_cancelable(
            &policy,
            None,
            || {
                let c = calls_clone.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err(CamelError::ProcessorError("transient".to_string()))
                }
            },
            |_| true,
            &cancel,
        )
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn retry_async_cancelable_returns_last_error_when_cancelled_during_sleep() {
        use std::sync::atomic::{AtomicU32, Ordering};

        // Long delay so cancel is guaranteed to fire during the sleep,
        // not before op() runs or after it wakes.
        let policy = NetworkRetryPolicy {
            max_attempts: 5,
            initial_delay: Duration::from_secs(60),
            max_delay: Duration::from_secs(60),
            multiplier: 1.0,
            ..NetworkRetryPolicy::default()
        };
        let cancel = tokio_util::sync::CancellationToken::new();

        let calls = std::sync::Arc::new(AtomicU32::new(0));
        let calls_clone = calls.clone();
        let op_ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let op_ran_clone = op_ran.clone();
        let cancel_clone = cancel.clone();

        // Spawn cancel task: poll until op signals it has run (and is about to
        // return Err), THEN cancel. The retry will be in its sleep(60s) at
        // this point. Eliminates the race in the previous spawn+sleep(1ms) approach.
        let cancel_task = tokio::spawn(async move {
            while !op_ran_clone.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            cancel_clone.cancel();
        });

        let result: Result<u32, CamelError> = retry_async_cancelable(
            &policy,
            None,
            || {
                let c = calls_clone.clone();
                let flag = op_ran.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    Err(CamelError::Io("connection refused".to_string()))
                }
            },
            |_| true,
            &cancel,
        )
        .await;

        cancel_task.await.ok();

        assert!(
            result.is_err(),
            "cancel should cause early return with error"
        );
        // Exactly one op call — cancel fired during first sleep, no second attempt
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retry_async_cancelable_honors_cancel_signalled_during_op_at_next_sleep() {
        // Per docs: cancel is checked during inter-retry sleep, NOT during op itself.
        // If cancel fires while op is in-flight, op completes normally (proving
        // cancel didn't kill it), then the next sleep boundary observes the
        // cancel and aborts with the last operation error.
        use std::sync::atomic::{AtomicU32, Ordering};

        let policy = NetworkRetryPolicy {
            max_attempts: 5,
            initial_delay: Duration::from_secs(60),
            max_delay: Duration::from_secs(60),
            multiplier: 1.0,
            ..NetworkRetryPolicy::default()
        };
        let cancel = tokio_util::sync::CancellationToken::new();
        let cancel_clone = cancel.clone();

        let calls = std::sync::Arc::new(AtomicU32::new(0));
        let calls_clone = calls.clone();

        // Cancel fires during op's 20ms sleep (at ~5ms)
        let cancel_task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            cancel_clone.cancel();
        });

        let result: Result<u32, CamelError> = retry_async_cancelable(
            &policy,
            None,
            || {
                let c = calls_clone.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    // Slow op: sleeps 20ms then returns retryable error.
                    // Cancel fires at ~5ms during this sleep but op completes.
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    Err(CamelError::Io("transient".to_string()))
                }
            },
            |_| true,
            &cancel,
        )
        .await;

        cancel_task.await.ok();

        // Op was called once and completed its 20ms sleep (cancel didn't kill it);
        // then the next sleep(60s) boundary observed cancel and aborted.
        assert!(
            result.is_err(),
            "cancel during op must abort at next sleep boundary"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "op ran exactly once — cancel honored at sleep, not during op"
        );
    }

    #[tokio::test]
    async fn retry_async_cancelable_does_not_retry_permanent_error() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let policy = NetworkRetryPolicy {
            max_attempts: 5,
            initial_delay: Duration::from_millis(1),
            ..NetworkRetryPolicy::default()
        };
        let cancel = tokio_util::sync::CancellationToken::new();

        let calls = std::sync::Arc::new(AtomicU32::new(0));
        let calls_clone = calls.clone();
        let result: Result<u32, CamelError> = retry_async_cancelable(
            &policy,
            None,
            || {
                let c = calls_clone.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err(CamelError::ProcessorError("permanent".to_string()))
                }
            },
            |_| false, // no errors are retryable
            &cancel,
        )
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    // ── Labeled retry observability tests ───────────────────────────────

    use std::fmt::Write as FmtWrite;
    use std::sync::{Arc, Mutex};
    use tracing::Subscriber;
    use tracing_subscriber::layer::SubscriberExt;

    /// A tracing `Layer` that captures formatted event field key=value pairs
    /// into a shared buffer.
    struct CollectingLayer {
        events: Arc<Mutex<Vec<String>>>,
    }

    impl<S: Subscriber> tracing_subscriber::Layer<S> for CollectingLayer {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut buf = String::new();
            let mut visitor = CollectingVisitor { fields: &mut buf };
            event.record(&mut visitor);
            if let Ok(mut events) = self.events.lock() {
                events.push(buf);
            }
        }
    }

    struct CollectingVisitor<'a> {
        fields: &'a mut String,
    }

    impl CollectingVisitor<'_> {
        fn record_field(&mut self, name: &str, value: &str) {
            write!(self.fields, " {name}={value}").ok();
        }
    }

    impl tracing::field::Visit for CollectingVisitor<'_> {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.record_field(field.name(), value);
        }
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.record_field(field.name(), &format!("{value:?}"));
        }
        fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
            self.record_field(field.name(), &value.to_string());
        }
    }

    #[tokio::test]
    async fn labeled_policy_emits_component_in_log_message() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let layer = CollectingLayer {
            events: events.clone(),
        };
        let subscriber = tracing_subscriber::registry().with(layer);
        let _guard = tracing::subscriber::set_default(subscriber);

        let policy = NetworkRetryPolicy {
            max_attempts: 2,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            ..NetworkRetryPolicy::default()
        };

        let result = retry_async::<u32, _, _, _, CamelError>(
            &policy,
            Some("ws-producer"),
            || async { Err(CamelError::Io("connection refused".to_string())) },
            |_| true,
        )
        .await;

        assert!(result.is_err());
        let captured = events.lock().unwrap();
        assert!(
            !captured.is_empty(),
            "expected at least one log event, got none"
        );
        // The first (and only) retry should contain the component label
        let first = &captured[0];
        assert!(
            first.contains("component=ws-producer"),
            "expected 'component=ws-producer' in log event, got: {first}"
        );
        assert!(
            first.contains("ws-producer: transient error"),
            "expected 'ws-producer: transient error' in log event, got: {first}"
        );
    }

    #[tokio::test]
    async fn labeled_policy_preserves_structured_fields() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let layer = CollectingLayer {
            events: events.clone(),
        };
        let subscriber = tracing_subscriber::registry().with(layer);
        let _guard = tracing::subscriber::set_default(subscriber);

        let policy = NetworkRetryPolicy {
            max_attempts: 2,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            ..NetworkRetryPolicy::default()
        };

        let result = retry_async::<u32, _, _, _, CamelError>(
            &policy,
            Some("sql-producer"),
            || async { Err(CamelError::Io("timeout".to_string())) },
            |_| true,
        )
        .await;

        assert!(result.is_err());
        let captured = events.lock().unwrap();
        assert!(!captured.is_empty());
        let first = &captured[0];
        // Structured fields must be present
        assert!(
            first.contains("attempt=0"),
            "expected 'attempt=0' field, got: {first}"
        );
        assert!(
            first.contains("component=sql-producer"),
            "expected 'component=sql-producer' field, got: {first}"
        );
        assert!(
            first.contains("error=IO error: timeout"),
            "expected 'error=IO error: timeout' field, got: {first}"
        );
        assert!(
            first.contains("delay_ms="),
            "expected 'delay_ms=' field, got: {first}"
        );
    }

    #[tokio::test]
    async fn unlabeled_policy_omits_component_field() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let layer = CollectingLayer {
            events: events.clone(),
        };
        let subscriber = tracing_subscriber::registry().with(layer);
        let _guard = tracing::subscriber::set_default(subscriber);

        let policy = NetworkRetryPolicy {
            max_attempts: 2,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            ..NetworkRetryPolicy::default()
        };
        // Unlabeled path (label = None) — no component field emitted

        let result = retry_async::<u32, _, _, _, CamelError>(
            &policy,
            None,
            || async { Err(CamelError::Io("connection refused".to_string())) },
            |_| true,
        )
        .await;

        assert!(result.is_err());
        let captured = events.lock().unwrap();
        assert!(!captured.is_empty());
        let first = &captured[0];
        assert!(
            !first.contains("component="),
            "expected no 'component=' field in unlabeled path, got: {first}"
        );
        assert!(
            first.contains("transient error — retrying"),
            "expected bare 'transient error — retrying' message, got: {first}"
        );
    }
}
