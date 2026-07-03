// serde_yml migrated to noyalib (compat-serde-yaml shim) — closes RUSTSEC-2025-0068.
// Module alias preserves call-site paths byte-for-byte.
use noyalib::compat::serde_yaml as serde_yml;

use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Deserialize, Clone)]
pub struct RouteDslRoutes {
    /// Optional JSON Schema URL (ignored by the parser; consumed by SDKs/editors).
    #[serde(default, skip_serializing, rename = "$schema")]
    pub schema_url: Option<String>,

    #[serde(default)]
    pub routes: Vec<RouteDslRoute>,
    #[serde(default)]
    pub templates: Vec<RouteDslTemplate>,
    #[serde(default)]
    pub templated_routes: Vec<RouteDslTemplatedRoute>,
    #[serde(default)]
    pub rest: Vec<RouteDslRest>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Clone)]
pub struct RouteDslRoute {
    pub id: String,
    pub from: String,
    #[serde(default)]
    pub steps: Vec<RouteDslStep>,
    #[serde(default = "default_true")]
    pub auto_startup: bool,
    #[serde(default = "default_startup_order")]
    pub startup_order: i32,
    #[serde(default)]
    pub sequential: bool,
    #[serde(default)]
    pub concurrent: Option<usize>,
    #[serde(default)]
    pub error_handler: Option<RouteDslErrorHandler>,
    #[serde(default)]
    pub circuit_breaker: Option<RouteDslCircuitBreaker>,
    #[serde(default)]
    pub security_policy: Option<RouteDslSecurityPolicy>,
    #[serde(default)]
    pub on_complete: Option<String>,
    #[serde(default)]
    pub on_failure: Option<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct RouteDslSecurityPolicy {
    #[serde(default)]
    pub roles: Option<Vec<String>>,
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
    #[serde(default)]
    pub all_required: Option<bool>,
    #[serde(default)]
    pub r#ref: Option<String>,
    #[serde(default)]
    pub wasm: Option<String>,
    #[serde(default)]
    pub config: Option<std::collections::HashMap<String, String>>,
    #[serde(default)]
    pub permission: Option<RouteDslPermissionPolicy>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct RouteDslPermissionPolicy {
    pub policy: String,
    #[serde(default)]
    pub resource: Option<RouteDslPermissionValueSource>,
    #[serde(default)]
    pub action: Option<RouteDslPermissionValueSource>,
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
    #[serde(default)]
    pub context: Option<RouteDslPermissionContext>,
    #[serde(default)]
    pub cache_ttl_secs: Option<u64>,
    #[serde(default)]
    pub cache_negative_ttl_secs: Option<u64>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct RouteDslPermissionValueSource {
    #[serde(default)]
    pub literal: Option<String>,
    #[serde(default)]
    pub header: Option<String>,
    #[serde(default)]
    pub property: Option<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct RouteDslPermissionContext {
    #[serde(default)]
    pub headers: Vec<String>,
    #[serde(default)]
    pub properties: Vec<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Clone)]
pub struct RouteDslErrorHandler {
    #[serde(default)]
    pub dead_letter_channel: Option<String>,
    #[serde(default)]
    pub retry: Option<RouteDslRedeliveryPolicy>,
    #[serde(default)]
    pub on_exceptions: Option<Vec<RouteDslOnException>>,
    #[serde(default)]
    pub use_original_message: bool,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Clone)]
pub struct RouteDslOnException {
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub message_contains: Option<String>,
    #[serde(default)]
    pub retry: Option<RouteDslRedeliveryPolicy>,
    #[serde(default)]
    pub steps: Vec<RouteDslStep>,
    #[serde(default)]
    pub handled: Option<bool>,
    #[serde(default)]
    pub continued: Option<bool>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Clone)]
pub struct RouteDslRedeliveryPolicy {
    pub max_attempts: u32,
    #[serde(default = "default_initial_delay_ms")]
    pub initial_delay_ms: u64,
    #[serde(default = "default_multiplier")]
    pub multiplier: f64,
    #[serde(default = "default_max_delay_ms")]
    pub max_delay_ms: u64,
    #[serde(default = "default_jitter_factor")]
    pub jitter_factor: f64,
    #[serde(default)]
    pub handled_by: Option<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Clone)]
pub struct RouteDslCircuitBreaker {
    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: u32,
    #[serde(default = "default_open_duration_ms")]
    pub open_duration_ms: u64,
}

fn default_true() -> bool {
    true
}

fn default_startup_order() -> i32 {
    1000
}

fn default_initial_delay_ms() -> u64 {
    100
}

fn default_multiplier() -> f64 {
    2.0
}

fn default_max_delay_ms() -> u64 {
    10_000
}

fn default_jitter_factor() -> f64 {
    0.0
}

fn default_failure_threshold() -> u32 {
    5
}

