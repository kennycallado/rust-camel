pub mod aggregator;
pub mod body;
pub mod body_converter;
pub mod circuit_breaker;
pub mod declarative;
pub mod dynamic_router;
pub mod error;
pub mod error_handler;
pub mod exchange;
pub mod filter;
pub mod health;
pub mod lifecycle;
pub mod load_balancer;
pub mod message;
pub mod metrics;
pub mod multicast;
pub mod processor;
pub mod producer;
pub mod route_controller;
pub mod runtime;
pub mod splitter;
pub mod supervision;
pub mod throttler;
pub mod value;

// Re-export core types at crate root for convenience.
pub use aggregator::{AggregationFn, AggregatorConfig, CompletionCondition};
pub use body::{Body, BoxAsyncRead, StreamBody, StreamMetadata};
pub use body_converter::{BodyType, convert as convert_body};
pub use circuit_breaker::CircuitBreakerConfig;
pub use declarative::{LanguageExpressionDef, ValueSourceDef};
pub use dynamic_router::{DynamicRouterConfig, RouterExpression};
pub use error::CamelError;
pub use error_handler::{
    ErrorHandlerConfig, ExceptionPolicy, ExceptionPolicyBuilder, HEADER_REDELIVERED,
    HEADER_REDELIVERY_COUNTER, HEADER_REDELIVERY_MAX_COUNTER, RedeliveryPolicy,
};
// Backwards compatibility re-export (deprecated)
#[allow(deprecated)]
pub use error_handler::ExponentialBackoff;
pub use exchange::{Exchange, ExchangePattern};
pub use filter::FilterPredicate;
pub use health::{HealthReport, ServiceHealth};
pub use lifecycle::{HealthStatus, Lifecycle, ServiceStatus};
pub use load_balancer::{LoadBalanceStrategy, LoadBalancerConfig};
pub use message::Message;
pub use metrics::{MetricsCollector, NoOpMetrics};
pub use multicast::{MulticastAggregationFn, MulticastConfig, MulticastStrategy};
pub use processor::{BoxProcessor, BoxProcessorExt, IdentityProcessor, Processor, ProcessorFn};
pub use producer::ProducerContext;
pub use route_controller::{RouteAction, RouteController, RouteStatus};
pub use runtime::{
    CANONICAL_CONTRACT_DECLARATIVE_ONLY_STEPS, CANONICAL_CONTRACT_EXCLUDED_DECLARATIVE_STEPS,
    CANONICAL_CONTRACT_NAME, CANONICAL_CONTRACT_RUST_ONLY_STEPS,
    CANONICAL_CONTRACT_SUPPORTED_STEPS, CANONICAL_CONTRACT_VERSION, CanonicalRouteSpec,
    RuntimeCommand, RuntimeCommandBus, RuntimeCommandResult, RuntimeEvent, RuntimeHandle,
    RuntimeQuery, RuntimeQueryBus, RuntimeQueryResult, canonical_contract_rejection_reason,
    canonical_contract_supports_step,
};
pub use splitter::{
    AggregationStrategy, SplitExpression, SplitterConfig, split_body, split_body_json_array,
    split_body_lines,
};
pub use supervision::SupervisionConfig;
pub use throttler::{ThrottleStrategy, ThrottlerConfig};
pub use value::{Headers, Value};
