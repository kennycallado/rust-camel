use serde::Deserialize;

#[derive(Deserialize)]
pub struct YamlRoutes {
    pub routes: Vec<YamlRoute>,
}

#[derive(Deserialize)]
pub struct YamlRoute {
    pub id: String,
    pub from: String,
    #[serde(default)]
    pub steps: Vec<YamlStep>,
    #[serde(default = "default_true")]
    pub auto_startup: bool,
    #[serde(default = "default_startup_order")]
    pub startup_order: i32,
    #[serde(default)]
    pub sequential: bool,
    #[serde(default)]
    pub concurrent: Option<usize>,
    #[serde(default)]
    pub error_handler: Option<YamlErrorHandler>,
    #[serde(default)]
    pub circuit_breaker: Option<YamlCircuitBreaker>,
    #[serde(default)]
    pub on_complete: Option<String>,
    #[serde(default)]
    pub on_failure: Option<String>,
}

#[derive(Deserialize)]
pub struct YamlErrorHandler {
    #[serde(default)]
    pub dead_letter_channel: Option<String>,
    #[serde(default)]
    pub retry: Option<YamlRedeliveryPolicy>,
    #[serde(default)]
    pub on_exceptions: Option<Vec<YamlOnException>>,
}

#[derive(Deserialize)]
pub struct YamlOnException {
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub message_contains: Option<String>,
    #[serde(default)]
    pub retry: Option<YamlRedeliveryPolicy>,
}

