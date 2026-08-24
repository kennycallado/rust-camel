// lifecycle/application/route_definition.rs
// Route definition and builder-step types. Route (compiled artifact) lives in adapters.

use std::sync::Arc;

use camel_api::UnitOfWorkConfig;
use camel_api::circuit_breaker::CircuitBreakerConfig;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::loop_eip::LoopConfig;
use camel_api::security_policy::SecurityPolicyConfig;
use camel_api::{
    AggregatorConfig, FilterPredicate, MulticastConfig, OpaqueProcessor, ResequencePolicyConfig,
    SplitterConfig,
};
use camel_auth::TokenAuthenticator;
use camel_component_api::ConcurrencyModel;

/// An unresolved when-clause: predicate + nested steps for the sub-pipeline.
#[derive(Clone)]
pub struct WhenStep {
    pub predicate: FilterPredicate,
    pub steps: Vec<BuilderStep>,
}

impl std::fmt::Debug for WhenStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WhenStep")
            .field("predicate", &self.predicate)
            .field("steps", &self.steps)
            .finish()
    }
}

pub use camel_api::declarative::{LanguageExpressionDef, ValueSourceDef};

/// Declarative `when` clause resolved later by the runtime.
#[derive(Debug, Clone)]
pub struct DeclarativeWhenStep {
    pub predicate: LanguageExpressionDef,
    pub steps: Vec<BuilderStep>,
}

/// Builder struct for a single `doCatch` clause in the declarative pipeline.
#[derive(Debug, Clone)]
pub struct DoTryCatchClauseBuilder {
    pub exception: Option<Vec<String>>,
    pub when: Option<LanguageExpressionDef>,
    pub on_when: Option<LanguageExpressionDef>,
    pub disposition: camel_api::error_handler::ExceptionDisposition,
    pub steps: Vec<BuilderStep>,
}

/// Builder struct for the `doFinally` block in the declarative pipeline.
#[derive(Debug, Clone)]
pub struct DoTryFinallyBuilder {
    pub on_when: Option<LanguageExpressionDef>,
    pub steps: Vec<BuilderStep>,
}

