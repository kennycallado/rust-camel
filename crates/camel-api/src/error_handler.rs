use std::sync::Arc;
use std::time::Duration;

use crate::CamelError;

/// Camel-compatible header names for redelivery state.
pub const HEADER_REDELIVERED: &str = "CamelRedelivered";
pub const HEADER_REDELIVERY_COUNTER: &str = "CamelRedeliveryCounter";
pub const HEADER_REDELIVERY_MAX_COUNTER: &str = "CamelRedeliveryMaxCounter";

/// Redelivery policy with exponential backoff and optional jitter.
#[derive(Debug, Clone)]
pub struct RedeliveryPolicy {
    pub max_attempts: u32,
    pub initial_delay: Duration,
    pub multiplier: f64,
    pub max_delay: Duration,
    pub jitter_factor: f64,
}

impl RedeliveryPolicy {
    /// Create a new policy with default delays (100ms initial, 2x multiplier, 10s max, no jitter).
    ///
    /// Note: `max_attempts = 0` means no retries (immediate failure to DLC/handler).
    /// Use `max_attempts > 0` to enable retry behavior.
    pub fn new(max_attempts: u32) -> Self {
        Self {
            max_attempts,
            initial_delay: Duration::from_millis(100),
            multiplier: 2.0,
            max_delay: Duration::from_secs(10),
            jitter_factor: 0.0,
        }
    }

    /// Override the initial delay before the first retry.
    pub fn with_initial_delay(mut self, d: Duration) -> Self {
        self.initial_delay = d;
        self
    }

    /// Override the backoff multiplier applied after each attempt.
    pub fn with_multiplier(mut self, m: f64) -> Self {
        self.multiplier = m;
        self
    }

    /// Cap the maximum delay between retries.
    pub fn with_max_delay(mut self, d: Duration) -> Self {
        self.max_delay = d;
        self
    }

    /// Set jitter factor (0.0 = no jitter, 0.2 = ±20% randomization).
    ///
    /// Recommended values: 0.1-0.3 (10-30%) for most use cases.
    /// Helps prevent thundering herd problems in distributed systems
    /// by adding randomization to retry timing.
    pub fn with_jitter(mut self, j: f64) -> Self {
        self.jitter_factor = j.clamp(0.0, 1.0);
        self
    }

    /// Compute the sleep duration before retry attempt N (0-indexed) with jitter applied.
    pub fn delay_for(&self, attempt: u32) -> Duration {
        let base_ms = self.initial_delay.as_millis() as f64 * self.multiplier.powi(attempt as i32);
        let capped_ms = base_ms.min(self.max_delay.as_millis() as f64);

        if self.jitter_factor > 0.0 {
            let jitter = capped_ms * self.jitter_factor * (rand::random::<f64>() * 2.0 - 1.0);
            Duration::from_millis((capped_ms + jitter).max(0.0) as u64)
        } else {
            Duration::from_millis(capped_ms as u64)
        }
    }
}

/// A rule that matches specific errors and defines retry + redirect behaviour.
pub struct ExceptionPolicy {
    /// Predicate: returns `true` if this policy applies to the given error.
    pub matches: Arc<dyn Fn(&CamelError) -> bool + Send + Sync>,
    /// Optional retry configuration; if absent, no retries are attempted.
    pub retry: Option<RedeliveryPolicy>,
    /// Optional URI of a specific endpoint to route failed exchanges to.
    pub handled_by: Option<String>,
}

impl ExceptionPolicy {
    /// Create a new policy that matches errors using the given predicate.
    pub fn new(matches: impl Fn(&CamelError) -> bool + Send + Sync + 'static) -> Self {
        Self {
            matches: Arc::new(matches),
            retry: None,
            handled_by: None,
        }
    }
}

impl Clone for ExceptionPolicy {
    fn clone(&self) -> Self {
        Self {
            matches: Arc::clone(&self.matches),
            retry: self.retry.clone(),
            handled_by: self.handled_by.clone(),
        }
    }
}

/// Full error handler configuration: Dead Letter Channel URI and per-exception policies.
#[derive(Clone)]
pub struct ErrorHandlerConfig {
    /// URI of the Dead Letter Channel endpoint (None = log only).
    pub dlc_uri: Option<String>,
    /// Per-exception policies evaluated in order; first match wins.
    pub policies: Vec<ExceptionPolicy>,
}

impl ErrorHandlerConfig {
    /// Log-only error handler: errors are logged but not forwarded anywhere.
    pub fn log_only() -> Self {
        Self {
            dlc_uri: None,
            policies: Vec::new(),
        }
    }

    /// Dead Letter Channel: failed exchanges are forwarded to the given URI.
    pub fn dead_letter_channel(uri: impl Into<String>) -> Self {
        Self {
            dlc_uri: Some(uri.into()),
            policies: Vec::new(),
        }
    }

    /// Start building an `ExceptionPolicy` attached to this config.
    pub fn on_exception(
        self,
        matches: impl Fn(&CamelError) -> bool + Send + Sync + 'static,
    ) -> ExceptionPolicyBuilder {
        ExceptionPolicyBuilder {
            config: self,
            policy: ExceptionPolicy::new(matches),
        }
    }
}

/// Builder for a single [`ExceptionPolicy`] attached to an [`ErrorHandlerConfig`].
pub struct ExceptionPolicyBuilder {
    config: ErrorHandlerConfig,
    policy: ExceptionPolicy,
}

