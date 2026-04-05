use std::sync::Arc;
use std::time::Duration;

use crate::exchange::Exchange;

/// Aggregation function — left-fold binary: (accumulated, next) -> merged.
pub type AggregationFn = Arc<dyn Fn(Exchange, Exchange) -> Exchange + Send + Sync>;

/// Strategy for correlating exchanges into aggregation buckets.
pub enum CorrelationStrategy {
    /// Correlate by the value of a named header.
    HeaderName(String),
    /// Correlate by evaluating an expression using a language registry.
    Expression { expr: String, language: String },
    /// Correlate using a custom function.
    #[allow(clippy::type_complexity)]
    Fn(Arc<dyn Fn(&Exchange) -> Option<String> + Send + Sync>),
}

impl Clone for CorrelationStrategy {
    fn clone(&self) -> Self {
        match self {
            CorrelationStrategy::HeaderName(h) => CorrelationStrategy::HeaderName(h.clone()),
            CorrelationStrategy::Expression { expr, language } => CorrelationStrategy::Expression {
                expr: expr.clone(),
                language: language.clone(),
            },
            CorrelationStrategy::Fn(f) => CorrelationStrategy::Fn(Arc::clone(f)),
        }
    }
}

impl std::fmt::Debug for CorrelationStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CorrelationStrategy::HeaderName(h) => f.debug_tuple("HeaderName").field(h).finish(),
            CorrelationStrategy::Expression { expr, language } => f
                .debug_struct("Expression")
                .field("expr", expr)
                .field("language", language)
                .finish(),
            CorrelationStrategy::Fn(_) => f.write_str("Fn(..)"),
        }
    }
}

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
    #[allow(clippy::type_complexity)]
    Predicate(Arc<dyn Fn(&[Exchange]) -> bool + Send + Sync>),
    /// Emit when the bucket has been inactive for the given duration.
    Timeout(Duration),
}

/// Determines how a bucket's completion is evaluated.
/// `Single` wraps one condition; `Any` completes when the first condition triggers.
#[derive(Clone)]
pub enum CompletionMode {
    Single(CompletionCondition),
    Any(Vec<CompletionCondition>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionReason {
    Size,
    Predicate,
    Timeout,
    Stop,
}

impl CompletionReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            CompletionReason::Size => "size",
            CompletionReason::Predicate => "predicate",
            CompletionReason::Timeout => "timeout",
            CompletionReason::Stop => "stop",
        }
    }
}

/// Configuration for the Aggregator EIP.
#[derive(Clone)]
pub struct AggregatorConfig {
    /// Name of the header used as correlation key.
    pub header_name: String,
    /// When to emit the aggregated exchange.
    pub completion: CompletionMode,
    /// Strategy for determining correlation keys.
    pub correlation: CorrelationStrategy,
    /// How to combine the bucket into one exchange.
    pub strategy: AggregationStrategy,
    /// Maximum number of correlation key buckets (memory protection).
    /// When limit is reached, new correlation keys are rejected.
    pub max_buckets: Option<usize>,
    /// Time-to-live for inactive buckets (memory protection).
    /// Buckets not updated for this duration are evicted.
    pub bucket_ttl: Option<Duration>,
    /// Force-complete all pending buckets when the route is stopped.
    pub force_completion_on_stop: bool,
    /// Discard bucket contents on timeout instead of emitting.
    pub discard_on_timeout: bool,
}

impl AggregatorConfig {
    /// Start building config with correlation key extracted from the named header.
    pub fn correlate_by(header: impl Into<String>) -> AggregatorConfigBuilder {
        let header_name = header.into();
        AggregatorConfigBuilder {
            header_name: header_name.clone(),
            completion: None,
            correlation: CorrelationStrategy::HeaderName(header_name),
            strategy: AggregationStrategy::CollectAll,
            max_buckets: None,
            bucket_ttl: None,
            force_completion_on_stop: false,
            discard_on_timeout: false,
        }
    }
}

/// Builder for `AggregatorConfig`.
pub struct AggregatorConfigBuilder {
    header_name: String,
    completion: Option<CompletionMode>,
    correlation: CorrelationStrategy,
    strategy: AggregationStrategy,
    max_buckets: Option<usize>,
    bucket_ttl: Option<Duration>,
    force_completion_on_stop: bool,
    discard_on_timeout: bool,
}

impl AggregatorConfigBuilder {
    /// Emit when bucket has N exchanges.
    pub fn complete_when_size(mut self, n: usize) -> Self {
        self.completion = Some(CompletionMode::Single(CompletionCondition::Size(n)));
        self
    }