/// A step in an unresolved route definition.
#[derive(Debug, Clone)]
pub enum BuilderStep {
    /// A pre-built Tower processor service.
    Processor(OpaqueProcessor),
    /// A destination URI — resolved at start time by CamelContext.
    To(String),
    /// A stop step that halts processing immediately.
    Stop,
    /// A static log step.
    Log {
        level: camel_processor::LogLevel,
        message: String,
    },
    /// Declarative set_header (literal or language-based value), resolved at route-add time.
    DeclarativeSetHeader {
        key: String,
        value: ValueSourceDef,
    },
    /// Declarative set_header_if_absent (internal-only), resolved at route-add time.
    DeclarativeSetHeaderIfAbsent {
        key: String,
        value: ValueSourceDef,
    },
    /// Declarative remove_header, resolved at route-add time.
    DeclarativeRemoveHeader {
        key: String,
    },
    DeclarativeSetProperty {
        key: String,
        value_source: ValueSourceDef,
    },
    /// Declarative set_body (literal or language-based value), resolved at route-add time.
    DeclarativeSetBody {
        value: ValueSourceDef,
    },
    /// Declarative filter using a language predicate, resolved at route-add time.
    DeclarativeFilter {
        predicate: LanguageExpressionDef,
        steps: Vec<BuilderStep>,
    },
    /// Declarative choice/when/otherwise using language predicates, resolved at route-add time.
    DeclarativeChoice {
        whens: Vec<DeclarativeWhenStep>,
        otherwise: Option<Vec<BuilderStep>>,
    },
    /// Declarative script step evaluated by language and written to body.
    DeclarativeScript {
        expression: LanguageExpressionDef,
    },
    DeclarativeFunction {
        definition: camel_api::FunctionDefinition,
    },
    /// Declarative split using a language expression, resolved at route-add time.
    DeclarativeSplit {
        expression: LanguageExpressionDef,
        aggregation: camel_api::splitter::AggregationStrategy,
        parallel: bool,
        parallel_limit: Option<usize>,
        stop_on_exception: bool,
        steps: Vec<BuilderStep>,
    },
    /// Declarative stream split using a streaming split expression, resolved at route-add time.
    DeclarativeStreamSplit {
        stream_config: camel_api::StreamSplitConfig,
        aggregation: camel_api::splitter::AggregationStrategy,
        stop_on_exception: bool,
        steps: Vec<BuilderStep>,
    },
    DeclarativeDynamicRouter {
        expression: LanguageExpressionDef,
        uri_delimiter: String,
        cache_size: i32,
        ignore_invalid_endpoints: bool,
        max_iterations: usize,
    },
    DeclarativeRoutingSlip {
        expression: LanguageExpressionDef,
        uri_delimiter: String,
        cache_size: i32,
        ignore_invalid_endpoints: bool,
    },
    /// A Splitter sub-pipeline: config + nested steps to execute per fragment.
    Split {
        config: SplitterConfig,
        steps: Vec<BuilderStep>,
    },
    /// An Aggregator step: collects exchanges by correlation key, emits when complete.
    Aggregate {
        config: AggregatorConfig,
    },
    /// A Filter sub-pipeline: predicate + nested steps executed only when predicate is true.
    Filter {
        predicate: FilterPredicate,
        steps: Vec<BuilderStep>,
    },
    /// A Choice step: evaluates when-clauses in order, routes to the first match.
    /// If no when matches, the optional otherwise branch is used.
    Choice {
        whens: Vec<WhenStep>,
        otherwise: Option<Vec<BuilderStep>>,
    },
    /// A WireTap step: sends a clone of the exchange to a tap endpoint (fire-and-forget).
    WireTap {
        uri: String,
    },
    /// A Multicast step: sends the same exchange to multiple destinations.
    Multicast {
        steps: Vec<BuilderStep>,
        config: MulticastConfig,
    },
    /// Declarative log step with a language-evaluated message, resolved at route-add time.
    DeclarativeLog {
        level: camel_processor::LogLevel,
        message: ValueSourceDef,
    },
    /// Bean invocation step — resolved at route-add time.
    Bean {
        name: String,
        method: String,
    },
    /// Script step: executes a script that can mutate the exchange.
    /// The script has access to `headers`, `properties`, and `body`.
    Script {
        language: String,
        script: String,
    },
    /// Throttle step: rate limiting with configurable behavior when limit exceeded.
    Throttle {
        config: camel_api::ThrottlerConfig,
        steps: Vec<BuilderStep>,
    },
    /// LoadBalance step: distributes exchanges across multiple endpoints using a strategy.
    LoadBalance {
        config: camel_api::LoadBalancerConfig,
        steps: Vec<BuilderStep>,
    },
    /// DynamicRouter step: routes exchanges dynamically based on expression evaluation.
    DynamicRouter {
        config: camel_api::DynamicRouterConfig,
    },
    RoutingSlip {
        config: camel_api::RoutingSlipConfig,
    },
    RecipientList {
        config: camel_api::recipient_list::RecipientListConfig,
    },
    DeclarativeRecipientList {
        expression: LanguageExpressionDef,
        delimiter: String,
        parallel: bool,
        parallel_limit: Option<usize>,
        stop_on_exception: bool,
        aggregation: String,
    },
    Delay {
        config: camel_api::DelayConfig,
    },
    /// Runtime loop with closure-based predicate (programmatic DSL).
    Loop {
        config: LoopConfig,
        steps: Vec<BuilderStep>,
    },
    /// Declarative loop with optional language-based while predicate (YAML DSL).
    DeclarativeLoop {
        count: Option<usize>,
        while_predicate: Option<LanguageExpressionDef>,
        steps: Vec<BuilderStep>,
        max_iterations: Option<usize>,
    },
    /// EIP-7 enrich: synchronous content enrichment via a resolved producer.
    Enrich {
        uri: String,
        strategy: Option<String>,
        timeout_ms: Option<u64>,
    },
    /// EIP-7 pollEnrich: blocking poll of a PollingConsumer with timeout.
    PollEnrich {
        uri: String,
        strategy: Option<String>,
        timeout_ms: Option<u64>,
    },
    /// Validate step: evaluates a language expression as predicate.
    /// Exchange passes if predicate returns true; else CamelError::ValidationError.
    Validate {
        predicate: LanguageExpressionDef,
    },
    /// Claim Check step (EIP). Transforms the exchange body to/from a
    /// `ClaimCheckRepository` by key. Process-mode, no child pipeline.
    /// `filter` enables selective merge-back of body/headers during checkout.
    ClaimCheck {
        repository: String,
        operation: String,
        key: LanguageExpressionDef,
        filter: Option<String>,
    },
    /// Sampling step (EIP). Passes 1 of every N exchanges (counter-based,
    /// deterministic). Non-sampled exchanges get CamelStop=true (drop semantics).
    /// Process-mode, stateless. No StepLifecycle — counter is route-scoped.
    Sampling {
        period: usize,
    },
    /// Sort step (EIP). Orders a body array by extracting a sort key
    /// from each element via a language expression. Process-mode, stateless.
    Sort {
        expression: LanguageExpressionDef,
        reverse: bool,
    },
    /// Idempotent Consumer step (EIP). Wraps a child sub-pipeline that runs
    /// only when the message-id is NOT present in the named repository.
    /// Compiled to a `IdempotentConsumerSegment` (OutcomePipeline, segment-mode).
    IdempotentConsumer {
        repository: String,
        expression: LanguageExpressionDef,
        steps: Vec<BuilderStep>,
        eager: bool,
        remove_on_failure: bool,
    },
    /// Cache step (EIP). Wraps a child sub-pipeline that runs only on cache
    /// miss; on hit the stored body is reconstructed and the child is skipped.
    /// Compiled to a `CacheService` (OutcomePipeline, segment-mode).
    Cache {
        repository: Option<String>,
        key: LanguageExpressionDef,
        ttl: Option<String>,
        max_entry_bytes: Option<usize>,
        /// Coalesce concurrent misses on the same key into one `on_miss` run.
        coalesce_misses: bool,
        on_miss: Vec<BuilderStep>,
    },
    /// Cache Invalidate step (EIP). Removes a single cache entry by exact key
    /// (`key`) or every entry under a namespace prefix (`key_prefix`) — exactly
    /// one of the two. Compiled to a `CacheInvalidateService` (OutcomePipeline,
    /// segment-mode).
    CacheInvalidate {
        repository: Option<String>,
        key: Option<LanguageExpressionDef>,
        key_prefix: Option<LanguageExpressionDef>,
    },
    /// Cache Clear step (EIP). Removes all entries from the repository.
    /// Compiled to a `CacheClearService` (OutcomePipeline, segment-mode).
    CacheClear {
        repository: Option<String>,
    },
    /// Cache Stats step (EIP). Emits the repository stats as a JSON body.
    /// Compiled to a `CacheStatsService` (OutcomePipeline, segment-mode).
    CacheStats {
        repository: Option<String>,
    },
    /// Cache Peek Stale step (EIP). Serves a stale (post-expiry) cache entry.
    /// Compiled to a `CachePeekStaleService` (OutcomePipeline, segment-mode).
    CachePeekStale {
        repository: Option<String>,
        key: LanguageExpressionDef,
        on_miss: camel_processor::PeekStaleMissPolicy,
    },
    /// Declarative doTry/doCatch/doFinally, resolved at route-add time.
    DeclarativeDoTry {
        try_steps: Vec<BuilderStep>,
        catch: Vec<DoTryCatchClauseBuilder>,
        finally: Option<DoTryFinallyBuilder>,
    },
    /// Resequencer EIP: resequences exchanges by sequence number.
    /// Must be a top-level step (not nested inside structural EIPs).
    Resequence {
        policy_config: ResequencePolicyConfig,
    },
}

