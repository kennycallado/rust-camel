pub use camel_api::{LanguageExpressionDef, StreamSplitConfig, ValueSourceDef};

#[derive(Default)]
pub struct SecurityCompileContext {
    pub authenticator: Option<std::sync::Arc<dyn camel_auth::TokenAuthenticator>>,
    pub registry: Option<std::sync::Arc<camel_auth::SecurityPolicyRegistry>>,
    pub evaluator_registry: Option<std::sync::Arc<camel_auth::PermissionEvaluatorRegistry>>,
}

impl Clone for SecurityCompileContext {
    fn clone(&self) -> Self {
        Self {
            authenticator: self.authenticator.clone(),
            registry: self.registry.clone(),
            evaluator_registry: self.evaluator_registry.clone(),
        }
    }
}

impl SecurityCompileContext {
    pub fn new(
        authenticator: Option<std::sync::Arc<dyn camel_auth::TokenAuthenticator>>,
        registry: Option<std::sync::Arc<camel_auth::SecurityPolicyRegistry>>,
    ) -> Self {
        Self {
            authenticator,
            registry,
            evaluator_registry: None,
        }
    }

    pub fn with_evaluator_registry(
        mut self,
        registry: std::sync::Arc<camel_auth::PermissionEvaluatorRegistry>,
    ) -> Self {
        self.evaluator_registry = Some(registry);
        self
    }