fn default_open_duration_ms() -> u64 {
    30_000
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct DelayStep {
    pub delay: DelayBody,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
#[cfg_attr(feature = "schema", schemars(untagged))]
pub enum DelayBody {
    Short(u64),
    Full(DelayFullConfig),
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct DelayFullConfig {
    pub delay_ms: u64,
    #[serde(default)]
    pub dynamic_header: Option<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct LoopStep {
    #[serde(rename = "loop")]
    pub loop_data: LoopData,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
#[cfg_attr(feature = "schema", schemars(untagged))]
pub enum LoopData {
    /// Shorthand: `loop: 3`
    Count(usize),
    /// Full form with config block.
    Full(LoopFullConfig),
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct LoopFullConfig {
    /// Fixed iteration count. Mutually exclusive with `while`.
    pub count: Option<usize>,
    /// While condition — expression-only (no nested steps).
    #[serde(rename = "while")]
    pub while_expr: Option<LoopWhileExpr>,
    #[serde(default)]
    pub steps: Vec<RouteDslStep>,
}

/// Expression-only predicate for loop while condition.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct LoopWhileExpr {
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub simple: Option<String>,
    #[serde(default)]
    pub rhai: Option<String>,
    #[serde(default)]
    pub jsonpath: Option<String>,
    #[serde(default)]
    pub xpath: Option<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
#[cfg_attr(feature = "schema", schemars(untagged))]
pub enum RouteDslStep {
    To(ToStep),
    SetHeader(SetHeaderStep),
    SetProperty(SetPropertyStep),
    SetBody(SetBodyStep),
    Bean(BeanStep),
    Choice(ChoiceStep),
    DynamicRouter(DynamicRouterStep),
    Filter(FilterStep),
    Function(FunctionStep),
    LoadBalance(LoadBalanceStep),
    Log(LogStep),
    Split(SplitStep),
    Aggregate(AggregateStep),
    WireTap(WireTapStep),
    Multicast(MulticastStep),
    RoutingSlip(RoutingSlipStep),
    RecipientList(RecipientListStep),
    ScatterGather(ScatterGatherStep),
    Stop(StopStep),
    StreamCache(StreamCacheStep),
    Throttle(ThrottleStep),
    Transform(TransformStep),
    Script(ScriptStep),
    ConvertBodyTo(ConvertBodyToStep),
    Marshal(MarshalStep),
    Unmarshal(UnmarshalStep),
    Delay(DelayStep),
    DoTry(DoTryStep),
    Loop(LoopStep),
    Validate(ValidateStep),
    Enrich(EnrichStep),
    PollEnrich(PollEnrichStep),
    IdempotentConsumer(IdempotentConsumerStep),
    ClaimCheck(ClaimCheckStep),
    Sampling(SamplingStep),
    Sort(SortStep),
    Resequence(ResequenceStep),
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct FunctionStep {
    pub function: FunctionData,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct FunctionData {
    pub runtime: String,
    pub source: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct ToStep {
    pub to: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct SetHeaderStep {
    pub set_header: SetHeaderData,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct SetPropertyStep {
    pub set_property: SetPropertyData,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct SetHeaderData {
    pub key: String,
    #[serde(default)]
    pub value: Option<serde_json::Value>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub simple: Option<String>,
    #[serde(default)]
    pub rhai: Option<String>,
    #[serde(default)]
    pub jsonpath: Option<String>,
    #[serde(default)]
    pub xpath: Option<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct SetPropertyData {
    pub name: String,
    #[serde(default)]
    pub value: Option<serde_json::Value>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub simple: Option<String>,
    #[serde(default)]
    pub rhai: Option<String>,
    #[serde(default)]
    pub jsonpath: Option<String>,
    #[serde(default)]
    pub xpath: Option<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct SetBodyStep {
    pub set_body: SetBodyData,
}

/// `transform:` step — alias for `set_body:`, reuses all SetBodyData types.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct TransformStep {
    pub transform: SetBodyData,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
#[cfg_attr(feature = "schema", schemars(untagged))]
pub enum SetBodyData {
    // Config must be tried BEFORE Literal: serde_json::Value matches any YAML value (including
    // objects), so it would greedily swallow `{ simple: "..." }` as a Literal JSON object
    // instead of a SetBodyConfig. By placing Config first, structured forms like
    // `set_body: { simple: "..." }` deserialize correctly.
    Config(SetBodyConfig),
    Literal(serde_json::Value),
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct SetBodyConfig {
    #[serde(default)]
    pub value: Option<serde_json::Value>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub simple: Option<String>,
    #[serde(default)]
    pub rhai: Option<String>,
    #[serde(default)]
    pub jsonpath: Option<String>,
    #[serde(default)]
    pub xpath: Option<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
#[cfg_attr(feature = "schema", schemars(untagged))]
pub enum LogBody {
    Message(String),
    Config(LogConfig),
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct LogConfig {
    /// The log message. Can be a plain string literal or a nested expression object.
    pub message: LogMessageData,
    #[serde(default)]
    pub level: Option<String>,
}

/// The `message` field inside a `log: { message: ... }` config block.
/// Either a bare string literal or a value-source expression (simple, rhai, language+source).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
#[cfg_attr(feature = "schema", schemars(untagged))]
pub enum LogMessageData {
    Literal(String),
    Expr(LogMessageExpr),
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct LogMessageExpr {
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub simple: Option<String>,
    #[serde(default)]
    pub rhai: Option<String>,
    #[serde(default)]
    pub jsonpath: Option<String>,
    #[serde(default)]
    pub xpath: Option<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct LogStep {
    pub log: LogBody,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct FilterStep {
    pub filter: PredicateBlock,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct PredicateBlock {
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub simple: Option<String>,
    #[serde(default)]
    pub rhai: Option<String>,
    #[serde(default)]
    pub jsonpath: Option<String>,
    #[serde(default)]
    pub xpath: Option<String>,
    #[serde(default)]
    pub steps: Vec<RouteDslStep>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct ChoiceStep {
    pub choice: ChoiceData,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ChoiceData {
    #[serde(default)]
    pub when: Vec<PredicateBlock>,
    #[serde(default)]
    pub otherwise: Option<Vec<RouteDslStep>>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct DoTryStep {
    pub do_try: DoTryData,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct DoTryData {
    pub steps: Vec<RouteDslStep>,
    #[serde(default)]
    pub catch: Vec<CatchClauseData>,
    #[serde(default)]
    pub finally: Option<FinallyData>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct CatchClauseData {
    pub exception: Option<Vec<String>>,
    pub when: Option<String>,
    pub on_when: Option<String>,
    #[serde(default = "default_handled_disposition")]
    pub disposition: camel_api::error_handler::ExceptionDisposition,
    pub steps: Vec<RouteDslStep>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct FinallyData {
    pub on_when: Option<String>,
    pub steps: Vec<RouteDslStep>,
}

fn default_handled_disposition() -> camel_api::error_handler::ExceptionDisposition {
    camel_api::error_handler::ExceptionDisposition::Handled
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct SplitStep {
    pub split: SplitData,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct SplitData {
    #[serde(default)]
    pub expression: Option<SplitExpressionYaml>,
    #[serde(default = "default_split_aggregation")]
    pub aggregation: String,
    #[serde(default)]
    pub parallel: bool,
    #[serde(default)]
    pub parallel_limit: Option<usize>,
    #[serde(default = "default_true")]
    pub stop_on_exception: bool,
    #[serde(default)]
    pub steps: Vec<RouteDslStep>,
    #[serde(default)]
    pub streaming: bool,
    #[serde(default)]
    pub stream: Option<StreamConfigYaml>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Default, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct StreamConfigYaml {
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub max_record_bytes: Option<usize>,
    #[serde(default)]
    pub batch_size: Option<usize>,
    #[serde(default)]
    pub chunk_size: Option<usize>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
#[cfg_attr(feature = "schema", schemars(untagged))]
pub enum SplitExpressionYaml {
    Simple(String),
    Config(SplitExpressionConfig),
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct SplitExpressionConfig {
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub simple: Option<String>,
    #[serde(default)]
    pub rhai: Option<String>,
    #[serde(default)]
    pub jsonpath: Option<String>,
    #[serde(default)]
    pub xpath: Option<String>,
}

fn default_split_aggregation() -> String {
    "last_wins".to_string()
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct AggregateStep {
    pub aggregate: AggregateData,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct AggregateData {
    pub header: String,
    #[serde(default)]
    pub correlation_key: Option<String>,
    #[serde(default)]
    pub completion_size: Option<usize>,
    #[serde(default)]
    pub completion_timeout_ms: Option<u64>,
    #[serde(default)]
    pub completion_predicate: Option<PredicateBlock>,
    #[serde(default = "default_aggregate_strategy")]
    pub strategy: String,
    #[serde(default)]
    pub max_buckets: Option<usize>,
    #[serde(default)]
    pub bucket_ttl_ms: Option<u64>,
    #[serde(default)]
    pub force_completion_on_stop: Option<bool>,
    #[serde(default)]
    pub discard_on_timeout: Option<bool>,
}

fn default_aggregate_strategy() -> String {
    "collect_all".to_string()
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct WireTapStep {
    pub wire_tap: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct MulticastStep {
    pub multicast: MulticastData,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct MulticastData {
    #[serde(default)]
    pub parallel: bool,
    #[serde(default)]
    pub parallel_limit: Option<usize>,
    #[serde(default)]
    pub stop_on_exception: bool,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default = "default_multicast_aggregation")]
    pub aggregation: String,
    #[serde(default)]
    pub steps: Vec<RouteDslStep>,
}

fn default_multicast_aggregation() -> String {
    "last_wins".to_string()
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct ScatterGatherStep {
    pub scatter_gather: ScatterGatherData,
}

/// Scatter-Gather EIP — stateless parallel fan-out + gather.
/// Lowers to Multicast with aggregation. No correlation key
/// (spec §5 "correlation key" wording corrected: stateless form
/// has none; stateful aggregation is the separate Aggregator EIP).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ScatterGatherData {
    #[serde(default)]
    pub endpoints: Vec<String>,
    #[serde(default = "default_multicast_aggregation")]
    pub aggregation: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct StopStep {
    pub stop: bool,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct StreamCacheStep {
    pub stream_cache: StreamCacheBody,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
#[cfg_attr(feature = "schema", schemars(untagged))]
pub enum StreamCacheBody {
    Enabled(bool),
    Config(StreamCacheConfig),
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct StreamCacheConfig {
    #[serde(default)]
    pub threshold: Option<usize>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct ScriptStep {
    pub script: ScriptData,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct ConvertBodyToStep {
    pub convert_body_to: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct MarshalStep {
    pub marshal: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct UnmarshalStep {
    pub unmarshal: String,
    /// Optional JSON Schema. When present, the compiled UnmarshalService is
    /// wrapped with a `JsonSchemaValidateService` that returns
    /// `CamelError::ValidationError` (→ 400 Bad Request) on a mismatch.
    /// Only effective when the format parses to `Body::Json` (currently `json`
    /// is the only format that produces JSON in v1).
    #[serde(default)]
    pub schema: Option<serde_json::Value>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct ValidateStep {
    pub validate: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ClaimCheckStep {
    pub claim_check: ClaimCheckBody,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ClaimCheckBody {
    /// Name of the registered ClaimCheckRepository (e.g. "memory").
    pub repository: String,
    /// Operation: "set", "get", "get_and_remove", "push", "pop".
    pub operation: String,
    /// Simple-language expression for the claim-check key (e.g. "${header.claimKey}").
    pub key: String,
    /// Optional filter string for selective merge-back during checkout operations.
    /// Grammar: comma-separated tokens with optional `+`/`-`/`--` prefix.
    #[serde(default)]
    pub filter: Option<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct SamplingStep {
    pub sampling: SamplingBody,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
#[cfg_attr(feature = "schema", schemars(untagged))]
pub enum SamplingBody {
    /// Shorthand: `sampling: 5` (period)
    Short(usize),
    /// Full form: `sampling: { period: 5 }`
    Full(SamplingConfig),
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct SamplingConfig {
    pub period: usize,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct SortStep {
    pub sort: SortBody,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct SortBody {
    /// Expression that extracts a sort key from each body element (e.g. simple: "${body.field}").
    pub expression: String,
    /// Reverse (descending) sort. Default false.
    #[serde(default)]
    pub reverse: bool,
    /// Optional language specification (default: "simple").
    #[serde(default)]
    pub language: Option<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct IdempotentConsumerStep {
    pub idempotent_consumer: IdempotentConsumerBody,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct IdempotentConsumerBody {
    /// Name of the registered IdempotentRepository (e.g. `"memory"`).
    pub repository: String,
    /// Simple-language expression that extracts the message-id (e.g. `"${header.messageId}"`).
    pub expression: String,
    /// Child sub-pipeline executed on first-time (non-duplicate) exchanges.
    #[serde(default)]
    pub steps: Vec<RouteDslStep>,
    /// Reserve the key before running the child (default `false`).
    #[serde(default)]
    pub eager: Option<bool>,
    /// When `eager`, remove the key if the child fails (default `false`).
    #[serde(default)]
    pub remove_on_failure: Option<bool>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct EnrichStep {
    pub enrich: EnrichBody,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct PollEnrichStep {
    pub poll_enrich: EnrichBody,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
#[cfg_attr(feature = "schema", schemars(untagged))]
pub enum EnrichBody {
    /// Shorthand: `enrich: "file:..."` or `poll_enrich: "file:..."`
    Uri(String),
    /// Full form: `enrich: { uri: "...", strategy: "...", timeout: ... }`
    Full(EnrichConfig),
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct EnrichConfig {
    pub uri: String,
    #[serde(default)]
    pub strategy: Option<String>,
    #[serde(default)]
    pub timeout: Option<u64>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ScriptData {
    pub language: String,
    pub source: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ThrottleStep {
    pub throttle: ThrottleData,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ThrottleData {
    pub max_requests: usize,
    #[serde(default = "default_throttle_period_secs")]
    pub period_secs: u64,
    #[serde(default)]
    pub strategy: Option<String>,
    #[serde(default)]
    pub steps: Vec<RouteDslStep>,
}

fn default_throttle_period_secs() -> u64 {
    1
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct LoadBalanceStep {
    pub load_balance: LoadBalanceData,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct LoadBalanceData {
    #[serde(default = "default_lb_strategy")]
    pub strategy: String,
    #[serde(default)]
    pub distribution_ratio: Option<String>,
    #[serde(default)]
    pub steps: Vec<RouteDslStep>,
}

fn default_lb_strategy() -> String {
    "round_robin".to_string()
}

// ---------------------------------------------------------------------------
// Template support types (Phase 4)
// ---------------------------------------------------------------------------

/// A reusable route template declared in YAML/JSON.
#[derive(Deserialize, Debug, Clone)]
pub struct RouteDslTemplate {
    pub id: String,
    #[serde(default)]
    pub parameters: Vec<RouteDslTemplateParameter>,
    #[serde(default)]
    pub routes: Vec<serde_yml::Value>,
}

/// A single parameter that a route template accepts.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct RouteDslTemplateParameter {
    /// The parameter name (used inside `{{name}}` placeholders).
    pub name: String,
    /// Optional default value used when the caller does not supply one.
    #[serde(default)]
    pub default_value: Option<String>,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
}

/// A request to instantiate a template with concrete parameter values.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct RouteDslTemplatedRoute {
    pub route_template_ref: String,
    /// Optional explicit route id for the resulting instance.
    #[serde(default)]
    pub route_id: Option<String>,
    /// Concrete parameter values keyed by parameter name.
    #[serde(default)]
    pub parameters: BTreeMap<String, String>,
}

// ---------------------------------------------------------------------------
// REST DSL types (Phase 1)
// ---------------------------------------------------------------------------

/// A top-level REST block (`rest:` in YAML/JSON).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct RouteDslRest {
    /// Listen host address.
    #[serde(default = "default_rest_host")]
    pub host: String,
    /// Listen port.
    #[serde(default = "default_rest_port")]
    pub port: u16,
    /// Base path for all operations in this block (e.g. `/api/users`).
    #[serde(default)]
    pub path: String,
    /// Operations keyed by HTTP verb (`get`, `post`, `put`, `delete`, etc.).
    #[serde(default)]
    pub operations: BTreeMap<String, RouteDslRestOperation>,
}

/// A single REST operation (identified by HTTP verb key in the `operations` map).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct RouteDslRestOperation {
    /// Sub-path relative to the parent RouteDslRest path.
    #[serde(default = "default_operation_path")]
    pub path: String,
    /// Optional unique identifier for this operation.
    #[serde(default)]
    pub operation_id: Option<String>,
    /// Endpoint URI to route matching requests to (e.g. `bean:userService`).
    #[serde(default)]
    pub to: Option<String>,
    /// Child sub-pipeline steps.
    #[serde(default)]
    pub steps: Vec<RouteDslStep>,
    /// Default content-type consumed by this operation.
    #[serde(default = "default_consumes")]
    pub consumes: String,
    /// Default content-type produced by this operation.
    #[serde(default = "default_produces")]
    pub produces: String,
    /// Expected success HTTP status code (e.g. 200, 201).
    #[serde(default)]
    pub success_status: Option<u16>,
    /// Optional request body schema.
    #[serde(default)]
    pub request_schema: Option<serde_json::Value>,
    /// Response definition.
    #[serde(default)]
    pub response: Option<RouteDslRestResponse>,
    /// Optional description of this operation.
    #[serde(default)]
    pub description: Option<String>,
    /// Additional operation parameters.
    #[serde(default)]
    pub parameters: BTreeMap<String, serde_json::Value>,
}

/// Response definition for a REST operation.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct RouteDslRestResponse {
    /// Optional response description.
    #[serde(default)]
    pub description: Option<String>,
    /// Response body schema.
    #[serde(default)]
    pub schema: Option<serde_json::Value>,
    /// Response headers map.
    #[serde(default)]
    pub headers: BTreeMap<String, serde_json::Value>,
}

fn default_rest_host() -> String {
    "0.0.0.0".to_string()
}

fn default_rest_port() -> u16 {
    8080
}

fn default_operation_path() -> String {
    "/".to_string()
}

fn default_consumes() -> String {
    "application/json".to_string()
}

fn default_produces() -> String {
    "application/json".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        assert!(default_true());
        assert_eq!(default_startup_order(), 1000);
        assert_eq!(default_initial_delay_ms(), 100);
        assert_eq!(default_multiplier(), 2.0);
        assert_eq!(default_max_delay_ms(), 10_000);
        assert_eq!(default_jitter_factor(), 0.0);
        assert_eq!(default_failure_threshold(), 5);
        assert_eq!(default_open_duration_ms(), 30_000);
        assert_eq!(default_split_aggregation(), "last_wins");
        assert_eq!(default_aggregate_strategy(), "collect_all");
        assert_eq!(default_multicast_aggregation(), "last_wins");
        assert_eq!(default_throttle_period_secs(), 1);
        assert_eq!(default_lb_strategy(), "round_robin");
        assert_eq!(default_uri_delimiter(), ",");
        assert_eq!(default_cache_size(), 1000);
        assert_eq!(default_max_iterations(), 1000);
        assert_eq!(default_rest_host(), "0.0.0.0");
        assert_eq!(default_rest_port(), 8080);
        assert_eq!(default_operation_path(), "/");
        assert_eq!(default_consumes(), "application/json");
        assert_eq!(default_produces(), "application/json");
    }

    #[test]
    fn parse_minimal_route() {
        let yaml = r#"
routes:
  - id: r1
    from: direct:start
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        assert_eq!(parsed.routes.len(), 1);
        assert_eq!(parsed.routes[0].id, "r1");
        assert_eq!(parsed.routes[0].from, "direct:start");
        assert!(parsed.routes[0].auto_startup);
        assert_eq!(parsed.routes[0].startup_order, 1000);
        assert!(parsed.routes[0].steps.is_empty());
    }

    #[test]
    fn parse_route_with_concurrency() {
        let yaml = r#"
routes:
  - id: r2
    from: timer:tick
    concurrent: 4
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        assert_eq!(parsed.routes[0].concurrent, Some(4));
        assert!(!parsed.routes[0].sequential);
    }

    #[test]
    fn parse_route_with_error_handler() {
        let yaml = r#"
routes:
  - id: r3
    from: direct:in
    error_handler:
      dead_letter_channel: log:dlq
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        let eh = parsed.routes[0].error_handler.as_ref().unwrap();
        assert_eq!(eh.dead_letter_channel.as_deref(), Some("log:dlq"));
    }

    #[test]
    fn parse_route_with_circuit_breaker() {
        let yaml = r#"
routes:
  - id: r4
    from: direct:in
    circuit_breaker:
      failure_threshold: 3
      open_duration_ms: 5000
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        let cb = parsed.routes[0].circuit_breaker.as_ref().unwrap();
        assert_eq!(cb.failure_threshold, 3);
        assert_eq!(cb.open_duration_ms, 5000);
    }

    #[test]
    fn parse_circuit_breaker_defaults() {
        let yaml = r#"
routes:
  - id: r5
    from: direct:in
    circuit_breaker: {}
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        let cb = parsed.routes[0].circuit_breaker.as_ref().unwrap();
        assert_eq!(cb.failure_threshold, 5);
        assert_eq!(cb.open_duration_ms, 30_000);
    }

    #[test]
    fn parse_to_step() {
        let yaml = r#"
routes:
  - id: r6
    from: direct:start
    steps:
      - to: log:out
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        assert_eq!(parsed.routes[0].steps.len(), 1);
    }

    #[test]
    fn parse_delay_short_form() {
        let yaml = r#"
routes:
  - id: r7
    from: direct:start
    steps:
      - delay: 500
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        match &parsed.routes[0].steps[0] {
            RouteDslStep::Delay(d) => match &d.delay {
                DelayBody::Short(ms) => assert_eq!(*ms, 500),
                _ => panic!("expected short form"),
            },
            _ => panic!("expected delay step"),
        }
    }

    #[test]
    fn parse_delay_full_form() {
        let yaml = r#"
routes:
  - id: r8
    from: direct:start
    steps:
      - delay:
          delay_ms: 200
          dynamic_header: X-Delay
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        match &parsed.routes[0].steps[0] {
            RouteDslStep::Delay(d) => match &d.delay {
                DelayBody::Full(cfg) => {
                    assert_eq!(cfg.delay_ms, 200);
                    assert_eq!(cfg.dynamic_header.as_deref(), Some("X-Delay"));
                }
                _ => panic!("expected full form"),
            },
            _ => panic!("expected delay step"),
        }
    }

    #[test]
    fn parse_redelivery_policy_defaults() {
        let yaml = r#"
routes:
  - id: r9
    from: direct:in
    error_handler:
      retry:
        max_attempts: 3
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        let retry = parsed.routes[0]
            .error_handler
            .as_ref()
            .unwrap()
            .retry
            .as_ref()
            .unwrap();
        assert_eq!(retry.max_attempts, 3);
        assert_eq!(retry.initial_delay_ms, 100);
        assert_eq!(retry.multiplier, 2.0);
        assert_eq!(retry.max_delay_ms, 10_000);
        assert_eq!(retry.jitter_factor, 0.0);
    }

    #[test]
    fn parse_stream_cache_bool() {
        let yaml = r#"
routes:
  - id: r10
    from: direct:start
    steps:
      - stream_cache: true
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        match &parsed.routes[0].steps[0] {
            RouteDslStep::StreamCache(s) => match &s.stream_cache {
                StreamCacheBody::Enabled(b) => assert!(*b),
                _ => panic!("expected enabled"),
            },
            _ => panic!("expected stream_cache"),
        }
    }

    #[test]
    fn parse_stop_step() {
        let yaml = r#"
routes:
  - id: r11
    from: direct:start
    steps:
      - stop: true
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        match &parsed.routes[0].steps[0] {
            RouteDslStep::Stop(s) => assert!(s.stop),
            _ => panic!("expected stop"),
        }
    }

    #[test]
    fn parse_convert_body_to() {
        let yaml = r#"
routes:
  - id: r12
    from: direct:start
    steps:
      - convert_body_to: json
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        match &parsed.routes[0].steps[0] {
            RouteDslStep::ConvertBodyTo(s) => assert_eq!(s.convert_body_to, "json"),
            _ => panic!("expected convert_body_to"),
        }
    }

    #[test]
    fn parse_marshal_unmarshal() {
        let yaml = r#"
routes:
  - id: r13
    from: direct:start
    steps:
      - marshal: protobuf
      - unmarshal: protobuf
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        assert_eq!(parsed.routes[0].steps.len(), 2);
    }

    #[test]
    fn parse_bean_step() {
        let yaml = r#"
routes:
  - id: r14
    from: direct:start
    steps:
      - bean:
          name: myBean
          method: handle
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        match &parsed.routes[0].steps[0] {
            RouteDslStep::Bean(b) => {
                assert_eq!(b.bean.name, "myBean");
                assert_eq!(b.bean.method, "handle");
            }
            _ => panic!("expected bean"),
        }
    }

    #[test]
    fn parse_script_step() {
        let yaml = r#"
routes:
  - id: r15
    from: direct:start
    steps:
      - script:
          language: rhai
          source: "1 + 1"
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        match &parsed.routes[0].steps[0] {
            RouteDslStep::Script(s) => {
                assert_eq!(s.script.language, "rhai");
                assert_eq!(s.script.source, "1 + 1");
            }
            _ => panic!("expected script"),
        }
    }

    // --- Template AST tests (Phase 4) ---

    #[test]
    fn parse_yaml_with_templates_and_templated_routes() {
        let yaml = r#"
routes:
  - id: r1
    from: direct:start
templates:
  - id: http-route
    parameters:
      - name: path
        default_value: /api
        description: The REST path
    routes:
      - id: "instance-route"
        from: "rest:{{path}}"
        steps:
          - to: "log:info"
templated_routes:
  - route_template_ref: http-route
    route_id: my-http-route
    parameters:
      path: /users
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        assert_eq!(parsed.routes.len(), 1);
        assert_eq!(parsed.templates.len(), 1);
        assert_eq!(parsed.templated_routes.len(), 1);

        let tpl = &parsed.templates[0];
        assert_eq!(tpl.id, "http-route");
        assert_eq!(tpl.parameters.len(), 1);
        assert_eq!(tpl.parameters[0].name, "path");
        assert_eq!(tpl.parameters[0].default_value.as_deref(), Some("/api"));
        assert_eq!(
            tpl.parameters[0].description.as_deref(),
            Some("The REST path")
        );

        let tr = &parsed.templated_routes[0];
        assert_eq!(tr.route_template_ref, "http-route");
        assert_eq!(tr.route_id.as_deref(), Some("my-http-route"));
        assert_eq!(tr.parameters["path"], "/users");
    }

    #[test]
    fn parse_yaml_backward_compat_no_templates() {
        let yaml = r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - to: log:info
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        assert_eq!(parsed.routes.len(), 1);
        assert!(parsed.templates.is_empty());
        assert!(parsed.templated_routes.is_empty());
    }

    #[test]
    fn parse_yaml_template_with_empty_parameters() {
        let yaml = r#"
routes: []
templates:
  - id: simple-tpl
    routes:
      - id: simple-route
        from: timer:tick
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        assert_eq!(parsed.templates.len(), 1);
        assert!(parsed.templates[0].parameters.is_empty());
    }

    #[test]
    fn parse_yaml_templated_routes_with_defaults() {
        let yaml = r#"
routes: []
templated_routes:
  - route_template_ref: my-tpl
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        assert_eq!(parsed.templated_routes.len(), 1);
        let tr = &parsed.templated_routes[0];
        assert_eq!(tr.route_template_ref, "my-tpl");
        assert!(tr.route_id.is_none());
        assert!(tr.parameters.is_empty());
    }

    // ── REST DSL tests (Phase 1) ──

    #[test]
    fn parse_rest_block_basic() {
        let yaml = r#"
rest:
  - host: 0.0.0.0
    port: 8080
    path: /users
    operations:
      get:
        path: /{id}
        operation_id: getUser
        to: bean:userService
        produces: application/json
      post:
        operation_id: createUser
        consumes: application/json
        produces: application/json
        success_status: 201
        to: bean:createUser
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        assert_eq!(parsed.rest.len(), 1);
        let rest = &parsed.rest[0];
        assert_eq!(rest.host, "0.0.0.0");
        assert_eq!(rest.port, 8080);
        assert_eq!(rest.path, "/users");
        assert_eq!(rest.operations.len(), 2);

        let get_op = rest.operations.get("get").unwrap();
        assert_eq!(get_op.path, "/{id}");
        assert_eq!(get_op.operation_id.as_deref(), Some("getUser"));
        assert_eq!(get_op.to.as_deref(), Some("bean:userService"));
        assert_eq!(get_op.produces, "application/json");

        let post_op = rest.operations.get("post").unwrap();
        assert_eq!(post_op.operation_id.as_deref(), Some("createUser"));
        assert_eq!(post_op.consumes, "application/json");
        assert_eq!(post_op.produces, "application/json");
        assert_eq!(post_op.success_status, Some(201));
        assert_eq!(post_op.to.as_deref(), Some("bean:createUser"));
    }

    #[test]
    fn parse_rest_defaults() {
        let yaml = r#"
rest:
  - path: /api
    operations:
      get:
        to: direct:handler
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        let rest = &parsed.rest[0];
        assert_eq!(rest.host, "0.0.0.0");
        assert_eq!(rest.port, 8080);
        assert_eq!(rest.path, "/api");
        let op = rest.operations.get("get").unwrap();
        assert_eq!(op.path, "/");
        assert_eq!(op.consumes, "application/json");
        assert_eq!(op.produces, "application/json");
        assert!(op.success_status.is_none());
        assert!(op.response.is_none());
        assert!(op.steps.is_empty());
    }

    #[test]
    fn parse_rest_empty() {
        let yaml = r#"
routes:
  - id: r1
    from: direct:start
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        assert!(parsed.rest.is_empty());
    }

    #[test]
    fn parse_rest_multiple_blocks() {
        let yaml = r#"
rest:
  - path: /users
    operations:
      get:
        to: direct:handleUsers
  - path: /orders
    operations:
      get:
        to: direct:handleOrders
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        assert_eq!(parsed.rest.len(), 2);
        assert_eq!(parsed.rest[0].path, "/users");
        assert_eq!(parsed.rest[1].path, "/orders");
    }

    #[test]
    fn parse_rest_backward_compat_no_rest() {
        let yaml = r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - to: log:info
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        assert_eq!(parsed.routes.len(), 1);
        assert!(parsed.rest.is_empty());
    }

    #[test]
    fn parse_rest_and_routes_together() {
        let yaml = r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - to: log:info
rest:
  - host: localhost
    port: 3000
    path: /api
    operations:
      get:
        to: direct:apiHandler
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        assert_eq!(parsed.routes.len(), 1);
        assert_eq!(parsed.rest.len(), 1);
        assert_eq!(parsed.rest[0].host, "localhost");
        assert_eq!(parsed.rest[0].port, 3000);
        assert_eq!(parsed.rest[0].path, "/api");
        assert!(parsed.rest[0].operations.contains_key("get"));
    }

    #[test]
    fn parse_rest_with_response() {
        let yaml = r#"
rest:
  - path: /api
    operations:
      get:
        path: /items
        to: direct:listItems
        response:
          description: A list of items
          schema:
            type: array
            items:
              type: string
          headers:
            X-Total-Count:
              type: integer
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        let rest = &parsed.rest[0];
        let op = rest.operations.get("get").unwrap();
        let resp = op.response.as_ref().unwrap();
        assert_eq!(resp.description.as_deref(), Some("A list of items"));
        assert!(resp.schema.is_some());
        assert!(resp.headers.contains_key("X-Total-Count"));
    }

    #[test]
    fn parse_rest_with_request_schema_and_parameters() {
        let yaml = r#"
rest:
  - path: /api
    operations:
      post:
        path: /create
        to: direct:create
        request_schema:
          type: object
          properties:
            name:
              type: string
        parameters:
          limit:
            type: integer
          offset:
            type: integer
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        let op = parsed.rest[0].operations.get("post").unwrap();
        assert!(op.request_schema.is_some());
        assert!(op.parameters.contains_key("limit"));
        assert!(op.parameters.contains_key("offset"));
    }

    #[test]
    fn parse_rest_multiple_verbs_in_one_block() {
        let yaml = r#"
rest:
  - path: /api
    operations:
      get:
        path: /items
        to: direct:getItems
      post:
        path: /items
        to: direct:createItem
      put:
        path: /items/{id}
        to: direct:updateItem
      delete:
        path: /items/{id}
        to: direct:deleteItem
"#;
        let parsed: RouteDslRoutes = serde_yml::from_str(yaml).unwrap();
        let ops = &parsed.rest[0].operations;
        assert_eq!(ops.len(), 4);
        assert!(ops.contains_key("get"));
        assert!(ops.contains_key("post"));
        assert!(ops.contains_key("put"));
        assert!(ops.contains_key("delete"));
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct DynamicRouterStep {
    pub dynamic_router: DynamicRouterData,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct DynamicRouterData {
    #[serde(default)]
    pub simple: Option<String>,
    #[serde(default)]
    pub rhai: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default = "default_uri_delimiter")]
    pub uri_delimiter: String,
    #[serde(default = "default_cache_size")]
    pub cache_size: i32,
    #[serde(default)]
    pub ignore_invalid_endpoints: bool,
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
}

fn default_uri_delimiter() -> String {
    ",".to_string()
}

fn default_cache_size() -> i32 {
    1000
}

fn default_max_iterations() -> usize {
    1000
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct RoutingSlipStep {
    pub routing_slip: RoutingSlipData,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct RoutingSlipData {
    #[serde(default)]
    pub simple: Option<String>,
    #[serde(default)]
    pub rhai: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default = "default_uri_delimiter")]
    pub uri_delimiter: String,
    #[serde(default = "default_cache_size")]
    pub cache_size: i32,
    #[serde(default)]
    pub ignore_invalid_endpoints: bool,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct RecipientListStep {
    pub recipient_list: RecipientListData,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct RecipientListData {
    #[serde(default)]
    pub simple: Option<String>,
    #[serde(default)]
    pub rhai: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default = "default_uri_delimiter")]
    pub delimiter: String,
    #[serde(default)]
    pub parallel: bool,
    #[serde(default)]
    pub parallel_limit: Option<usize>,
    #[serde(default)]
    pub stop_on_exception: bool,
    #[serde(default)]
    pub strategy: Option<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct BeanStep {
    pub bean: BeanStepData,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct BeanStepData {
    pub name: String,
    pub method: String,
}

// ── Resequence step (Phase 3) ──

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
pub struct ResequenceStep {
    pub resequence: ResequenceData,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ResequenceData {
    #[serde(default)]
    pub batch: Option<ResequenceBatchYaml>,
    #[serde(default)]
    pub stream: Option<ResequenceStreamYaml>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ResequenceBatchYaml {
    pub correlation: String,
    pub sort: String,
    pub completion: ResequenceCompletionYaml,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ResequenceCompletionYaml {
    #[serde(default)]
    pub size: Option<usize>,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub size_or_timeout: Option<Vec<u64>>,
}

// NOTE: ResequenceStreamYaml intentionally omitted — stream resequencing is
// not yet implemented (Task 3). Scaffolding will be added when Stream is ready.

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ResequenceStreamYaml {
    pub sequence: String,
    #[serde(default = "default_stream_capacity")]
    pub capacity: usize,
    #[serde(default = "default_gap_timeout")]
    pub gap_timeout: u64,
    #[serde(default)]
    pub on_gap: StreamGapPolicyYaml,
    #[serde(default)]
    pub on_capacity_exceeded: StreamCapacityPolicyYaml,
    #[serde(default)]
    pub dedup: bool,
}

fn default_stream_capacity() -> usize {
    1000
}

fn default_gap_timeout() -> u64 {
    5000
}

/// Stream resequencer gap policy (YAML representation).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StreamGapPolicyYaml {
    #[default]
    EmitPartial,
    DropAndLog,
}

/// Stream resequencer capacity policy (YAML representation).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[derive(Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StreamCapacityPolicyYaml {
    #[default]
    LogAndDrop,
    DropOldest,
}
