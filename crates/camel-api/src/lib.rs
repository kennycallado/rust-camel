pub mod aggregator;
pub mod body;
pub mod circuit_breaker;
pub mod declarative;
pub mod error;
pub mod error_handler;
pub mod exchange;
pub mod filter;
pub mod message;
pub mod metrics;
pub mod multicast;
pub mod processor;
pub mod producer;
pub mod route_controller;
pub mod splitter;
pub mod supervision;
pub mod value;

// Re-export core types at crate root for convenience.
pub use aggregator::{AggregationFn, AggregatorConfig, CompletionCondition};
pub use body::{Body, StreamBody};
pub use circuit_breaker::CircuitBreakerConfig;
pub use declarative::{LanguageExpressionDef, ValueSourceDef};
pub use error::CamelError;
pub use error_handler::{
    ErrorHandlerConfig, ExceptionPolicy, ExceptionPolicyBuilder, ExponentialBackoff,
};
pub use exchange::{Exchange, ExchangePattern};
pub use filter::FilterPredicate;
pub use message::Message;
pub use metrics::{MetricsCollector, NoOpMetrics};
pub use multicast::{MulticastAggregationFn, MulticastConfig, MulticastStrategy};
pub use processor::{BoxProcessor, BoxProcessorExt, IdentityProcessor, Processor, ProcessorFn};
pub use producer::ProducerContext;
pub use route_controller::{RouteAction, RouteController, RouteStatus};
pub use splitter::{
    AggregationStrategy, SplitExpression, SplitterConfig, split_body, split_body_json_array,
    split_body_lines,
};
pub use supervision::SupervisionConfig;
pub use value::{Headers, Value};
