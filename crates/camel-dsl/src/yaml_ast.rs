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
}

#[derive(Deserialize)]
pub struct YamlErrorHandler {
    #[serde(default)]
    pub dead_letter_channel: Option<String>,
    #[serde(default)]
    pub retry: Option<YamlRetryPolicy>,
}

#[derive(Deserialize)]
pub struct YamlRetryPolicy {
    pub max_attempts: u32,
    #[serde(default = "default_initial_delay_ms")]
    pub initial_delay_ms: u64,
    #[serde(default = "default_multiplier")]
    pub multiplier: f64,
    #[serde(default = "default_max_delay_ms")]
    pub max_delay_ms: u64,
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

fn default_failure_threshold() -> u32 {
    5
}

fn default_open_duration_ms() -> u64 {
    30_000
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum YamlStep {
    To(ToStep),
    SetHeader(SetHeaderStep),
    SetBody(SetBodyStep),
    Log(LogStep),
    Filter(FilterStep),
    Choice(ChoiceStep),
    Split(SplitStep),
    Aggregate(AggregateStep),
    WireTap(WireTapStep),
    Multicast(MulticastStep),
    Stop(StopStep),
    Script(ScriptStep),
    ConvertBodyTo(ConvertBodyToStep),
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
}

#[derive(Deserialize, Debug)]
pub struct SetBodyStep {
    pub set_body: SetBodyData,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum SetBodyData {
    Literal(serde_json::Value),
    Config(SetBodyConfig),
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
#[serde(deny_unknown_fields)]
pub struct ScriptData {
    pub language: String,
    pub source: String,
}
