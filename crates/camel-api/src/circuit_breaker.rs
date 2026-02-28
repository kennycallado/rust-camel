use std::time::Duration;

/// Configuration for the circuit breaker pattern.
///
/// The circuit breaker monitors failures and temporarily stops sending
/// requests to a failing service, giving it time to recover.
///
/// # States
///
/// - **Closed** — Normal operation. Failures are counted; when `failure_threshold`
///   consecutive failures occur, the circuit opens.
/// - **Open** — All calls are rejected with [`CamelError::CircuitOpen`](crate::CamelError::CircuitOpen).
///   After `open_duration` elapses, the circuit transitions to half-open.
/// - **Half-Open** — A single probe call is allowed through. If it succeeds
///   (`success_threshold` times), the circuit closes. If it fails, the circuit reopens.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening the circuit.
    pub failure_threshold: u32,
    /// Number of successful probes in half-open state before closing.
    ///
    /// **Current limitation:** Only `1` is effectively supported. The state
    /// machine resets to `Closed` on the first successful half-open probe
    /// regardless of this value. Multi-probe half-open tracking is deferred.
    pub success_threshold: u32,
    /// How long the circuit stays open before allowing a probe.
    pub open_duration: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            success_threshold: 1,
            open_duration: Duration::from_secs(30),
        }
    }
}

impl CircuitBreakerConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn failure_threshold(mut self, n: u32) -> Self {
        self.failure_threshold = n;
        self
    }

    pub fn open_duration(mut self, duration: Duration) -> Self {
        self.open_duration = duration;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = CircuitBreakerConfig::default();
        assert_eq!(config.failure_threshold, 5);
        assert_eq!(config.success_threshold, 1);
        assert_eq!(config.open_duration, Duration::from_secs(30));
    }

    #[test]
    fn test_builder_pattern() {
        let config = CircuitBreakerConfig::new()
            .failure_threshold(3)
            .open_duration(Duration::from_millis(500));
        assert_eq!(config.failure_threshold, 3);
        assert_eq!(config.success_threshold, 1); // default, no setter exposed
        assert_eq!(config.open_duration, Duration::from_millis(500));
    }
}