impl ExceptionPolicyBuilder {
    /// Configure retry with the given maximum number of attempts (exponential backoff defaults).
    pub fn retry(mut self, max_attempts: u32) -> Self {
        self.policy.retry = Some(RedeliveryPolicy::new(max_attempts));
        self
    }

    /// Override backoff parameters for the retry (call after `.retry()`).
    pub fn with_backoff(mut self, initial: Duration, multiplier: f64, max: Duration) -> Self {
        if let Some(ref mut p) = self.policy.retry {
            p.initial_delay = initial;
            p.multiplier = multiplier;
            p.max_delay = max;
        }
        self
    }

    /// Set jitter factor for retry delays (call after `.retry()`).
    /// Valid range: 0.0 (no jitter) to 1.0 (±100% randomization).
    pub fn with_jitter(mut self, jitter_factor: f64) -> Self {
        if let Some(ref mut p) = self.policy.retry {
            p.jitter_factor = jitter_factor.clamp(0.0, 1.0);
        }
        self
    }

    /// Route failed exchanges matching this policy to the given URI instead of the DLC.
    pub fn handled_by(mut self, uri: impl Into<String>) -> Self {
        self.policy.handled_by = Some(uri.into());
        self
    }

    /// Finish this policy and return the updated config.
    pub fn build(mut self) -> ErrorHandlerConfig {
        self.config.policies.push(self.policy);
        self.config
    }
}

// Backwards compatibility alias
#[deprecated(since = "0.1.0", note = "Use `RedeliveryPolicy` instead")]
pub type ExponentialBackoff = RedeliveryPolicy;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CamelError;
    use std::time::Duration;

    #[test]
    fn test_redelivery_policy_defaults() {
        let p = RedeliveryPolicy::new(3);
        assert_eq!(p.max_attempts, 3);
        assert_eq!(p.initial_delay, Duration::from_millis(100));
        assert_eq!(p.multiplier, 2.0);
        assert_eq!(p.max_delay, Duration::from_secs(10));
        assert_eq!(p.jitter_factor, 0.0);
    }

    #[test]
    fn test_exception_policy_matches() {
        let policy = ExceptionPolicy::new(|e| matches!(e, CamelError::ProcessorError(_)));
        assert!((policy.matches)(&CamelError::ProcessorError("oops".into())));
        assert!(!(policy.matches)(&CamelError::Io("io".into())));
    }

    #[test]
    fn test_error_handler_config_log_only() {
        let config = ErrorHandlerConfig::log_only();
        assert!(config.dlc_uri.is_none());
        assert!(config.policies.is_empty());
    }

    #[test]
    fn test_error_handler_config_dlc() {
        let config = ErrorHandlerConfig::dead_letter_channel("log:dlc");
        assert_eq!(config.dlc_uri.as_deref(), Some("log:dlc"));
    }

    #[test]
    fn test_error_handler_config_with_policy() {
        let config = ErrorHandlerConfig::dead_letter_channel("log:dlc")
            .on_exception(|e| matches!(e, CamelError::Io(_)))
            .retry(2)
            .handled_by("log:io-errors")
            .build();
        assert_eq!(config.policies.len(), 1);
        let p = &config.policies[0];
        assert!(p.retry.is_some());
        assert_eq!(p.retry.as_ref().unwrap().max_attempts, 2);
        assert_eq!(p.handled_by.as_deref(), Some("log:io-errors"));
    }

    #[test]
    fn test_jitter_applies_randomness() {
        let policy = RedeliveryPolicy::new(3)
            .with_initial_delay(Duration::from_millis(100))
            .with_jitter(0.5);

        let mut delays = std::collections::HashSet::new();
        for _ in 0..10 {
            delays.insert(policy.delay_for(0));
        }

        assert!(delays.len() > 1, "jitter should produce varying delays");
    }

    #[test]
    fn test_jitter_stays_within_bounds() {
        let policy = RedeliveryPolicy::new(3)
            .with_initial_delay(Duration::from_millis(100))
            .with_jitter(0.5);

        for _ in 0..100 {
            let delay = policy.delay_for(0);
            assert!(
                delay >= Duration::from_millis(50),
                "delay too low: {:?}",
                delay
            );
            assert!(
                delay <= Duration::from_millis(150),
                "delay too high: {:?}",
                delay
            );
        }
    }

    #[test]
    fn test_max_attempts_zero_means_no_retries() {
        let policy = RedeliveryPolicy::new(0);
        assert_eq!(policy.max_attempts, 0);
    }

    #[test]
    fn test_jitter_zero_produces_exact_delay() {
        let policy = RedeliveryPolicy::new(3)
            .with_initial_delay(Duration::from_millis(100))
            .with_jitter(0.0);

        for _ in 0..10 {
            let delay = policy.delay_for(0);
            assert_eq!(delay, Duration::from_millis(100));
        }
    }

    #[test]
    fn test_jitter_one_produces_wide_range() {
        let policy = RedeliveryPolicy::new(3)
            .with_initial_delay(Duration::from_millis(100))
            .with_jitter(1.0);

        for _ in 0..100 {
            let delay = policy.delay_for(0);
            assert!(
                delay >= Duration::from_millis(0),
                "delay should be >= 0, got {:?}",
                delay
            );
            assert!(
                delay <= Duration::from_millis(200),
                "delay should be <= 200ms, got {:?}",
                delay
            );
        }
    }
}
