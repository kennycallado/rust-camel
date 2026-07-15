use crate::error::CamelError;
use crate::exchange::Exchange;
use std::sync::Arc;
use std::time::Duration;

pub type MulticastAggregationFn = Arc<dyn Fn(Exchange, Exchange) -> Exchange + Send + Sync>;

#[derive(Clone, Default)]
pub enum MulticastStrategy {
    #[default]
    LastWins,
    CollectAll,
    Original,
    Custom(MulticastAggregationFn),
}

impl std::fmt::Debug for MulticastStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MulticastStrategy::LastWins => f.write_str("LastWins"),
            MulticastStrategy::CollectAll => f.write_str("CollectAll"),
            MulticastStrategy::Original => f.write_str("Original"),
            MulticastStrategy::Custom(_) => f.write_str("Custom(..)"),
        }
    }
}

#[derive(Clone)]
pub struct MulticastConfig {
    pub parallel: bool,
    pub parallel_limit: Option<usize>,
    pub stop_on_exception: bool,
    pub timeout: Option<Duration>,
    pub aggregation: MulticastStrategy,
}

impl std::fmt::Debug for MulticastConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MulticastConfig")
            .field("parallel", &self.parallel)
            .field("parallel_limit", &self.parallel_limit)
            .field("stop_on_exception", &self.stop_on_exception)
            .field("timeout", &self.timeout)
            .field("aggregation", &self.aggregation)
            .finish()
    }
}

impl MulticastConfig {
    pub fn new() -> Self {
        Self {
            parallel: false,
            parallel_limit: None,
            stop_on_exception: false,
            timeout: None,
            aggregation: MulticastStrategy::default(),
        }
    }

    pub fn parallel(mut self, parallel: bool) -> Self {
        self.parallel = parallel;
        self
    }

    pub fn parallel_limit(mut self, limit: usize) -> Self {
        self.parallel_limit = Some(limit);
        self
    }

    pub fn stop_on_exception(mut self, stop: bool) -> Self {
        self.stop_on_exception = stop;
        self
    }

    pub fn timeout(mut self, duration: Duration) -> Self {
        self.timeout = Some(duration);
        self
    }

    pub fn aggregation(mut self, strategy: MulticastStrategy) -> Self {
        self.aggregation = strategy;
        self
    }

    /// Validates the configuration.
    ///
    /// Returns `Err(CamelError::Config)` if `parallel_limit` is set to 0,
    /// which would cause a `Semaphore::new(0)` panic at runtime.
    pub fn validate(&self) -> Result<(), CamelError> {
        if self.parallel && self.parallel_limit == Some(0) {
            return Err(CamelError::Config(
                "multicast parallel_limit must be > 0".to_string(),
            ));
        }
        Ok(())
    }
}

impl Default for MulticastConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_multicast_config_defaults() {
        let config = MulticastConfig::new();
        assert!(!config.parallel);
        assert!(config.parallel_limit.is_none());
        assert!(!config.stop_on_exception);
        assert!(config.timeout.is_none());
        assert!(matches!(config.aggregation, MulticastStrategy::LastWins));
    }

    #[test]
    fn test_multicast_config_builder() {
        let config = MulticastConfig::new()
            .parallel(true)
            .parallel_limit(4)
            .stop_on_exception(true)
            .timeout(Duration::from_millis(500));

        assert!(config.parallel);
        assert_eq!(config.parallel_limit, Some(4));
        assert!(config.stop_on_exception);
        assert_eq!(config.timeout, Some(Duration::from_millis(500)));
    }
}
