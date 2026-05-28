//! Shared backoff configuration for transient retry loops.
//!
//! Used by networked components (kafka, redis, ws, jms, xj, xslt) to
//! throttle reconnect/polling attempts with capped exponential backoff.
//!
//! **This is NOT route supervision.** Component backoff throttles transient
//! polling/reconnect loops. [`SupervisionConfig`](super::supervision::SupervisionConfig)
//! governs route restart policy. They share math but are separate concepts.

use std::time::Duration;

/// Configuration for capped exponential backoff.
///
/// Defaults: 100ms initial, 2x multiplier, 30s cap. Multiplier MUST be >= 1.0;
/// values below 1.0 silently produce zero-delay backoff (no throttling).
#[derive(Debug, Clone, PartialEq)]
pub struct BackoffConfig {
    pub initial_delay: Duration,
    pub multiplier: f64,
    pub max_delay: Duration,
}

impl Default for BackoffConfig {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_millis(100),
            multiplier: 2.0,
            max_delay: Duration::from_secs(30),
        }
    }
}

impl BackoffConfig {
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let exp = (attempt as i32).min(63);
        let exp_value = self.multiplier.powi(exp);
        let millis = (self.initial_delay.as_millis() as f64 * exp_value) as u128;
        let capped = millis.min(self.max_delay.as_millis());
        Duration::from_millis(capped as u64)
    }
}

/// Stateful backoff tracker. Call [`next_delay()`](Self::next_delay) after each
/// failed attempt and [`reset()`](Self::reset) after success.
#[derive(Debug, Clone)]
pub struct BackoffState {
    config: BackoffConfig,
    current_delay: Duration,
}

impl BackoffState {
    pub fn new(config: BackoffConfig) -> Self {
        let current_delay = config.initial_delay;
        Self {
            config,
            current_delay,
        }
    }

    pub fn next_delay(&mut self) -> Duration {
        let delay = self.current_delay;
        let exp_value = self.config.multiplier;
        let millis = (delay.as_millis() as f64 * exp_value) as u128;
        let capped = millis.min(self.config.max_delay.as_millis());
        self.current_delay = Duration::from_millis(capped as u64);
        delay
    }

    pub fn reset(&mut self) {
        self.current_delay = self.config.initial_delay;
    }

    pub fn current_delay(&self) -> Duration {
        self.current_delay
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_default_config_values() {
        let cfg = BackoffConfig::default();
        assert_eq!(cfg.initial_delay, Duration::from_millis(100));
        assert_eq!(cfg.multiplier, 2.0);
        assert_eq!(cfg.max_delay, Duration::from_secs(30));
    }

    #[test]
    fn backoff_state_exponential_growth() {
        let mut state = BackoffState::new(BackoffConfig::default());
        assert_eq!(state.next_delay(), Duration::from_millis(100));
        assert_eq!(state.next_delay(), Duration::from_millis(200));
        assert_eq!(state.next_delay(), Duration::from_millis(400));
        assert_eq!(state.next_delay(), Duration::from_millis(800));
    }

    #[test]
    fn backoff_state_caps_at_max() {
        let cfg = BackoffConfig {
            max_delay: Duration::from_millis(500),
            ..Default::default()
        };
        let mut state = BackoffState::new(cfg);
        assert_eq!(state.next_delay(), Duration::from_millis(100));
        assert_eq!(state.next_delay(), Duration::from_millis(200));
        assert_eq!(state.next_delay(), Duration::from_millis(400));
        assert_eq!(state.next_delay(), Duration::from_millis(500));
        assert_eq!(state.next_delay(), Duration::from_millis(500));
    }

    #[test]
    fn backoff_state_reset() {
        let mut state = BackoffState::new(BackoffConfig::default());
        state.next_delay();
        state.next_delay();
        assert_eq!(state.current_delay(), Duration::from_millis(400));
        state.reset();
        assert_eq!(state.current_delay(), Duration::from_millis(100));
        assert_eq!(state.next_delay(), Duration::from_millis(100));
    }

    #[test]
    fn backoff_delay_for_attempt_stateless() {
        let cfg = BackoffConfig::default();
        assert_eq!(cfg.delay_for_attempt(0), Duration::from_millis(100));
        assert_eq!(cfg.delay_for_attempt(1), Duration::from_millis(200));
        assert_eq!(cfg.delay_for_attempt(2), Duration::from_millis(400));
    }

    #[test]
    fn backoff_delay_for_attempt_caps() {
        let cfg = BackoffConfig {
            max_delay: Duration::from_secs(30),
            ..Default::default()
        };
        let d = cfg.delay_for_attempt(100);
        assert_eq!(d, Duration::from_secs(30));
    }

    #[test]
    fn backoff_custom_multiplier() {
        let cfg = BackoffConfig {
            multiplier: 1.5,
            initial_delay: Duration::from_millis(200),
            max_delay: Duration::from_secs(60),
        };
        let mut state = BackoffState::new(cfg);
        assert_eq!(state.next_delay(), Duration::from_millis(200));
        assert_eq!(state.next_delay(), Duration::from_millis(300));
        assert_eq!(state.next_delay(), Duration::from_millis(450));
    }
}
