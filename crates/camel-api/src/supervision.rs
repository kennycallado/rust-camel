//! Supervision configuration for automatic route restart.

use std::time::Duration;

/// Configuration for route supervision (automatic restart on crash).
#[derive(Debug, Clone, PartialEq)]
pub struct SupervisionConfig {
    /// Maximum number of restart attempts. `None` means infinite retries.
    /// Default: `Some(5)`.
    pub max_attempts: Option<u32>,

    /// Delay before the first restart attempt.
    /// Default: 1 second.
    pub initial_delay: Duration,

    /// Multiplier applied to the delay after each failed attempt.
    /// Default: 2.0 (doubles each time).
    pub backoff_multiplier: f64,

    /// Maximum delay cap between restart attempts.
    /// Default: 60 seconds.
    pub max_delay: Duration,
}

impl SupervisionConfig {
    /// Compute the delay before attempt number `attempt` (1-indexed).
    ///
    /// Formula: `min(initial_delay * backoff_multiplier^(attempt-1), max_delay)`
    pub fn next_delay(&self, attempt: u32) -> Duration {
        if attempt == 0 {
            return self.initial_delay;
        }
        // Cap exponent to prevent overflow with very large attempt counts
        let exp = ((attempt - 1) as i32).min(63);
        let exp_value = self.backoff_multiplier.powi(exp);
        let millis = (self.initial_delay.as_millis() as f64 * exp_value) as u128;
        let delay = Duration::from_millis(millis as u64);
        delay.min(self.max_delay)
    }
}

impl Default for SupervisionConfig {
    fn default() -> Self {
        Self {
            max_attempts: Some(5),
            initial_delay: Duration::from_secs(1),
            backoff_multiplier: 2.0,
            max_delay: Duration::from_secs(60),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_supervision_config_defaults() {
        let cfg = SupervisionConfig::default();
        assert_eq!(cfg.max_attempts, Some(5));
        assert_eq!(cfg.initial_delay, Duration::from_secs(1));
        assert_eq!(cfg.backoff_multiplier, 2.0);
        assert_eq!(cfg.max_delay, Duration::from_secs(60));
    }

    #[test]
    fn test_supervision_config_infinite() {
        let cfg = SupervisionConfig {
            max_attempts: None,
            ..Default::default()
        };
        assert!(cfg.max_attempts.is_none());
    }

    #[test]
    fn test_supervision_config_next_delay_growth() {
        let cfg = SupervisionConfig::default();
        let d1 = cfg.next_delay(1); // attempt 1 → 1s * 2^0 = 1s
        let d2 = cfg.next_delay(2); // attempt 2 → 1s * 2^1 = 2s
        let d3 = cfg.next_delay(3); // attempt 3 → 1s * 2^2 = 4s
        assert_eq!(d1, Duration::from_secs(1));
        assert_eq!(d2, Duration::from_secs(2));
        assert_eq!(d3, Duration::from_secs(4));
    }

    #[test]
    fn test_supervision_config_next_delay_capped() {
        let cfg = SupervisionConfig {
            max_delay: Duration::from_secs(5),
            ..Default::default()
        };
        let d = cfg.next_delay(10); // would be 512s, capped at 5s
        assert_eq!(d, Duration::from_secs(5));
    }

    #[test]
    fn test_next_delay_attempt_zero_returns_initial_delay() {
        let cfg = SupervisionConfig::default();
        let d = cfg.next_delay(0);
        assert_eq!(d, cfg.initial_delay);
    }
}
