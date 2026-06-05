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
#[path = "network_retry_tests.rs"]
mod tests;
