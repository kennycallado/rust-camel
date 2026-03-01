use std::sync::Arc;

use crate::exchange::Exchange;

/// Aggregation function — left-fold binary: (accumulated, next) -> merged.
pub type AggregationFn = Arc<dyn Fn(Exchange, Exchange) -> Exchange + Send + Sync>;

/// How to combine collected exchanges into one.
#[derive(Clone)]
pub enum AggregationStrategy {
    /// Collects all bodies into Body::Json([body1, body2, ...]).
    CollectAll,
    /// Left-fold: f(f(ex1, ex2), ex3), ...
    Custom(AggregationFn),
}

/// When the bucket is considered complete and should be emitted.
#[derive(Clone)]
pub enum CompletionCondition {
    /// Emit when bucket reaches exactly N exchanges.
    Size(usize),
    /// Emit when predicate returns true for current bucket.
    Predicate(Arc<dyn Fn(&[Exchange]) -> bool + Send + Sync>),
}

/// Configuration for the Aggregator EIP.
#[derive(Clone)]
pub struct AggregatorConfig {
    /// Name of the header used as correlation key.
    pub header_name: String,
    /// When to emit the aggregated exchange.
    pub completion: CompletionCondition,
    /// How to combine the bucket into one exchange.
    pub strategy: AggregationStrategy,
}

impl AggregatorConfig {
    /// Start building config with correlation key extracted from the named header.
    pub fn correlate_by(header: impl Into<String>) -> AggregatorConfigBuilder {
        AggregatorConfigBuilder {
            header_name: header.into(),
            completion: None,
            strategy: AggregationStrategy::CollectAll,
        }
    }
}

/// Builder for `AggregatorConfig`.
pub struct AggregatorConfigBuilder {
    header_name: String,
    completion: Option<CompletionCondition>,
    strategy: AggregationStrategy,
}

impl AggregatorConfigBuilder {
    /// Emit when bucket has N exchanges.
    pub fn complete_when_size(mut self, n: usize) -> Self {
        self.completion = Some(CompletionCondition::Size(n));
        self
    }

    /// Emit when predicate returns true for the current bucket.
    pub fn complete_when<F>(mut self, predicate: F) -> Self
    where
        F: Fn(&[Exchange]) -> bool + Send + Sync + 'static,
    {
        self.completion = Some(CompletionCondition::Predicate(Arc::new(predicate)));
        self
    }

    /// Override the default `CollectAll` aggregation strategy.
    pub fn strategy(mut self, strategy: AggregationStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Build the config. Panics if no completion condition was set.
    pub fn build(self) -> AggregatorConfig {
        AggregatorConfig {
            header_name: self.header_name,
            completion: self.completion.expect("completion condition required"),
            strategy: self.strategy,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aggregator_config_complete_when_size() {
        let config = AggregatorConfig::correlate_by("orderId")
            .complete_when_size(3)
            .build();
        assert_eq!(config.header_name, "orderId");
        matches!(config.completion, CompletionCondition::Size(3));
        matches!(config.strategy, AggregationStrategy::CollectAll);
    }

    #[test]
    fn test_aggregator_config_complete_when_predicate() {
        let config = AggregatorConfig::correlate_by("key")
            .complete_when(|bucket| bucket.len() >= 2)
            .build();
        matches!(config.completion, CompletionCondition::Predicate(_));
    }

    #[test]
    fn test_aggregator_config_custom_strategy() {
        use std::sync::Arc;
        let f: AggregationFn = Arc::new(|acc, _next| acc);
        let config = AggregatorConfig::correlate_by("key")
            .complete_when_size(1)
            .strategy(AggregationStrategy::Custom(f))
            .build();
        matches!(config.strategy, AggregationStrategy::Custom(_));
    }

    #[test]
    #[should_panic(expected = "completion condition required")]
    fn test_aggregator_config_missing_completion_panics() {
        AggregatorConfig::correlate_by("key").build();
    }
}