    /// Emit when predicate returns true for the current bucket.
    pub fn complete_when<F>(mut self, predicate: F) -> Self
    where
        F: Fn(&[Exchange]) -> bool + Send + Sync + 'static,
    {
        self.completion = Some(CompletionMode::Single(CompletionCondition::Predicate(
            Arc::new(predicate),
        )));
        self
    }

    /// Emit when the bucket has been inactive for the given duration.
    pub fn complete_on_timeout(mut self, duration: Duration) -> Self {
        self.completion = Some(CompletionMode::Single(CompletionCondition::Timeout(
            duration,
        )));
        self
    }

    /// Emit when the bucket reaches `size` OR has been inactive for `timeout`.
    pub fn complete_on_size_or_timeout(mut self, size: usize, timeout: Duration) -> Self {
        self.completion = Some(CompletionMode::Any(vec![
            CompletionCondition::Size(size),
            CompletionCondition::Timeout(timeout),
        ]));
        self
    }

    /// Enable force-completion of pending buckets when the route is stopped.
    pub fn force_completion_on_stop(mut self, enabled: bool) -> Self {
        self.force_completion_on_stop = enabled;
        self
    }

    /// Discard bucket contents on timeout instead of emitting the aggregated exchange.
    pub fn discard_on_timeout(mut self, enabled: bool) -> Self {
        self.discard_on_timeout = enabled;
        self
    }

    /// Override the correlation strategy with a header-based key.
    pub fn correlate_by(mut self, header: impl Into<String>) -> Self {
        let header = header.into();
        self.header_name = header.clone();
        self.correlation = CorrelationStrategy::HeaderName(header);
        self
    }

    /// Override the default `CollectAll` aggregation strategy.
    pub fn strategy(mut self, strategy: AggregationStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Set the maximum number of correlation key buckets.
    /// When the limit is reached, new correlation keys are rejected with an error.
    pub fn max_buckets(mut self, max: usize) -> Self {
        self.max_buckets = Some(max);
        self
    }

    /// Set the time-to-live for inactive buckets.
    /// Buckets that haven't been updated for this duration will be evicted.
    pub fn bucket_ttl(mut self, ttl: Duration) -> Self {
        self.bucket_ttl = Some(ttl);
        self
    }

    /// Build the config. Panics if no completion condition was set.
    pub fn build(self) -> AggregatorConfig {
        AggregatorConfig {
            header_name: self.header_name,
            completion: self.completion.expect("completion condition required"),
            correlation: self.correlation,
            strategy: self.strategy,
            max_buckets: self.max_buckets,
            bucket_ttl: self.bucket_ttl,
            force_completion_on_stop: self.force_completion_on_stop,
            discard_on_timeout: self.discard_on_timeout,
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
        assert!(matches!(
            config.completion,
            CompletionMode::Single(CompletionCondition::Size(3))
        ));
        assert!(matches!(config.strategy, AggregationStrategy::CollectAll));
    }

    #[test]
    fn test_aggregator_config_complete_when_predicate() {
        let config = AggregatorConfig::correlate_by("key")
            .complete_when(|bucket| bucket.len() >= 2)
            .build();
        assert!(matches!(
            config.completion,
            CompletionMode::Single(CompletionCondition::Predicate(_))
        ));
    }

    #[test]
    fn test_aggregator_config_custom_strategy() {
        use std::sync::Arc;
        let f: AggregationFn = Arc::new(|acc, _next| acc);
        let config = AggregatorConfig::correlate_by("key")
            .complete_when_size(1)
            .strategy(AggregationStrategy::Custom(f))
            .build();
        assert!(matches!(config.strategy, AggregationStrategy::Custom(_)));
    }

    #[test]
    #[should_panic(expected = "completion condition required")]
    fn test_aggregator_config_missing_completion_panics() {
        AggregatorConfig::correlate_by("key").build();
    }

    #[test]
    fn test_complete_on_size_or_timeout() {
        let config = AggregatorConfig::correlate_by("key")
            .complete_on_size_or_timeout(3, Duration::from_secs(5))
            .build();
        assert!(matches!(config.completion, CompletionMode::Any(v) if v.len() == 2));
    }

    #[test]
    fn test_force_completion_on_stop_default() {
        let config = AggregatorConfig::correlate_by("key")
            .complete_when_size(1)
            .build();
        assert!(!config.force_completion_on_stop);
        assert!(!config.discard_on_timeout);
    }
}
