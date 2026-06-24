//! # rust-camel API
//!
//! Core Camel abstractions: exchanges, messages, body, errors, processors.
//!
//! Note: Component, Endpoint, Consumer, Producer traits are defined in
//! `camel-component-api`. This crate focuses on data types and EIP abstractions.
// TODO(API-006): Consider re-exporting Component, Endpoint, Consumer, Producer
// from camel-component-api here for a unified API surface.

pub mod aggregator;
pub mod backoff;
pub mod body;
pub mod body_converter;
pub mod circuit_breaker;
pub mod data_format;
pub mod datasource;
pub mod declarative;
pub mod delayer;
pub mod dynamic_router;
pub mod endpoint_pipeline;
pub mod error;
pub mod error_handler;
pub mod exchange;
pub mod exchange_lookup;
pub mod filter;
pub mod from_body;
pub mod function;
pub mod health;
pub mod lifecycle;
pub mod load_balancer;
pub mod loop_eip;
pub mod message;
pub mod metrics;
pub mod multicast;
pub mod outcome_pipeline;
pub mod outcome_segment;
pub mod pipeline_outcome;
pub mod platform;
pub mod processor;
pub mod producer;
pub mod recipient_list;
pub mod route_controller;
pub mod routing_slip;
pub mod runtime;
pub mod security_policy;
pub mod splitter;
pub mod stream_cache;
pub mod supervision;
pub mod template;
pub mod throttler;
pub mod unit_of_work;
pub mod value;
pub mod xml_convert;

// Re-export core types at crate root for convenience.
pub use aggregator::{AggregationFn, AggregatorConfig, CompletionCondition};
pub use backoff::{BackoffConfig, BackoffState};
pub use body::{Body, BoxAsyncRead, StreamBody, StreamMetadata};
pub use body_converter::{BodyType, convert as convert_body};
pub use circuit_breaker::CircuitBreakerConfig;
pub use data_format::DataFormat;
pub use datasource::{
    DatasourceCatalog, DatasourceConfig, DatasourceHandle, PoolFactory, ResourceRef,
};
pub use declarative::{LanguageExpressionDef, ValueSourceDef};
pub use delayer::DelayConfig;
pub use dynamic_router::{DynamicRouterConfig, RouterExpression};
pub use endpoint_pipeline::{CAMEL_SLIP_ENDPOINT, EndpointPipelineConfig, EndpointResolver};
pub use error::CamelError;
pub use error_handler::{
    BoundaryKind, ErrorHandlerConfig, ExceptionDisposition, ExceptionPolicy,
    ExceptionPolicyBuilder, HEADER_REDELIVERED, HEADER_REDELIVERY_COUNTER,
    HEADER_REDELIVERY_MAX_COUNTER, PolicyId, RedeliveryPolicy, RetryOutcome, RetryableStep,
    StepDisposition,
};
pub use security_policy::{
    AuthorizationDecision, PRINCIPAL_AUDIENCE_KEY, PRINCIPAL_CLAIMS_KEY, PRINCIPAL_ISSUER_KEY,
    PRINCIPAL_KEY, PRINCIPAL_ROLES_KEY, PRINCIPAL_SCOPES_KEY, PRINCIPAL_SUBJECT_KEY, Principal,
    SecurityPolicy, SecurityPolicyConfig, store_principal_properties,
};
// Backwards compatibility re-export (deprecated)
#[allow(deprecated)]
pub use error_handler::ExponentialBackoff;
pub use exchange::{Exchange, ExchangePattern};
pub use exchange_lookup::{ExchangeLookupPath, LookupPathError, PathSegment};
pub use filter::FilterPredicate;
pub use from_body::FromBody;
pub use function::{
    ExchangePatch, FunctionDefinition, FunctionDiff, FunctionId, FunctionInvocationError,
    FunctionInvoker, FunctionInvokerSync, PatchBody,
};
pub use health::{AsyncHealthCheck, CheckResult, HealthReport, HealthSource, ServiceHealth};
pub use lifecycle::{HealthStatus, Lifecycle, ServiceStatus};
pub use load_balancer::{LoadBalanceStrategy, LoadBalancerConfig};
pub use message::Message;
pub use metrics::{MetricsCollector, NoOpMetrics};
pub use multicast::{MulticastAggregationFn, MulticastConfig, MulticastStrategy};
pub use outcome_pipeline::OutcomePipeline;
pub use outcome_segment::OutcomeSegment;
pub use pipeline_outcome::PipelineOutcome;
pub use platform::{
    LeadershipEvent, LeadershipHandle, LeadershipService, NoopLeadershipService,
    NoopPlatformService, NoopReadinessGate, PlatformError, PlatformIdentity, PlatformService,
    ReadinessGate,
};
pub use processor::{
    BoxProcessor, BoxProcessorExt, IdentityProcessor, Processor, ProcessorFn, SyncBoxProcessor,
};
pub use producer::ProducerContext;
pub use route_controller::{RouteAction, RouteController, RouteStatus};
pub use routing_slip::{RoutingSlipConfig, RoutingSlipExpression};
pub use runtime::{
    CANONICAL_CONTRACT_DECLARATIVE_ONLY_STEPS, CANONICAL_CONTRACT_EXCLUDED_DECLARATIVE_STEPS,
    CANONICAL_CONTRACT_NAME, CANONICAL_CONTRACT_RUST_ONLY_STEPS,
    CANONICAL_CONTRACT_SUPPORTED_STEPS, CANONICAL_CONTRACT_VERSION, CanonicalConcurrencySpec,
    CanonicalFieldLoss, CanonicalLossReport, CanonicalRouteSpec, RuntimeCommand, RuntimeCommandBus,
    RuntimeCommandResult, RuntimeEvent, RuntimeHandle, RuntimeQuery, RuntimeQueryBus,
    RuntimeQueryResult, canonical_contract_rejection_reason, canonical_contract_supports_step,
};
pub use splitter::{
    AggregationStrategy, SplitExpression, SplitterConfig, StreamSplitConfig, StreamSplitFormat,
    StreamingSplitExpression, fragment_exchange, split_body, split_body_json_array,
    split_body_lines,
};
pub use supervision::SupervisionConfig;
pub use throttler::{ThrottleStrategy, ThrottlerConfig};
pub use unit_of_work::UnitOfWorkConfig;
pub use value::{Headers, Value};

// Template types
pub use template::{
    RouteTemplateSpec, TemplateError, TemplateInstanceRecord, TemplateParameterSpec,
    TemplatedRouteSpec,
};
