use std::sync::Arc;
use std::time::Duration;

use crate::CamelError;

/// Exponential backoff configuration for retry.
#[derive(Debug, Clone)]
pub struct ExponentialBackoff {
    pub max_attempts: u32,
    pub initial_delay: Duration,
    pub multiplier: f64,
    pub max_delay: Duration,
}

impl ExponentialBackoff {
    /// Create a new backoff config with default delays (100ms initial, 2x multiplier, 10s max).
    pub fn new(max_attempts: u32) -> Self {
        Self {
            max_attempts,
            initial_delay: Duration::from_millis(100),
            multiplier: 2.0,
            max_delay: Duration::from_secs(10),
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

    /// Compute the sleep duration before retry attempt N (0-indexed).
    pub fn delay_for(&self, attempt: u32) -> Duration {
        let millis = self.initial_delay.as_millis() as f64 * self.multiplier.powi(attempt as i32);
        let d = Duration::from_millis(millis as u64);
        d.min(self.max_delay)
    }
}

/// A rule that matches specific errors and defines retry + redirect behaviour.
pub struct ExceptionPolicy {
    /// Predicate: returns `true` if this policy applies to the given error.
    pub matches: Arc<dyn Fn(&CamelError) -> bool + Send + Sync>,
    /// Optional retry configuration; if absent, no retries are attempted.
    pub retry: Option<ExponentialBackoff>,
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
        self.policy.retry = Some(ExponentialBackoff::new(max_attempts));
        self
    }

    /// Override backoff parameters for the retry (call after `.retry()`).
    pub fn with_backoff(mut self, initial: Duration, multiplier: f64, max: Duration) -> Self {
        if let Some(ref mut b) = self.policy.retry {
            b.initial_delay = initial;
            b.multiplier = multiplier;
            b.max_delay = max;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CamelError;
    use std::time::Duration;

    #[test]
    fn test_exponential_backoff_defaults() {
        let b = ExponentialBackoff::new(3);
        assert_eq!(b.max_attempts, 3);
        assert_eq!(b.initial_delay, Duration::from_millis(100));
        assert_eq!(b.multiplier, 2.0);
        assert_eq!(b.max_delay, Duration::from_secs(10));
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
}
