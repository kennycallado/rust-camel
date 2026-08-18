pub use camel_api::{LanguageExpressionDef, StreamSplitConfig, ValueSourceDef};

use crate::route_ast::CredentialSourceDsl;

#[derive(Default)]
pub struct SecurityCompileContext {
    pub providers:
        std::collections::HashMap<String, std::sync::Arc<dyn camel_auth::TokenAuthenticator>>,
    pub registry: Option<std::sync::Arc<camel_auth::SecurityPolicyRegistry>>,
    pub evaluator_registry: Option<std::sync::Arc<camel_auth::PermissionEvaluatorRegistry>>,
}

impl Clone for SecurityCompileContext {
    fn clone(&self) -> Self {
        Self {
            providers: self.providers.clone(),
            registry: self.registry.clone(),
            evaluator_registry: self.evaluator_registry.clone(),
        }
    }
}

impl SecurityCompileContext {
    /// `Some(a)` registers `a` as a named provider under the reserved name
    /// `"default"`; `None` leaves the provider map empty.
    pub fn new(
        authenticator: Option<std::sync::Arc<dyn camel_auth::TokenAuthenticator>>,
        registry: Option<std::sync::Arc<camel_auth::SecurityPolicyRegistry>>,
    ) -> Self {
        let mut providers = std::collections::HashMap::new();
        if let Some(auth) = authenticator {
            providers.insert("default".to_string(), auth);
        }
        Self {
            providers,
            registry,
            evaluator_registry: None,
        }
    }

    /// Register a named authenticator provider.
    pub fn with_named_authenticator(
        mut self,
        name: &str,
        auth: std::sync::Arc<dyn camel_auth::TokenAuthenticator>,
    ) -> Self {
        self.providers.insert(name.to_string(), auth);
        self
    }

    /// Resolve the authenticator for a route.
    ///
    /// `None` resolves to the sole registered provider when exactly one is
    /// registered, and to `None` when the map is empty. More than one provider
    /// requires an explicit provider name.
    pub fn authenticator_for(
        &self,
        name: Option<&str>,
    ) -> Result<Option<std::sync::Arc<dyn camel_auth::TokenAuthenticator>>, String> {
        match name {
            None => match self.providers.len() {
                0 => Ok(None),
                1 => Ok(self.providers.values().next().cloned()),
                _ => Err(format!(
                    "multiple authenticators configured: {}; route must declare security_policy.provider",
                    self.sorted_provider_names()
                )),
            },
            Some(n) => match self.providers.get(n) {
                Some(auth) => Ok(Some(auth.clone())),
                None => Err(format!(
                    "unknown provider: {}; available: {}",
                    n,
                    self.sorted_provider_names()
                )),
            },
        }
    }