/// An unresolved route definition. "to" URIs have not been resolved to producers yet.
pub struct RouteDefinition {
    pub(crate) from_uri: String,
    pub(crate) steps: Vec<BuilderStep>,
    /// Optional per-route error handler config. Takes precedence over the global one.
    pub(crate) error_handler: Option<ErrorHandlerConfig>,
    /// Optional circuit breaker config. Applied between error handler and step pipeline.
    pub(crate) circuit_breaker: Option<CircuitBreakerConfig>,
    /// Circuit breaker fallback sub-pipeline, as UNRESOLVED steps.
    ///
    /// Sibling of [`RouteDefinition::circuit_breaker`]: the DSL layers thread
    /// `Vec<BuilderStep>` here (same rule as `cache_peek_stale.on_miss`) and
    /// camel-core compiles it via the `StepCompilerRegistry` when the route is
    /// compiled. The resolved `CircuitBreakerConfig.fallback` (`BoxProcessor`)
    /// stays `None` until that compile — the DSL never constructs processors.
    pub(crate) circuit_breaker_fallback: Vec<BuilderStep>,
    pub(crate) security_policy: Option<SecurityPolicyConfig>,
    /// Optional token authenticator for validating JWT/OAuth tokens.
    pub(crate) security_authenticator: Option<Arc<dyn TokenAuthenticator>>,
    /// Named authentication providers for this route, injected into the
    /// consumer `SecurityContext` so Phase-2 transports can resolve them.
    pub(crate) provider_registry: Option<Arc<camel_auth::ProviderRegistry>>,
    /// Declared provider name for plan compilation (`security_policy.provider`
    /// in the DSL). Resolved against the provider registry at staging time;
    /// a missing name fails compilation instead of downgrading to `Public`.
    pub(crate) security_provider: Option<String>,
    /// Route-level audience override for plan compilation. When present the
    /// compiled `RouteSecurityPlan.audience_binding` uses these audiences
    /// (issuers still come from the provider); when absent the resolved
    /// provider's binding is copied verbatim.
    pub(crate) security_audiences: Option<Vec<String>>,
    /// Optional Unit of Work config for in-flight tracking and completion hooks.
    pub(crate) unit_of_work: Option<UnitOfWorkConfig>,
    /// User override for the consumer's concurrency model. `None` means
    /// "use whatever the consumer declares".
    pub(crate) concurrency: Option<ConcurrencyModel>,
    /// Unique identifier for this route. Required.
    pub(crate) route_id: String,
    /// Whether this route should start automatically when the context starts.
    pub(crate) auto_startup: bool,
    /// Order in which routes are started. Lower values start first.
    pub(crate) startup_order: i32,
    pub(crate) source_hash: Option<u64>,
}

impl RouteDefinition {
    /// Create a new route definition with the required route ID.
    pub fn new(from_uri: impl Into<String>, steps: Vec<BuilderStep>) -> Self {
        Self {
            from_uri: from_uri.into(),
            steps,
            error_handler: None,
            circuit_breaker: None,
            circuit_breaker_fallback: Vec::new(),
            security_policy: None,
            security_authenticator: None,
            provider_registry: None,
            security_provider: None,
            security_audiences: None,
            unit_of_work: None,
            concurrency: None,
            route_id: String::new(), // Will be set by with_route_id()
            auto_startup: true,
            startup_order: 1000,
            source_hash: None,
        }
    }

    /// The source endpoint URI.
    pub fn from_uri(&self) -> &str {
        &self.from_uri
    }

    /// The steps in this route definition.
    pub fn steps(&self) -> &[BuilderStep] {
        &self.steps
    }

    pub fn circuit_breaker_fallback(&self) -> &[BuilderStep] {
        &self.circuit_breaker_fallback
    }

    /// Transform the step list, consuming and returning self.
    /// Used for post-parse instrumentation (e.g. benchmark timing injection
    /// around `To` steps). The field stays private; callers rebuild via this
    /// consuming method rather than holding a `&mut` alias.
    pub fn map_steps(mut self, f: impl FnOnce(Vec<BuilderStep>) -> Vec<BuilderStep>) -> Self {
        self.steps = f(self.steps);
        self
    }

    /// Set a per-route error handler, overriding the global one.
    pub fn with_error_handler(mut self, config: ErrorHandlerConfig) -> Self {
        self.error_handler = Some(config);
        self
    }

    /// Get the route-level error handler config, if set.
    pub fn error_handler_config(&self) -> Option<&ErrorHandlerConfig> {
        self.error_handler.as_ref()
    }

    /// Set a circuit breaker for this route.
    pub fn with_circuit_breaker(mut self, config: CircuitBreakerConfig) -> Self {
        self.circuit_breaker = Some(config);
        self
    }

    /// Set the circuit breaker fallback sub-pipeline (unresolved steps).
    ///
    /// Mirror of [`RouteDefinition::with_circuit_breaker`]: the steps are
    /// compiled by camel-core at route-compile time (registry monopoly) and
    /// attached to the resolved `CircuitBreakerConfig.fallback`. Empty when no
    /// CB is configured or the CB has no fallback.
    pub fn with_circuit_breaker_fallback(mut self, steps: Vec<BuilderStep>) -> Self {
        self.circuit_breaker_fallback = steps;
        self
    }

    /// Set a security policy for this route.
    pub fn with_security_policy(mut self, config: SecurityPolicyConfig) -> Self {
        self.security_policy = Some(config);
        self
    }

    /// Set a token authenticator for this route.
    pub fn with_security_authenticator(
        mut self,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        self.security_authenticator = Some(authenticator);
        self
    }

    /// Set the named authentication providers for this route.
    ///
    /// The registry is injected into the consumer `SecurityContext` so
    /// Phase-2 transports can resolve providers without holding their own
    /// authenticator.
    pub fn with_provider_registry(mut self, registry: Arc<camel_auth::ProviderRegistry>) -> Self {
        self.provider_registry = Some(registry);
        self
    }

    /// Set the declared provider name used by plan compilation.
    pub fn with_security_provider(mut self, name: impl Into<String>) -> Self {
        self.security_provider = Some(name.into());
        self
    }