#[derive(Deserialize)]
pub struct YamlRedeliveryPolicy {
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

#[derive(Deserialize)]
pub struct YamlCircuitBreaker {
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

#[derive(Deserialize, Debug)]
pub struct DelayStep {
    pub delay: DelayBody,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum DelayBody {
    Short(u64),
    Full(DelayFullConfig),
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct DelayFullConfig {
    pub delay_ms: u64,
    #[serde(default)]
    pub dynamic_header: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum YamlStep {
    To(ToStep),
    SetHeader(SetHeaderStep),
    SetBody(SetBodyStep),
    Bean(BeanStep),
    Choice(ChoiceStep),
    DynamicRouter(DynamicRouterStep),
    Filter(FilterStep),
    LoadBalance(LoadBalanceStep),
    Log(LogStep),
    Split(SplitStep),
    Aggregate(AggregateStep),
    WireTap(WireTapStep),
    Multicast(MulticastStep),
    RoutingSlip(RoutingSlipStep),
    RecipientList(RecipientListStep),
    Stop(StopStep),
    Throttle(ThrottleStep),
    Transform(TransformStep),
    Script(ScriptStep),
    ConvertBodyTo(ConvertBodyToStep),
    Marshal(MarshalStep),
    Unmarshal(UnmarshalStep),
    Delay(DelayStep),
}

#[derive(Deserialize, Debug)]
pub struct ToStep {
    pub to: String,
}

#[derive(Deserialize, Debug)]
pub struct SetHeaderStep {
    pub set_header: SetHeaderData,
}

#[derive(Deserialize, Debug)]
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

#[derive(Deserialize, Debug)]
pub struct SetBodyStep {
    pub set_body: SetBodyData,
}

/// `transform:` step — alias for `set_body:`, reuses all SetBodyData types.
#[derive(Deserialize, Debug)]
pub struct TransformStep {
    pub transform: SetBodyData,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum SetBodyData {
    // Config must be tried BEFORE Literal: serde_json::Value matches any YAML value (including
    // objects), so it would greedily swallow `{ simple: "..." }` as a Literal JSON object
    // instead of a SetBodyConfig. By placing Config first, structured forms like
    // `set_body: { simple: "..." }` deserialize correctly.
    Config(SetBodyConfig),
    Literal(serde_json::Value),
}

#[derive(Deserialize, Debug)]
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

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum LogBody {
    Message(String),
    Config(LogConfig),
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct LogConfig {
    /// The log message. Can be a plain string literal or a nested expression object.
    pub message: LogMessageData,
    #[serde(default)]
    pub level: Option<String>,
}

/// The `message` field inside a `log: { message: ... }` config block.
/// Either a bare string literal or a value-source expression (simple, rhai, language+source).
#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum LogMessageData {
    Literal(String),
    Expr(LogMessageExpr),
}

#[derive(Deserialize, Debug)]
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

#[derive(Deserialize, Debug)]
pub struct LogStep {
    pub log: LogBody,
}

#[derive(Deserialize, Debug)]
pub struct FilterStep {
    pub filter: PredicateBlock,
}

#[derive(Deserialize, Debug)]
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
    pub steps: Vec<YamlStep>,
}

#[derive(Deserialize, Debug)]
pub struct ChoiceStep {
    pub choice: ChoiceData,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct ChoiceData {
    #[serde(default)]
    pub when: Vec<PredicateBlock>,
    #[serde(default)]
    pub otherwise: Option<Vec<YamlStep>>,
}

#[derive(Deserialize, Debug)]
pub struct SplitStep {
    pub split: SplitData,
}

#[derive(Deserialize, Debug)]
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
    pub steps: Vec<YamlStep>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum SplitExpressionYaml {
    Simple(String),
    Config(SplitExpressionConfig),
}

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

#[derive(Deserialize, Debug)]
pub struct AggregateStep {
    pub aggregate: AggregateData,
}

#[derive(Deserialize, Debug)]
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

#[derive(Deserialize, Debug)]
pub struct WireTapStep {
    pub wire_tap: String,
}

#[derive(Deserialize, Debug)]
pub struct MulticastStep {
    pub multicast: MulticastData,
}

#[derive(Deserialize, Debug)]
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
    pub steps: Vec<YamlStep>,
}

fn default_multicast_aggregation() -> String {
    "last_wins".to_string()
}

#[derive(Deserialize, Debug)]
pub struct StopStep {
    pub stop: bool,
}

#[derive(Deserialize, Debug)]
pub struct ScriptStep {
    pub script: ScriptData,
}

#[derive(Deserialize, Debug)]
pub struct ConvertBodyToStep {
    pub convert_body_to: String,
}

#[derive(Deserialize, Debug)]
pub struct MarshalStep {
    pub marshal: String,
}

#[derive(Deserialize, Debug)]
pub struct UnmarshalStep {
    pub unmarshal: String,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct ScriptData {
    pub language: String,
    pub source: String,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct ThrottleStep {
    pub throttle: ThrottleData,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct ThrottleData {
    pub max_requests: usize,
    #[serde(default = "default_throttle_period_secs")]
    pub period_secs: u64,
    #[serde(default)]
    pub strategy: Option<String>,
    #[serde(default)]
    pub steps: Vec<YamlStep>,
}

fn default_throttle_period_secs() -> u64 {
    1
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct LoadBalanceStep {
    pub load_balance: LoadBalanceData,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct LoadBalanceData {
    #[serde(default = "default_lb_strategy")]
    pub strategy: String,
    #[serde(default)]
    pub distribution_ratio: Option<String>,
    #[serde(default)]
    pub parallel: bool,
    #[serde(default)]
    pub steps: Vec<YamlStep>,
}

fn default_lb_strategy() -> String {
    "round_robin".to_string()
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct DynamicRouterStep {
    pub dynamic_router: DynamicRouterData,
}

#[derive(Deserialize, Debug)]
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

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct RoutingSlipStep {
    pub routing_slip: RoutingSlipData,
}

#[derive(Deserialize, Debug)]
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

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct RecipientListStep {
    pub recipient_list: RecipientListData,
}

#[derive(Deserialize, Debug)]
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

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct BeanStep {
    pub bean: BeanStepData,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct BeanStepData {
    pub name: String,
    pub method: String,
}
