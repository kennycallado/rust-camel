use std::time::Duration;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ThrottleStrategy {
    /// Queue messages until capacity available (default)
    #[default]
    Delay,
    /// Return error immediately when throttled
    Reject,
    /// Silently discard excess messages
    Drop,
}

#[derive(Debug, Clone)]
pub struct ThrottlerConfig {
    /// Maximum messages per time window
    pub max_requests: usize,
    /// Time window duration
    pub period: Duration,
    /// Behavior when throttled
    pub strategy: ThrottleStrategy,
}

impl ThrottlerConfig {
    pub fn new(max_requests: usize, period: Duration) -> Self {
        Self {
            max_requests,
            period,
            strategy: ThrottleStrategy::default(),
        }
    }

    pub fn strategy(mut self, s: ThrottleStrategy) -> Self {
        self.strategy = s;
        self
    }
}
