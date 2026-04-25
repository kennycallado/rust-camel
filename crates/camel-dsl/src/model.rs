pub use camel_api::{LanguageExpressionDef, ValueSourceDef};

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
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeclarativeErrorHandler {
    pub dead_letter_channel: Option<String>,
    pub retry: Option<DeclarativeRedeliveryPolicy>,
    pub on_exceptions: Option<Vec<DeclarativeOnException>>,
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
pub struct SetBodyStepDef {
    pub value: ValueSourceDef,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FilterStepDef {
    pub predicate: LanguageExpressionDef,
    pub steps: Vec<DeclarativeStep>,
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
    pub parallel: bool,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataFormatDef {
    pub format: String,
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
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeclarativeStep {
    To(ToStepDef),
    SetHeader(SetHeaderStepDef),
    SetBody(SetBodyStepDef),
    ConvertBodyTo(BodyTypeDef),
    DynamicRouter(DynamicRouterStepDef),
    Filter(FilterStepDef),
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
    Marshal(DataFormatDef),
    Unmarshal(DataFormatDef),
    Bean(BeanStepDef),
    Delay(DelayStepDef),
    Loop(LoopStepDef),
}
