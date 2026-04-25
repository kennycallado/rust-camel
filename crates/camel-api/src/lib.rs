pub mod aggregator;
pub mod body;
pub mod body_converter;
pub mod circuit_breaker;
pub mod data_format;
pub mod declarative;
pub mod delayer;
pub mod dynamic_router;
pub mod endpoint_pipeline;
pub mod error;
pub mod error_handler;
pub mod exchange;
pub mod filter;
pub mod from_body;
pub mod health;
pub mod lifecycle;
pub mod load_balancer;
pub mod loop_eip;
pub mod message;
pub mod metrics;
pub mod multicast;
pub mod platform;
pub mod processor;
pub mod producer;
pub mod recipient_list;
pub mod route_controller;
pub mod routing_slip;
pub mod runtime;
pub mod splitter;
pub mod supervision;
pub mod throttler;
pub mod unit_of_work;
pub mod value;

// Re-export core types at crate root for convenience.
pub use aggregator::{AggregationFn, AggregatorConfig, CompletionCondition};
pub use body::{Body, BoxAsyncRead, StreamBody, StreamMetadata};
pub use body_converter::{BodyType, convert as convert_body};
pub use circuit_breaker::CircuitBreakerConfig;
pub use data_format::DataFormat;
pub use declarative::{LanguageExpressionDef, ValueSourceDef};
pub use delayer::DelayConfig;
pub use dynamic_router::{DynamicRouterConfig, RouterExpression};
pub use endpoint_pipeline::{CAMEL_SLIP_ENDPOINT, EndpointPipelineConfig, EndpointResolver};
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
pub use from_body::FromBody;
pub use health::{HealthChecker, HealthReport, HealthSource, ServiceHealth};
pub use lifecycle::{HealthStatus, Lifecycle, ServiceStatus};
pub use load_balancer::{LoadBalanceStrategy, LoadBalancerConfig};
pub use message::Message;
pub use metrics::{MetricsCollector, NoOpMetrics};
pub use multicast::{MulticastAggregationFn, MulticastConfig, MulticastStrategy};
pub use platform::{
    LeadershipEvent, LeadershipHandle, LeadershipService, NoopLeadershipService,
    NoopPlatformService, NoopReadinessGate, PlatformError, PlatformIdentity, PlatformService,
    ReadinessGate,
};
pub use processor::{BoxProcessor, BoxProcessorExt, IdentityProcessor, Processor, ProcessorFn};
pub use producer::ProducerContext;
pub use route_controller::{RouteAction, RouteController, RouteStatus};
pub use routing_slip::{RoutingSlipConfig, RoutingSlipExpression};
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
pub use unit_of_work::UnitOfWorkConfig;
pub use value::{Headers, Value};