    pub fn with_security_policy_registry(
        mut self,
        registry: std::sync::Arc<camel_auth::SecurityPolicyRegistry>,
    ) -> Self {
        self.registry = Some(registry);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclarativeConcurrency {
    Sequential,
    Concurrent { max: Option<usize> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclarativeCircuitBreaker {
    pub failure_threshold: u32,
    pub open_duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeclarativeSecurityPolicy {
    Roles {
        roles: Vec<String>,
        all_required: bool,
        trust_upstream_principal: bool,
    },
    Scopes {
        scopes: Vec<String>,
        all_required: bool,
        trust_upstream_principal: bool,
    },
    Ref {
        name: String,
    },
    /// WASM security policy reference. The `path` field is the registry name
    /// of a policy registered via `[security.policies.wasm.<name>]` in Camel.toml.
    /// Per-route `config` is not supported (registry is instance-based, not
    /// factory-based — see ADR-0014 §4 closure bd rc-0te).
    Wasm {
        /// Registry name of the WASM policy (from `[security.policies.wasm.<name>]` in Camel.toml).
        path: String,
        /// Reserved — must be empty. Use Camel.toml `[security.policies.wasm.<name>.config]` instead.
        config: std::collections::HashMap<String, String>,
    },
    Permission {
        policy: String,
        resource: camel_auth::PermissionValueSource,
        action: camel_auth::PermissionValueSource,
        scopes: Vec<String>,
        context: camel_auth::PermissionContextConfig,
        cache_ttl_secs: Option<u64>,
        cache_negative_ttl_secs: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeclarativeRedeliveryPolicy {
    pub max_attempts: u32,
    pub initial_delay_ms: u64,
    pub multiplier: f64,
    pub max_delay_ms: u64,
    pub jitter_factor: f64,
    pub handled_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeclarativeOnException {
    pub kind: Option<String>,
    pub message_contains: Option<String>,
    pub retry: Option<DeclarativeRedeliveryPolicy>,
    pub steps: Vec<DeclarativeStep>,
    pub handled: Option<bool>,
    pub continued: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeclarativeErrorHandler {
    pub dead_letter_channel: Option<String>,
    pub retry: Option<DeclarativeRedeliveryPolicy>,
    pub on_exceptions: Option<Vec<DeclarativeOnException>>,
    pub use_original_message: bool,
}

#[derive(Debug, Clone)]
pub struct DeclarativeRoute {
    pub from: String,
    pub route_id: String,
    pub auto_startup: bool,
    pub startup_order: i32,
    pub concurrency: Option<DeclarativeConcurrency>,
    pub error_handler: Option<DeclarativeErrorHandler>,
    pub circuit_breaker: Option<DeclarativeCircuitBreaker>,
    pub security_policy: Option<DeclarativeSecurityPolicy>,
    pub unit_of_work: Option<camel_api::UnitOfWorkConfig>,
    pub steps: Vec<DeclarativeStep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToStepDef {
    pub uri: String,
}

impl ToStepDef {
    pub fn new(uri: impl Into<String>) -> Self {
        Self { uri: uri.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogLevelDef {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

// Note: `Eq` is not derived because `ValueSourceDef` contains `serde_json::Value`
// which does not implement `Eq` (due to floating-point fields).
#[derive(Debug, Clone, PartialEq)]
pub struct LogStepDef {
    pub message: ValueSourceDef,
    pub level: LogLevelDef,
}

impl LogStepDef {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            message: ValueSourceDef::Literal(serde_json::Value::String(message.into())),
            level: LogLevelDef::Info,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SetHeaderStepDef {
    pub key: String,
    pub value: ValueSourceDef,
}

impl SetHeaderStepDef {
    pub fn literal(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: ValueSourceDef::Literal(serde_json::Value::String(value.into())),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SetPropertyStepDef {
    pub key: String,
    pub value: ValueSourceDef,
}

impl SetPropertyStepDef {
    pub fn literal(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: ValueSourceDef::Literal(serde_json::Value::String(value.into())),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SetBodyStepDef {
    pub value: ValueSourceDef,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FilterStepDef {
    pub predicate: LanguageExpressionDef,
    pub steps: Vec<DeclarativeStep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionStepDef {
    pub runtime: String,
    pub source: String,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WhenStepDef {
    pub predicate: LanguageExpressionDef,
    pub steps: Vec<DeclarativeStep>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChoiceStepDef {
    pub whens: Vec<WhenStepDef>,
    pub otherwise: Option<Vec<DeclarativeStep>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SplitExpressionDef {
    BodyLines,
    BodyJsonArray,
    Language(LanguageExpressionDef),
    Stream(StreamSplitConfig),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SplitAggregationDef {
    LastWins,
    CollectAll,
    Original,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SplitStepDef {
    pub expression: SplitExpressionDef,
    pub aggregation: SplitAggregationDef,
    pub parallel: bool,
    pub parallel_limit: Option<usize>,
    pub stop_on_exception: bool,
    pub steps: Vec<DeclarativeStep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AggregateStrategyDef {
    CollectAll,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AggregateStepDef {
    pub header: String,
    pub correlation_key: Option<String>,
    pub completion_size: Option<usize>,
    pub completion_timeout_ms: Option<u64>,
    pub completion_predicate: Option<LanguageExpressionDef>,
    pub strategy: AggregateStrategyDef,
    pub max_buckets: Option<usize>,
    pub bucket_ttl_ms: Option<u64>,
    pub force_completion_on_stop: Option<bool>,
    pub discard_on_timeout: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireTapStepDef {
    pub uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeanStepDef {
    pub name: String,
    pub method: String,
}

impl BeanStepDef {
    pub fn new(name: impl Into<String>, method: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            method: method.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ThrottleStrategyDef {
    #[default]
    Delay,
    Reject,
    Drop,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ThrottleStepDef {
    pub max_requests: usize,
    pub period_ms: u64,
    pub strategy: ThrottleStrategyDef,
    pub steps: Vec<DeclarativeStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LoadBalanceStrategyDef {
    #[default]
    RoundRobin,
    Random,
    Failover,
    Weighted {
        distribution_ratio: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadBalanceStepDef {
    pub strategy: LoadBalanceStrategyDef,
    pub steps: Vec<DeclarativeStep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicRouterStepDef {
    pub expression: LanguageExpressionDef,
    pub uri_delimiter: String,
    pub cache_size: i32,
    pub ignore_invalid_endpoints: bool,
    pub max_iterations: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingSlipStepDef {
    pub expression: LanguageExpressionDef,
    pub uri_delimiter: String,
    pub cache_size: i32,
    pub ignore_invalid_endpoints: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipientListStepDef {
    pub expression: LanguageExpressionDef,
    pub delimiter: String,
    pub parallel: bool,
    pub parallel_limit: Option<usize>,
    pub stop_on_exception: bool,
    pub aggregation: MulticastAggregationDef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MulticastAggregationDef {
    LastWins,
    CollectAll,
    Original,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MulticastStepDef {
    pub steps: Vec<DeclarativeStep>,
    pub parallel: bool,
    pub parallel_limit: Option<usize>,
    pub stop_on_exception: bool,
    pub timeout_ms: Option<u64>,
    pub aggregation: MulticastAggregationDef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptStepDef {
    pub expression: LanguageExpressionDef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyTypeDef {
    Text,
    Json,
    Bytes,
    Xml,
    Empty,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DataFormatDef {
    pub format: String,
    /// Optional JSON Schema for request-body validation (REST DSL
    /// `request_schema`). When present, the compiled UnmarshalService is
    /// wrapped with a `JsonSchemaValidateService`.
    pub schema: Option<serde_json::Value>,
    /// Optional per-format configuration (e.g. `{ "max_bytes": 67108864 }`).
    /// Deserialized by the config-aware factory per ADR-0038.
    pub config: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelayStepDef {
    pub delay_ms: u64,
    pub dynamic_header: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoopStepDef {
    pub count: Option<usize>,
    pub while_predicate: Option<LanguageExpressionDef>,
    pub steps: Vec<DeclarativeStep>,
    pub max_iterations: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamCacheStepDef {
    pub threshold: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidateStepDef {
    pub predicate: LanguageExpressionDef,
}

/// Claim Check EIP step definition.
///
/// Stashes/retrieves the message body from a `ClaimCheckRepository` by key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimCheckStepDef {
    /// Name of the registered `ClaimCheckRepository` (e.g. `"memory"`).
    pub repository: String,
    /// Operation: "set", "get", "get_and_remove", "push", "pop".
    pub operation: String,
    /// Expression that extracts the claim-check key from the exchange.
    pub key: LanguageExpressionDef,
    /// Optional filter string for selective merge-back during checkout operations.
    pub filter: Option<String>,
}

/// Idempotent Consumer EIP step definition.
///
/// Wraps a child sub-pipeline that runs only when the exchange's message-id
/// is NOT already present in the named `repository`. See ADR-0023.
#[derive(Debug, Clone, PartialEq)]
pub struct IdempotentConsumerStepDef {
    /// Name of the registered `IdempotentRepository` (e.g. `"memory"`).
    pub repository: String,
    /// Expression that extracts the message-id key from the exchange.
    pub expression: LanguageExpressionDef,
    /// Child sub-pipeline executed on first-time (non-duplicate) exchanges.
    pub steps: Vec<DeclarativeStep>,
    /// If `true`, reserve the key in the repository BEFORE running the child
    /// (eager mode). Default `false` (lazy: add only after the child completes).
    pub eager: Option<bool>,
    /// If `true` and `eager` is `true`, remove the key from the repository
    /// when the child returns `Failed`. Default `false`.
    pub remove_on_failure: Option<bool>,
}

/// Sampling EIP step definition.
///
/// Passes 1 of every N exchanges (counter-based, deterministic).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SamplingStepDef {
    /// Sampling period: 1 of every `period` exchanges passes.
    pub period: usize,
}

/// Sort EIP step definition.
///
/// Orders a body collection by extracting a sort key from each element
/// via a language expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortStepDef {
    /// Expression that produces the sort key for each element.
    pub expression: LanguageExpressionDef,
    /// Reverse (descending) sort when true. Default false (ascending).
    pub reverse: bool,
}

/// Resequence EIP step definition (Phase 3).
///
/// Supports batch and stream modes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResequenceStepDef {
    pub mode: ResequenceModeDef,
}

/// Resequence mode selection — batch or stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResequenceModeDef {
    Batch {
        correlation: String,
        sort: String,
        completion: camel_api::resequencer::BatchCompletion,
    },
    Stream {
        sequence: String,
        capacity: usize,
        gap_timeout: u64,
        on_gap: camel_api::resequencer::GapPolicy,
        on_capacity_exceeded: camel_api::resequencer::CapacityPolicy,
        dedup: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrichStepDef {
    pub uri: String,
    pub strategy: Option<String>,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DoTryCatchClauseDef {
    pub exception: Option<Vec<String>>,
    pub when: Option<LanguageExpressionDef>,
    pub on_when: Option<LanguageExpressionDef>,
    pub disposition: camel_api::error_handler::ExceptionDisposition,
    pub steps: Vec<DeclarativeStep>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DoTryFinallyDef {
    pub on_when: Option<LanguageExpressionDef>,
    pub steps: Vec<DeclarativeStep>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeclarativeStep {
    To(ToStepDef),
    SetHeader(SetHeaderStepDef),
    SetHeaderIfAbsent(SetHeaderStepDef),
    SetProperty(SetPropertyStepDef),
    SetBody(SetBodyStepDef),
    ConvertBodyTo(BodyTypeDef),
    DynamicRouter(DynamicRouterStepDef),
    Filter(FilterStepDef),
    Function(FunctionStepDef),
    LoadBalance(LoadBalanceStepDef),
    Log(LogStepDef),
    Choice(ChoiceStepDef),
    Split(SplitStepDef),
    Aggregate(AggregateStepDef),
    WireTap(WireTapStepDef),
    Multicast(MulticastStepDef),
    RoutingSlip(RoutingSlipStepDef),
    RecipientList(RecipientListStepDef),
    Stop,
    Throttle(ThrottleStepDef),
    Script(ScriptStepDef),
    StreamCache(StreamCacheStepDef),
    Marshal(DataFormatDef),
    Unmarshal(DataFormatDef),
    Validate(ValidateStepDef),
    Bean(BeanStepDef),
    Delay(DelayStepDef),
    Loop(LoopStepDef),
    Enrich(EnrichStepDef),
    PollEnrich(EnrichStepDef),
    IdempotentConsumer(IdempotentConsumerStepDef),
    ClaimCheck(ClaimCheckStepDef),
    Sampling(SamplingStepDef),
    Sort(SortStepDef),
    Resequence(ResequenceStepDef),
    DoTry {
        steps: Vec<DeclarativeStep>,
        catch: Vec<DoTryCatchClauseDef>,
        finally: Option<DoTryFinallyDef>,
    },
}

impl DeclarativeStep {
    pub fn kind(&self) -> crate::contract::DeclarativeStepKind {
        match self {
            DeclarativeStep::To(_) => crate::contract::DeclarativeStepKind::To,
            DeclarativeStep::Log(_) => crate::contract::DeclarativeStepKind::Log,
            DeclarativeStep::SetHeader(_) => crate::contract::DeclarativeStepKind::SetHeader,
            DeclarativeStep::SetHeaderIfAbsent(_) => {
                crate::contract::DeclarativeStepKind::SetHeaderIfAbsent
            }
            DeclarativeStep::SetProperty(_) => crate::contract::DeclarativeStepKind::SetProperty,
            DeclarativeStep::SetBody(_) => crate::contract::DeclarativeStepKind::SetBody,
            DeclarativeStep::ConvertBodyTo(_) => {
                crate::contract::DeclarativeStepKind::ConvertBodyTo
            }
            DeclarativeStep::DynamicRouter(_) => {
                crate::contract::DeclarativeStepKind::DynamicRouter
            }
            DeclarativeStep::Filter(_) => crate::contract::DeclarativeStepKind::Filter,
            DeclarativeStep::Function(_) => crate::contract::DeclarativeStepKind::Function,
            DeclarativeStep::LoadBalance(_) => crate::contract::DeclarativeStepKind::LoadBalance,
            DeclarativeStep::Choice(_) => crate::contract::DeclarativeStepKind::Choice,
            DeclarativeStep::Split(_) => crate::contract::DeclarativeStepKind::Split,
            DeclarativeStep::Aggregate(_) => crate::contract::DeclarativeStepKind::Aggregate,
            DeclarativeStep::WireTap(_) => crate::contract::DeclarativeStepKind::WireTap,
            DeclarativeStep::Multicast(_) => crate::contract::DeclarativeStepKind::Multicast,
            DeclarativeStep::RoutingSlip(_) => crate::contract::DeclarativeStepKind::RoutingSlip,
            DeclarativeStep::RecipientList(_) => {
                crate::contract::DeclarativeStepKind::RecipientList
            }
            DeclarativeStep::Stop => crate::contract::DeclarativeStepKind::Stop,
            DeclarativeStep::Throttle(_) => crate::contract::DeclarativeStepKind::Throttle,
            DeclarativeStep::Script(_) => crate::contract::DeclarativeStepKind::Script,
            DeclarativeStep::StreamCache(_) => crate::contract::DeclarativeStepKind::StreamCache,
            DeclarativeStep::Marshal(_) => crate::contract::DeclarativeStepKind::Marshal,
            DeclarativeStep::Unmarshal(_) => crate::contract::DeclarativeStepKind::Unmarshal,
            DeclarativeStep::Validate(_) => crate::contract::DeclarativeStepKind::Validate,
            DeclarativeStep::Bean(_) => crate::contract::DeclarativeStepKind::Bean,
            DeclarativeStep::Delay(_) => crate::contract::DeclarativeStepKind::Delay,
            DeclarativeStep::Loop(_) => crate::contract::DeclarativeStepKind::Loop,
            DeclarativeStep::Enrich(_) => crate::contract::DeclarativeStepKind::Enrich,
            DeclarativeStep::PollEnrich(_) => crate::contract::DeclarativeStepKind::PollEnrich,
            DeclarativeStep::IdempotentConsumer(_) => {
                crate::contract::DeclarativeStepKind::IdempotentConsumer
            }
            DeclarativeStep::ClaimCheck(_) => crate::contract::DeclarativeStepKind::ClaimCheck,
            DeclarativeStep::Sampling(_) => crate::contract::DeclarativeStepKind::Sampling,
            DeclarativeStep::Sort(_) => crate::contract::DeclarativeStepKind::Sort,
            DeclarativeStep::Resequence(_) => crate::contract::DeclarativeStepKind::Resequence,
            DeclarativeStep::DoTry { .. } => crate::contract::DeclarativeStepKind::DoTry,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_step_def_new() {
        let def = ToStepDef::new("direct:a");
        assert_eq!(def.uri, "direct:a");
    }

    #[test]
    fn log_step_def_info() {
        let def = LogStepDef::info("hello");
        assert_eq!(def.level, LogLevelDef::Info);
        match def.message {
            ValueSourceDef::Literal(v) => assert_eq!(v, serde_json::Value::String("hello".into())),
            _ => panic!("expected literal"),
        }
    }

    #[test]
    fn set_header_literal() {
        let def = SetHeaderStepDef::literal("key", "value");
        assert_eq!(def.key, "key");
        match def.value {
            ValueSourceDef::Literal(v) => assert_eq!(v, serde_json::Value::String("value".into())),
            _ => panic!("expected literal"),
        }
    }

    #[test]
    fn bean_step_def_new() {
        let def = BeanStepDef::new("myBean", "process");
        assert_eq!(def.name, "myBean");
        assert_eq!(def.method, "process");
    }

    #[test]
    fn throttle_strategy_default() {
        assert_eq!(ThrottleStrategyDef::default(), ThrottleStrategyDef::Delay);
    }

    #[test]
    fn load_balance_strategy_default() {
        assert_eq!(
            LoadBalanceStrategyDef::default(),
            LoadBalanceStrategyDef::RoundRobin
        );
    }

    #[test]
    fn concurrency_variants_equality() {
        assert_eq!(
            DeclarativeConcurrency::Sequential,
            DeclarativeConcurrency::Sequential
        );
        assert_ne!(
            DeclarativeConcurrency::Sequential,
            DeclarativeConcurrency::Concurrent { max: None }
        );
    }

    #[test]
    fn body_type_variants() {
        assert_eq!(BodyTypeDef::Text, BodyTypeDef::Text);
        assert_ne!(BodyTypeDef::Text, BodyTypeDef::Json);
    }

    #[test]
    fn data_format_def() {
        let def = DataFormatDef {
            format: "protobuf".into(),
            schema: None,
            config: None,
        };
        assert_eq!(def.format, "protobuf");
        assert!(def.schema.is_none());
    }

    #[test]
    fn stream_cache_step_def() {
        let def = StreamCacheStepDef {
            threshold: Some(1024),
        };
        assert_eq!(def.threshold, Some(1024));
    }

    #[test]
    fn delay_step_def() {
        let def = DelayStepDef {
            delay_ms: 500,
            dynamic_header: Some("X-Delay".into()),
        };
        assert_eq!(def.delay_ms, 500);
        assert_eq!(def.dynamic_header.as_deref(), Some("X-Delay"));
    }

    #[test]
    fn circuit_breaker_def() {
        let cb = DeclarativeCircuitBreaker {
            failure_threshold: 3,
            open_duration_ms: 5000,
        };
        assert_eq!(cb.failure_threshold, 3);
        assert_eq!(cb.open_duration_ms, 5000);
    }
}