    /// Set the route-level audience override used by plan compilation.
    pub fn with_security_audiences(mut self, audiences: Vec<String>) -> Self {
        self.security_audiences = Some(audiences);
        self
    }

    /// The declared provider name, if any.
    pub fn security_provider(&self) -> Option<&str> {
        self.security_provider.as_deref()
    }

    /// The route-level audience override, if any.
    pub fn security_audiences(&self) -> Option<&[String]> {
        self.security_audiences.as_deref()
    }

    /// Set a unit of work config for this route.
    pub fn with_unit_of_work(mut self, config: UnitOfWorkConfig) -> Self {
        self.unit_of_work = Some(config);
        self
    }

    /// Get the unit of work config, if set.
    pub fn unit_of_work_config(&self) -> Option<&UnitOfWorkConfig> {
        self.unit_of_work.as_ref()
    }

    /// Get the circuit breaker config, if set.
    pub fn circuit_breaker_config(&self) -> Option<&CircuitBreakerConfig> {
        self.circuit_breaker.as_ref()
    }

    pub fn security_policy_config(&self) -> Option<&SecurityPolicyConfig> {
        self.security_policy.as_ref()
    }

    pub fn security_authenticator(&self) -> Option<&Arc<dyn TokenAuthenticator>> {
        self.security_authenticator.as_ref()
    }

    /// User-specified concurrency override, if any.
    pub fn concurrency_override(&self) -> Option<&ConcurrencyModel> {
        self.concurrency.as_ref()
    }

    /// Override the consumer's concurrency model for this route.
    pub fn with_concurrency(mut self, model: ConcurrencyModel) -> Self {
        self.concurrency = Some(model);
        self
    }

    /// Get the route ID.
    pub fn route_id(&self) -> &str {
        &self.route_id
    }

    /// Whether this route should start automatically when the context starts.
    pub fn auto_startup(&self) -> bool {
        self.auto_startup
    }

    /// Order in which routes are started. Lower values start first.
    pub fn startup_order(&self) -> i32 {
        self.startup_order
    }

    /// Set a unique identifier for this route.
    pub fn with_route_id(mut self, id: impl Into<String>) -> Self {
        self.route_id = id.into();
        self
    }

    /// Set whether this route should start automatically.
    pub fn with_auto_startup(mut self, auto: bool) -> Self {
        self.auto_startup = auto;
        self
    }

    /// Set the startup order. Lower values start first.
    pub fn with_startup_order(mut self, order: i32) -> Self {
        self.startup_order = order;
        self
    }

    pub fn with_source_hash(mut self, hash: u64) -> Self {
        self.source_hash = Some(hash);
        self
    }

    pub fn source_hash(&self) -> Option<u64> {
        self.source_hash
    }

    /// Extract the metadata fields needed for introspection.
    /// This is used by RouteController to store route info without the non-Sync steps.
    pub fn to_info(&self) -> RouteDefinitionInfo {
        RouteDefinitionInfo {
            route_id: self.route_id.clone(),
            auto_startup: self.auto_startup,
            startup_order: self.startup_order,
            source_hash: self.source_hash,
        }
    }
}

/// Minimal route definition metadata for introspection.
///
/// This struct contains only the metadata fields from [`RouteDefinition`]
/// that are needed for route lifecycle management, without the `steps` field
/// (which contains non-Sync types and cannot be stored in a Sync struct).
#[derive(Clone)]
pub struct RouteDefinitionInfo {
    route_id: String,
    auto_startup: bool,
    startup_order: i32,
    pub(crate) source_hash: Option<u64>,
}

impl RouteDefinitionInfo {
    /// Get the route ID.
    pub fn route_id(&self) -> &str {
        &self.route_id
    }

    /// Whether this route should start automatically when the context starts.
    pub fn auto_startup(&self) -> bool {
        self.auto_startup
    }

    /// Order in which routes are started. Lower values start first.
    pub fn startup_order(&self) -> i32 {
        self.startup_order
    }

    pub fn source_hash(&self) -> Option<u64> {
        self.source_hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden debug output for EVERY BuilderStep variant.
    ///
    /// Before refactoring the manual `Debug` impl to `#[derive(Debug)]`, this test
    /// locks the exact output of every variant so the refactor can be verified.
    #[test]
    fn golden_debug_output_all_variants() {
        use camel_api::declarative::LanguageExpressionDef;
        use camel_api::loop_eip::LoopMode;
        use camel_api::recipient_list::RecipientListConfig;
        use camel_api::splitter::{AggregationStrategy, StreamSplitConfig, StreamSplitFormat};
        use camel_api::{
            BoxProcessor, DynamicRouterConfig, Exchange, FilterPredicate, FunctionDefinition,
            FunctionId, IdentityProcessor, MulticastConfig, OpaqueProcessor, RoutingSlipConfig,
            Value,
        };
        use std::sync::Arc;

        let expr = LanguageExpressionDef {
            language: "simple".into(),
            source: "${body}".into(),
        };

        // -- group A: trivial / non-recursive ----------------------------------
        // derive(Debug) emits the variant without the enum name for unit/tuple variants.
        assert_eq!(format!("{:?}", BuilderStep::Stop), "Stop");
        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::Processor(OpaqueProcessor(BoxProcessor::new(IdentityProcessor)))
            ),
            "Processor(BoxProcessor(...))"
        );
        assert_eq!(
            format!("{:?}", BuilderStep::To("mock:out".into())),
            "To(\"mock:out\")"
        );