    fn sorted_provider_names(&self) -> String {
        let mut names: Vec<&str> = self.providers.keys().map(|s| s.as_str()).collect();
        names.sort_unstable();
        names.join(", ")
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

#[derive(Debug, Clone, PartialEq)]
pub struct DeclarativeCircuitBreaker {
    pub failure_threshold: u32,
    pub open_duration_ms: u64,
    pub fallback: Vec<DeclarativeStep>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeclarativeSecurityPolicy {
    Roles {
        roles: Vec<String>,
        all_required: bool,
        trust_upstream_principal: bool,
        credential_sources: Option<Vec<CredentialSourceDsl>>,
        provider: Option<String>,
    },
    Scopes {
        scopes: Vec<String>,
        all_required: bool,
        trust_upstream_principal: bool,
        credential_sources: Option<Vec<CredentialSourceDsl>>,
        provider: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveHeaderStepDef {
    pub key: String,
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

/// Cache EIP step definition.
///
/// Looks up/puts data in a `CacheRepository` by key, running `on_miss`
/// when the key is absent.
#[derive(Debug, Clone, PartialEq)]
pub struct CacheStepDef {
    /// Name of the registered `CacheRepository` (optional; defaults to system default).
    pub repository: Option<String>,
    /// Expression that extracts the cache key from the exchange.
    pub key: LanguageExpressionDef,
    /// Optional TTL as a string expression (e.g. "60s", "5m").
    pub ttl: Option<String>,
    /// Maximum bytes per cache entry.
    pub max_entry_bytes: Option<usize>,
    /// Coalesce concurrent misses on the same key into a single `on_miss` run.
    pub coalesce_misses: bool,
    /// Child sub-pipeline executed on cache miss.
    pub on_miss: Vec<DeclarativeStep>,
}

/// Cache Invalidate EIP step definition.
///
/// Removes an entry from a `CacheRepository` by exact key or namespace prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheInvalidateStepDef {
    /// Name of the registered `CacheRepository` (optional; defaults to system default).
    pub repository: Option<String>,
    /// Expression that extracts the exact cache key to invalidate.
    /// Mutually exclusive with `key_prefix` (exactly one of the two).
    pub key: Option<LanguageExpressionDef>,
    /// Expression that extracts the namespace prefix to invalidate.
    /// Mutually exclusive with `key` (exactly one of the two).
    pub key_prefix: Option<LanguageExpressionDef>,
}

/// Cache Peek Stale EIP step definition.
///
/// Returns cached data even if TTL has expired (graceful degradation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachePeekStaleStepDef {
    /// Name of the registered `CacheRepository` (optional; defaults to system default).
    pub repository: Option<String>,
    /// Expression that extracts the cache key to peek.
    pub key: LanguageExpressionDef,
    /// On-miss policy: `"stop"` (default) or `"continue"`, validated at compile.
    pub on_miss: Option<String>,
}

/// Cache Clear EIP step definition.
///
/// Removes all entries from a `CacheRepository`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheClearStepDef {
    /// Name of the registered `CacheRepository` (optional; defaults to system default).
    pub repository: Option<String>,
}

/// Cache Stats EIP step definition.
///
/// Emits the repository statistics as a JSON body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheStatsStepDef {
    /// Name of the registered `CacheRepository` (optional; defaults to system default).
    pub repository: Option<String>,
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
    RemoveHeader(RemoveHeaderStepDef),
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
    Cache(CacheStepDef),
    CacheInvalidate(CacheInvalidateStepDef),
    CachePeekStale(CachePeekStaleStepDef),
    CacheClear(CacheClearStepDef),
    CacheStats(CacheStatsStepDef),
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
            DeclarativeStep::RemoveHeader(_) => crate::contract::DeclarativeStepKind::RemoveHeader,
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
            DeclarativeStep::Cache(_) => crate::contract::DeclarativeStepKind::Cache,
            DeclarativeStep::CacheInvalidate(_) => {
                crate::contract::DeclarativeStepKind::CacheInvalidate
            }
            DeclarativeStep::CachePeekStale(_) => {
                crate::contract::DeclarativeStepKind::CachePeekStale
            }
            DeclarativeStep::CacheClear(_) => crate::contract::DeclarativeStepKind::CacheClear,
            DeclarativeStep::CacheStats(_) => crate::contract::DeclarativeStepKind::CacheStats,
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
    fn remove_header_kind_returns_correct_variant() {
        let step = DeclarativeStep::RemoveHeader(RemoveHeaderStepDef {
            key: "X-Foo".into(),
        });
        assert_eq!(
            step.kind(),
            crate::contract::DeclarativeStepKind::RemoveHeader
        );
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
            fallback: vec![],
        };
        assert_eq!(cb.failure_threshold, 3);
        assert_eq!(cb.open_duration_ms, 5000);
    }

    mod provider_registry {
        use super::*;
        use crate::test_support::test_authenticator;
        use std::sync::Arc;

        #[test]
        fn sole_provider_resolves_without_name() {
            let ctx = SecurityCompileContext::default()
                .with_named_authenticator("native", test_authenticator("test-user"));
            let resolved = ctx.authenticator_for(None).unwrap();
            assert!(resolved.is_some());
        }

        #[test]
        fn legacy_constructor_registers_default() {
            let auth = test_authenticator("test-user");
            let ctx = SecurityCompileContext::new(Some(auth.clone()), None);
            let unnamed = ctx
                .authenticator_for(None)
                .unwrap()
                .expect("expected resolved authenticator");
            assert!(Arc::ptr_eq(&auth, &unnamed));
            let named = ctx
                .authenticator_for(Some("default"))
                .unwrap()
                .expect("expected default-named authenticator");
            assert!(Arc::ptr_eq(&auth, &named));
        }

        #[test]
        fn constructor_registered_and_named_are_ambiguous() {
            let legacy = test_authenticator("legacy-user");
            let named = test_authenticator("named-user");
            let ctx = SecurityCompileContext::new(Some(legacy.clone()), None)
                .with_named_authenticator("native", named.clone());
            let err = ctx.authenticator_for(None).err().expect("expected error");
            assert!(
                err.contains("multiple authenticators configured"),
                "got: {err}"
            );
            assert!(err.contains("default"), "got: {err}");
            assert!(err.contains("native"), "got: {err}");
        }

        #[test]
        fn multiple_providers_require_name() {
            let ctx = SecurityCompileContext::default()
                .with_named_authenticator("native", test_authenticator("native-user"))
                .with_named_authenticator("oidc", test_authenticator("oidc-user"));
            let err = ctx.authenticator_for(None).err().expect("expected error");
            assert!(err.contains("native"));
            assert!(err.contains("oidc"));
        }

        #[test]
        fn unknown_provider_errors() {
            let ctx = SecurityCompileContext::default()
                .with_named_authenticator("native", test_authenticator("native-user"))
                .with_named_authenticator("oidc", test_authenticator("oidc-user"));
            let err = ctx
                .authenticator_for(Some("saml"))
                .err()
                .expect("expected error");
            assert!(err.contains("saml"));
            assert!(err.contains("native"));
            assert!(err.contains("oidc"));
        }
    }
}