        // -- group B: variant with named fields (no sub-steps) -----------------
        // derive(Debug) emits ALL fields, not the redacted `..` form.
        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::Log {
                    level: camel_processor::LogLevel::Info,
                    message: "hello".into(),
                }
            ),
            "Log { level: Info, message: \"hello\" }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::DeclarativeSetHeader {
                    key: "k".into(),
                    value: ValueSourceDef::Literal(Value::String("v".into())),
                }
            ),
            "DeclarativeSetHeader { key: \"k\", value: Literal(String(\"v\")) }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::DeclarativeSetHeaderIfAbsent {
                    key: "k".into(),
                    value: ValueSourceDef::Literal(Value::String("v".into())),
                }
            ),
            "DeclarativeSetHeaderIfAbsent { key: \"k\", value: Literal(String(\"v\")) }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::DeclarativeSetBody {
                    value: ValueSourceDef::Literal(Value::String("v".into())),
                }
            ),
            "DeclarativeSetBody { value: Literal(String(\"v\")) }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::DeclarativeSetProperty {
                    key: "prop".into(),
                    value_source: ValueSourceDef::Literal(Value::String("v".into())),
                }
            ),
            "DeclarativeSetProperty { key: \"prop\", value_source: Literal(String(\"v\")) }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::DeclarativeScript {
                    expression: expr.clone(),
                }
            ),
            "DeclarativeScript { expression: LanguageExpressionDef { language: \"simple\", source: \"${body}\" } }"
        );

        // DeclarativeFunction needs a FunctionDefinition
        let func_def = FunctionDefinition {
            id: FunctionId("test-id".into()),
            runtime: "my_runtime".into(),
            source: "${body}".into(),
            timeout_ms: 5000,
            route_id: None,
            step_index: None,
        };
        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::DeclarativeFunction {
                    definition: func_def,
                }
            ),
            "DeclarativeFunction { definition: FunctionDefinition { id: FunctionId(\"test-id\"), runtime: \"my_runtime\", source: \"${body}\", timeout_ms: 5000, route_id: None, step_index: None } }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::WireTap {
                    uri: "mock:tap".into(),
                }
            ),
            "WireTap { uri: \"mock:tap\" }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::DeclarativeLog {
                    level: camel_processor::LogLevel::Info,
                    message: ValueSourceDef::Expression(expr.clone()),
                }
            ),
            "DeclarativeLog { level: Info, message: Expression(LanguageExpressionDef { language: \"simple\", source: \"${body}\" }) }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::Bean {
                    name: "myBean".into(),
                    method: "process".into(),
                }
            ),
            "Bean { name: \"myBean\", method: \"process\" }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::Script {
                    language: "js".into(),
                    script: "body".into(),
                }
            ),
            "Script { language: \"js\", script: \"body\" }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::Aggregate {
                    config: camel_api::AggregatorConfig::correlate_by("id")
                        .complete_when_size(1)
                        .build()
                        .unwrap(),
                }
            ),
            "Aggregate { config: AggregatorConfig { header_name: \"id\", completion: Single(Size(1)), correlation: HeaderName(\"id\"), strategy: CollectAll, max_buckets: Some(10000), bucket_ttl: Some(300s), force_completion_on_stop: false, discard_on_timeout: false, max_timeout_tasks: 1024 } }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::DynamicRouter {
                    config: DynamicRouterConfig::new(Arc::new(|_: &Exchange| Some(
                        "mock:dr".into()
                    ))),
                }
            ),
            "DynamicRouter { config: DynamicRouterConfig { uri_delimiter: \",\", cache_size: 1000, ignore_invalid_endpoints: false, max_iterations: 1000, timeout: Some(60s) } }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::RoutingSlip {
                    config: RoutingSlipConfig::new(Arc::new(|_: &Exchange| Some("mock:rs".into()))),
                }
            ),
            "RoutingSlip { config: RoutingSlipConfig { uri_delimiter: \",\", cache_size: 1000, ignore_invalid_endpoints: false } }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::RecipientList {
                    config: RecipientListConfig::new(Arc::new(|_: &Exchange| String::new())),
                }
            ),
            "RecipientList { config: RecipientListConfig { delimiter: \",\", parallel: false, parallel_limit: None, stop_on_exception: false, max_recipients: 1000 } }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::Enrich {
                    uri: "mock:enrich".into(),
                    strategy: Some("agg".into()),
                    timeout_ms: Some(1000),
                }
            ),
            "Enrich { uri: \"mock:enrich\", strategy: Some(\"agg\"), timeout_ms: Some(1000) }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::PollEnrich {
                    uri: "mock:poll".into(),
                    strategy: None,
                    timeout_ms: None,
                }
            ),
            "PollEnrich { uri: \"mock:poll\", strategy: None, timeout_ms: None }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::Validate {
                    predicate: expr.clone(),
                }
            ),
            "Validate { predicate: LanguageExpressionDef { language: \"simple\", source: \"${body}\" } }"
        );

        assert_eq!(
            format!("{:?}", BuilderStep::Sampling { period: 100 }),
            "Sampling { period: 100 }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::Resequence {
                    policy_config: Default::default(),
                }
            ),
            "Resequence { policy_config: ResequencePolicyConfig { mode: Batch { correlation: \"header.id\", sort: \"header.id\", completion: SizeOrTimeout(100, 30000) } } }"
        );

        // -- group C: variants with named fields (sub-steps is Vec) ------------
        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::DeclarativeFilter {
                    predicate: expr.clone(),
                    steps: vec![BuilderStep::Stop],
                }
            ),
            "DeclarativeFilter { predicate: LanguageExpressionDef { language: \"simple\", source: \"${body}\" }, steps: [Stop] }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::DeclarativeSplit {
                    expression: expr.clone(),
                    aggregation: AggregationStrategy::Original,
                    parallel: false,
                    parallel_limit: Some(2),
                    stop_on_exception: true,
                    steps: vec![BuilderStep::Stop],
                }
            ),
            "DeclarativeSplit { expression: LanguageExpressionDef { language: \"simple\", source: \"${body}\" }, aggregation: Original, parallel: false, parallel_limit: Some(2), stop_on_exception: true, steps: [Stop] }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::Split {
                    config: camel_api::splitter::SplitterConfig::new(
                        camel_api::splitter::split_body_lines()
                    ),
                    steps: vec![BuilderStep::Stop],
                }
            ),
            "Split { config: SplitterConfig { expression: \"<split-expression>\", aggregation: LastWins, parallel: false, parallel_limit: None, stop_on_exception: true, max_fragments: 100000 }, steps: [Stop] }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::Filter {
                    predicate: FilterPredicate::new(|_: &Exchange| true),
                    steps: vec![BuilderStep::Stop],
                }
            ),
            "Filter { predicate: FilterPredicate(..), steps: [Stop] }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::Throttle {
                    config: camel_api::ThrottlerConfig::new(
                        10,
                        std::time::Duration::from_millis(10)
                    ),
                    steps: vec![BuilderStep::Stop],
                }
            ),
            "Throttle { config: ThrottlerConfig { max_requests: 10, period: 10ms, strategy: Delay }, steps: [Stop] }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::LoadBalance {
                    config: camel_api::LoadBalancerConfig::round_robin(),
                    steps: vec![BuilderStep::To("mock:l1".into())],
                }
            ),
            "LoadBalance { config: LoadBalancerConfig { strategy: RoundRobin }, steps: [To(\"mock:l1\")] }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::Delay {
                    config: camel_api::DelayConfig::new(500),
                }
            ),
            "Delay { config: DelayConfig { delay_ms: 500, dynamic_header: None, max_delay_ms: 3600000 } }"
        );

        // -- group D: Choice / DeclarativeChoice --------------------------------
        // derive(Debug) emits full field enumeration; WhenStep Debug is a
        // full struct listing; nested BuilderStep::Stop stays as `Stop`.
        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::Choice {
                    whens: vec![WhenStep {
                        predicate: FilterPredicate::new(|_: &Exchange| true),
                        steps: vec![BuilderStep::To("mock:a".into())],
                    }],
                    otherwise: None,
                }
            ),
            "Choice { whens: [WhenStep { predicate: FilterPredicate(..), steps: [To(\"mock:a\")] }], otherwise: None }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::DeclarativeChoice {
                    whens: vec![DeclarativeWhenStep {
                        predicate: expr.clone(),
                        steps: vec![BuilderStep::Stop],
                    }],
                    otherwise: Some(vec![BuilderStep::Stop]),
                }
            ),
            "DeclarativeChoice { whens: [DeclarativeWhenStep { predicate: LanguageExpressionDef { language: \"simple\", source: \"${body}\" }, steps: [Stop] }], otherwise: Some([Stop]) }"
        );

        // -- group E: Multicast -------------------------------------------------
        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::Multicast {
                    steps: vec![BuilderStep::To("direct:a".into())],
                    config: MulticastConfig::new(),
                }
            ),
            "Multicast { steps: [To(\"direct:a\")], config: MulticastConfig { parallel: false, parallel_limit: None, stop_on_exception: false, timeout: None, aggregation: LastWins } }"
        );

        // -- group F: DeclarativeDynamicRouter / DeclarativeRoutingSlip --------
        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::DeclarativeDynamicRouter {
                    expression: expr.clone(),
                    uri_delimiter: ",".into(),
                    cache_size: 1000,
                    ignore_invalid_endpoints: false,
                    max_iterations: 1000,
                }
            ),
            "DeclarativeDynamicRouter { expression: LanguageExpressionDef { language: \"simple\", source: \"${body}\" }, uri_delimiter: \",\", cache_size: 1000, ignore_invalid_endpoints: false, max_iterations: 1000 }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::DeclarativeRoutingSlip {
                    expression: expr.clone(),
                    uri_delimiter: ",".into(),
                    cache_size: 1000,
                    ignore_invalid_endpoints: false,
                }
            ),
            "DeclarativeRoutingSlip { expression: LanguageExpressionDef { language: \"simple\", source: \"${body}\" }, uri_delimiter: \",\", cache_size: 1000, ignore_invalid_endpoints: false }"
        );

        // -- group G: DeclarativeRecipientList ---------------------------------
        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::DeclarativeRecipientList {
                    expression: expr.clone(),
                    delimiter: ",".into(),
                    parallel: false,
                    parallel_limit: None,
                    stop_on_exception: false,
                    aggregation: "original".into(),
                }
            ),
            "DeclarativeRecipientList { expression: LanguageExpressionDef { language: \"simple\", source: \"${body}\" }, delimiter: \",\", parallel: false, parallel_limit: None, stop_on_exception: false, aggregation: \"original\" }"
        );

        // -- group H: Loop / DeclarativeLoop -----------------------------------
        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::Loop {
                    config: camel_api::loop_eip::LoopConfig::new(LoopMode::Count(3)),
                    steps: vec![],
                }
            ),
            "Loop { config: LoopConfig { mode: Count(3), max_iterations: 10000 }, steps: [] }"
        );

        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::DeclarativeLoop {
                    count: Some(5),
                    while_predicate: None,
                    steps: vec![],
                    max_iterations: Some(100),
                }
            ),
            "DeclarativeLoop { count: Some(5), while_predicate: None, steps: [], max_iterations: Some(100) }"
        );

        // -- group I: ClaimCheck -----------------------------------------------
        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::ClaimCheck {
                    repository: "myRepo".into(),
                    operation: "checkout".into(),
                    key: expr.clone(),
                    filter: None,
                }
            ),
            "ClaimCheck { repository: \"myRepo\", operation: \"checkout\", key: LanguageExpressionDef { language: \"simple\", source: \"${body}\" }, filter: None }"
        );

        // -- group J: Sort -----------------------------------------------------
        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::Sort {
                    expression: expr.clone(),
                    reverse: false,
                }
            ),
            "Sort { expression: LanguageExpressionDef { language: \"simple\", source: \"${body}\" }, reverse: false }"
        );

        // -- group K: IdempotentConsumer ---------------------------------------
        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::IdempotentConsumer {
                    repository: "myRepo".into(),
                    expression: expr.clone(),
                    steps: vec![],
                    eager: true,
                    remove_on_failure: false,
                }
            ),
            "IdempotentConsumer { repository: \"myRepo\", expression: LanguageExpressionDef { language: \"simple\", source: \"${body}\" }, steps: [], eager: true, remove_on_failure: false }"
        );

        // -- group L: DeclarativeDoTry -----------------------------------------
        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::DeclarativeDoTry {
                    try_steps: vec![BuilderStep::Stop],
                    catch: vec![],
                    finally: None,
                }
            ),
            "DeclarativeDoTry { try_steps: [Stop], catch: [], finally: None }"
        );

        // -- group M: DeclarativeStreamSplit -----------------------------------
        assert_eq!(
            format!(
                "{:?}",
                BuilderStep::DeclarativeStreamSplit {
                    stream_config: StreamSplitConfig {
                        format: StreamSplitFormat::Ndjson,
                        max_record_bytes: 1024 * 1024,
                        batch_size: 1,
                        chunk_size: None,
                        include_origin: true,
                    },
                    aggregation: AggregationStrategy::Original,
                    stop_on_exception: true,
                    steps: vec![BuilderStep::Stop],
                }
            ),
            "DeclarativeStreamSplit { stream_config: StreamSplitConfig { format: Ndjson, max_record_bytes: 1048576, batch_size: 1, chunk_size: None, include_origin: true }, aggregation: Original, stop_on_exception: true, steps: [Stop] }"
        );
    }

    #[test]
    fn test_builder_step_multicast_variant() {
        use camel_api::MulticastConfig;

        let step = BuilderStep::Multicast {
            steps: vec![BuilderStep::To("direct:a".into())],
            config: MulticastConfig::new(),
        };

        assert!(matches!(step, BuilderStep::Multicast { .. }));
    }

    #[test]
    fn test_route_definition_defaults() {
        let def = RouteDefinition::new("direct:test", vec![]).with_route_id("test-route");
        assert_eq!(def.route_id(), "test-route");
        assert!(def.auto_startup());
        assert_eq!(def.startup_order(), 1000);
    }

    #[test]
    fn test_route_definition_builders() {
        let def = RouteDefinition::new("direct:test", vec![])
            .with_route_id("my-route")
            .with_auto_startup(false)
            .with_startup_order(50);
        assert_eq!(def.route_id(), "my-route");
        assert!(!def.auto_startup());
        assert_eq!(def.startup_order(), 50);
    }

    #[test]
    fn test_route_definition_accessors_cover_core_fields() {
        let def = RouteDefinition::new("direct:in", vec![BuilderStep::To("mock:out".into())])
            .with_route_id("accessor-route");

        assert_eq!(def.from_uri(), "direct:in");
        assert_eq!(def.steps().len(), 1);
        assert!(matches!(def.steps()[0], BuilderStep::To(_)));
    }

    #[test]
    fn test_route_definition_error_handler_circuit_breaker_and_concurrency_accessors() {
        use camel_api::circuit_breaker::CircuitBreakerConfig;
        use camel_api::error_handler::ErrorHandlerConfig;
        use camel_component_api::ConcurrencyModel;

        let def = RouteDefinition::new("direct:test", vec![])
            .with_route_id("eh-route")
            .with_error_handler(ErrorHandlerConfig::dead_letter_channel("log:dlc"))
            .with_circuit_breaker(CircuitBreakerConfig::new())
            .with_concurrency(ConcurrencyModel::Concurrent { max: Some(4) });

        let eh = def
            .error_handler_config()
            .expect("error handler should be set");
        assert_eq!(eh.dlc_uri.as_deref(), Some("log:dlc"));
        assert!(def.circuit_breaker_config().is_some());
        assert!(matches!(
            def.concurrency_override(),
            Some(ConcurrencyModel::Concurrent { max: Some(4) })
        ));
    }

    #[test]
    fn test_builder_step_debug_covers_many_variants() {
        use camel_api::splitter::{AggregationStrategy, SplitterConfig, split_body_lines};
        use camel_api::{
            BoxProcessor, DynamicRouterConfig, Exchange, FilterPredicate, IdentityProcessor,
            OpaqueProcessor, RoutingSlipConfig, Value,
        };
        use std::sync::Arc;

        let expr = LanguageExpressionDef {
            language: "simple".into(),
            source: "${body}".into(),
        };

        let steps = vec![
            BuilderStep::Processor(OpaqueProcessor(BoxProcessor::new(IdentityProcessor))),
            BuilderStep::To("mock:out".into()),
            BuilderStep::Stop,
            BuilderStep::Log {
                level: camel_processor::LogLevel::Info,
                message: "hello".into(),
            },
            BuilderStep::DeclarativeSetHeader {
                key: "k".into(),
                value: ValueSourceDef::Literal(Value::String("v".into())),
            },
            BuilderStep::DeclarativeSetBody {
                value: ValueSourceDef::Expression(expr.clone()),
            },
            BuilderStep::DeclarativeFilter {
                predicate: expr.clone(),
                steps: vec![BuilderStep::Stop],
            },
            BuilderStep::DeclarativeChoice {
                whens: vec![DeclarativeWhenStep {
                    predicate: expr.clone(),
                    steps: vec![BuilderStep::Stop],
                }],
                otherwise: Some(vec![BuilderStep::Stop]),
            },
            BuilderStep::DeclarativeScript {
                expression: expr.clone(),
            },
            BuilderStep::DeclarativeSplit {
                expression: expr.clone(),
                aggregation: AggregationStrategy::Original,
                parallel: false,
                parallel_limit: Some(2),
                stop_on_exception: true,
                steps: vec![BuilderStep::Stop],
            },
            BuilderStep::Split {
                config: SplitterConfig::new(split_body_lines()),
                steps: vec![BuilderStep::Stop],
            },
            BuilderStep::Aggregate {
                config: camel_api::AggregatorConfig::correlate_by("id")
                    .complete_when_size(1)
                    .build()
                    .unwrap(),
            },
            BuilderStep::Filter {
                predicate: FilterPredicate::new(|_: &Exchange| true),
                steps: vec![BuilderStep::Stop],
            },
            BuilderStep::WireTap {
                uri: "mock:tap".into(),
            },
            BuilderStep::DeclarativeLog {
                level: camel_processor::LogLevel::Info,
                message: ValueSourceDef::Expression(expr.clone()),
            },
            BuilderStep::Bean {
                name: "bean".into(),
                method: "call".into(),
            },
            BuilderStep::Script {
                language: "rhai".into(),
                script: "body".into(),
            },
            BuilderStep::Throttle {
                config: camel_api::ThrottlerConfig::new(10, std::time::Duration::from_millis(10)),
                steps: vec![BuilderStep::Stop],
            },
            BuilderStep::LoadBalance {
                config: camel_api::LoadBalancerConfig::round_robin(),
                steps: vec![BuilderStep::To("mock:l1".into())],
            },
            BuilderStep::DynamicRouter {
                config: DynamicRouterConfig::new(Arc::new(|_| Some("mock:dr".into()))),
            },
            BuilderStep::RoutingSlip {
                config: RoutingSlipConfig::new(Arc::new(|_| Some("mock:rs".into()))),
            },
        ];

        for step in steps {
            let dbg = format!("{step:?}");
            assert!(!dbg.is_empty());
        }
    }

    #[test]
    fn test_route_definition_to_info_preserves_metadata() {
        let info = RouteDefinition::new("direct:test", vec![])
            .with_route_id("meta-route")
            .with_auto_startup(false)
            .with_startup_order(7)
            .to_info();

        assert_eq!(info.route_id(), "meta-route");
        assert!(!info.auto_startup());
        assert_eq!(info.startup_order(), 7);
    }

    #[test]
    fn test_choice_builder_step_debug() {
        use camel_api::FilterPredicate;

        fn always_true(_: &camel_api::Exchange) -> bool {
            true
        }

        let step = BuilderStep::Choice {
            whens: vec![WhenStep {
                predicate: FilterPredicate::new(always_true),
                steps: vec![BuilderStep::To("mock:a".into())],
            }],
            otherwise: None,
        };
        let debug = format!("{step:?}");
        assert!(debug.contains("Choice"));
    }

    #[test]
    fn test_route_definition_unit_of_work() {
        use camel_api::UnitOfWorkConfig;
        let config = UnitOfWorkConfig {
            on_complete: Some("log:complete".into()),
            on_failure: Some("log:failed".into()),
        };
        let def = RouteDefinition::new("direct:test", vec![])
            .with_route_id("uow-test")
            .with_unit_of_work(config.clone());
        assert_eq!(
            def.unit_of_work_config().unwrap().on_complete.as_deref(),
            Some("log:complete")
        );
        assert_eq!(
            def.unit_of_work_config().unwrap().on_failure.as_deref(),
            Some("log:failed")
        );

        let def_no_uow = RouteDefinition::new("direct:test", vec![]).with_route_id("no-uow");
        assert!(def_no_uow.unit_of_work_config().is_none());
    }

    #[test]
    fn test_route_definition_security_policy_accessor() {
        use async_trait::async_trait;
        use camel_api::CamelError;
        use camel_api::Exchange;
        use camel_api::security_policy::{
            AuthContext, AuthorizationDecision, Principal, SecurityPolicy, SecurityPolicyConfig,
        };

        struct StubPolicy;
        #[async_trait]
        impl SecurityPolicy for StubPolicy {
            async fn evaluate(
                &self,
                _exchange: &mut Exchange,
                _auth: &AuthContext<'_>,
            ) -> Result<AuthorizationDecision, CamelError> {
                Ok(AuthorizationDecision::Granted {
                    principal: Principal {
                        subject: "test".into(),
                        issuer: "test".into(),
                        audience: vec![],
                        scopes: vec![],
                        roles: vec![],
                        claims: serde_json::Value::Null,
                    },
                })
            }
        }

        let def_no_sp = RouteDefinition::new("direct:test", vec![]).with_route_id("no-sp");
        assert!(def_no_sp.security_policy_config().is_none());

        let def = RouteDefinition::new("direct:test", vec![])
            .with_route_id("sp-test")
            .with_security_policy(SecurityPolicyConfig::new(StubPolicy));
        assert!(def.security_policy_config().is_some());
    }

    #[test]
    fn test_route_definition_security_authenticator_accessor() {
        use camel_api::security_policy::Principal;

        struct TestAuth;
        #[async_trait::async_trait]
        impl TokenAuthenticator for TestAuth {
            async fn authenticate_bearer(
                &self,
                _token: &str,
            ) -> Result<Principal, camel_api::CamelError> {
                Ok(Principal {
                    subject: "test".into(),
                    issuer: "test".into(),
                    audience: vec![],
                    scopes: vec![],
                    roles: vec![],
                    claims: serde_json::Value::Null,
                })
            }
        }

        let def_no_auth = RouteDefinition::new("direct:test".to_string(), vec![]);
        assert!(def_no_auth.security_authenticator().is_none());

        let auth = Arc::new(TestAuth);
        let def = RouteDefinition::new("direct:test".to_string(), vec![])
            .with_security_authenticator(auth);
        assert!(def.security_authenticator().is_some());
    }

    #[test]
    fn test_map_steps_swaps_steps_and_preserves_other_fields() {
        let original = RouteDefinition::new(
            "direct:test".to_string(),
            vec![BuilderStep::To("mock:a".into()), BuilderStep::Stop],
        )
        .with_route_id("my-route");

        let mapped = original.map_steps(|steps| {
            let mut out = Vec::with_capacity(steps.len() + 1);
            out.push(BuilderStep::To("mock:prefix".into()));
            out.extend(steps);
            out
        });

        // Steps were transformed.
        assert_eq!(mapped.steps().len(), 3);
        assert!(matches!(mapped.steps()[0], BuilderStep::To(ref s) if s == "mock:prefix"));
        assert!(matches!(mapped.steps()[1], BuilderStep::To(ref s) if s == "mock:a"));
        // Other fields preserved.
        assert_eq!(mapped.route_id(), "my-route");
    }

    #[test]
    fn circuit_breaker_fallback_accessor_returns_steps() {
        let def = RouteDefinition::new("direct:start", vec![])
            .with_circuit_breaker_fallback(vec![BuilderStep::To("mock:out".into())]);
        assert_eq!(def.circuit_breaker_fallback().len(), 1);
    }
}
