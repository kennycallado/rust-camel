// lifecycle/application/route_definition.rs
// Route definition and builder-step types. Route (compiled artifact) lives in adapters.

use camel_api::circuit_breaker::CircuitBreakerConfig;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::UnitOfWorkConfig;
use camel_api::{AggregatorConfig, BoxProcessor, FilterPredicate, MulticastConfig, SplitterConfig};
use camel_component_api::ConcurrencyModel;

/// An unresolved when-clause: predicate + nested steps for the sub-pipeline.
pub struct WhenStep {
    pub predicate: FilterPredicate,
    pub steps: Vec<BuilderStep>,
}

pub use camel_api::declarative::{LanguageExpressionDef, ValueSourceDef};

/// Declarative `when` clause resolved later by the runtime.
#[derive(Debug)]
pub struct DeclarativeWhenStep {
    pub predicate: LanguageExpressionDef,
    pub steps: Vec<BuilderStep>,
}

/// A step in an unresolved route definition.
pub enum BuilderStep {
    /// A pre-built Tower processor service.
    Processor(BoxProcessor),
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
    /// Declarative split using a language expression, resolved at route-add time.
    DeclarativeSplit {
        expression: LanguageExpressionDef,
        aggregation: camel_api::splitter::AggregationStrategy,
        parallel: bool,
        parallel_limit: Option<usize>,
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
    Delay {
        config: camel_api::DelayConfig,
    },
}

impl std::fmt::Debug for BuilderStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuilderStep::Processor(_) => write!(f, "BuilderStep::Processor(...)"),
            BuilderStep::To(uri) => write!(f, "BuilderStep::To({uri:?})"),
            BuilderStep::Stop => write!(f, "BuilderStep::Stop"),
            BuilderStep::Log { level, message } => write!(
                f,
                "BuilderStep::Log {{ level: {level:?}, message: {message:?} }}"
            ),
            BuilderStep::DeclarativeSetHeader { key, .. } => {
                write!(
                    f,
                    "BuilderStep::DeclarativeSetHeader {{ key: {key:?}, .. }}"
                )
            }
            BuilderStep::DeclarativeSetBody { .. } => {
                write!(f, "BuilderStep::DeclarativeSetBody {{ .. }}")
            }
            BuilderStep::DeclarativeFilter { steps, .. } => {
                write!(
                    f,
                    "BuilderStep::DeclarativeFilter {{ steps: {steps:?}, .. }}"
                )
            }
            BuilderStep::DeclarativeChoice { whens, otherwise } => {
                write!(
                    f,
                    "BuilderStep::DeclarativeChoice {{ whens: {} clause(s), otherwise: {} }}",
                    whens.len(),
                    if otherwise.is_some() { "Some" } else { "None" }
                )
            }
            BuilderStep::DeclarativeScript { expression } => write!(
                f,
                "BuilderStep::DeclarativeScript {{ language: {:?}, .. }}",
                expression.language
            ),
            BuilderStep::DeclarativeSplit { steps, .. } => {
                write!(
                    f,
                    "BuilderStep::DeclarativeSplit {{ steps: {steps:?}, .. }}"
                )
            }
            BuilderStep::DeclarativeDynamicRouter { expression, .. } => write!(
                f,
                "BuilderStep::DeclarativeDynamicRouter {{ language: {:?}, .. }}",
                expression.language
            ),
            BuilderStep::DeclarativeRoutingSlip { expression, .. } => write!(
                f,
                "BuilderStep::DeclarativeRoutingSlip {{ language: {:?}, .. }}",
                expression.language
            ),
            BuilderStep::Split { steps, .. } => {
                write!(f, "BuilderStep::Split {{ steps: {steps:?}, .. }}")
            }
            BuilderStep::Aggregate { .. } => write!(f, "BuilderStep::Aggregate {{ .. }}"),
            BuilderStep::Filter { steps, .. } => {
                write!(f, "BuilderStep::Filter {{ steps: {steps:?}, .. }}")
            }
            BuilderStep::Choice { whens, otherwise } => {
                write!(
                    f,
                    "BuilderStep::Choice {{ whens: {} clause(s), otherwise: {} }}",
                    whens.len(),
                    if otherwise.is_some() { "Some" } else { "None" }
                )
            }
            BuilderStep::WireTap { uri } => write!(f, "BuilderStep::WireTap {{ uri: {uri:?} }}"),
            BuilderStep::Multicast { steps, .. } => {
                write!(f, "BuilderStep::Multicast {{ steps: {steps:?}, .. }}")
            }
            BuilderStep::DeclarativeLog { level, .. } => {
                write!(f, "BuilderStep::DeclarativeLog {{ level: {level:?}, .. }}")
            }
            BuilderStep::Bean { name, method } => {
                write!(
                    f,
                    "BuilderStep::Bean {{ name: {name:?}, method: {method:?} }}"
                )
            }
            BuilderStep::Script { language, .. } => {
                write!(f, "BuilderStep::Script {{ language: {language:?}, .. }}")
            }
            BuilderStep::Throttle { steps, .. } => {
                write!(f, "BuilderStep::Throttle {{ steps: {steps:?}, .. }}")
            }
            BuilderStep::LoadBalance { steps, .. } => {
                write!(f, "BuilderStep::LoadBalance {{ steps: {steps:?}, .. }}")
            }
            BuilderStep::DynamicRouter { .. } => {
                write!(f, "BuilderStep::DynamicRouter {{ .. }}")
            }
            BuilderStep::RoutingSlip { .. } => {
                write!(f, "BuilderStep::RoutingSlip {{ .. }}")
            }
            BuilderStep::Delay { config } => {
                write!(f, "BuilderStep::Delay {{ config: {:?} }}", config)
            }
        }
    }
}

/// An unresolved route definition. "to" URIs have not been resolved to producers yet.
pub struct RouteDefinition {
    pub(crate) from_uri: String,
    pub(crate) steps: Vec<BuilderStep>,
    /// Optional per-route error handler config. Takes precedence over the global one.
    pub(crate) error_handler: Option<ErrorHandlerConfig>,
    /// Optional circuit breaker config. Applied between error handler and step pipeline.
    pub(crate) circuit_breaker: Option<CircuitBreakerConfig>,
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
}

impl RouteDefinition {
    /// Create a new route definition with the required route ID.
    pub fn new(from_uri: impl Into<String>, steps: Vec<BuilderStep>) -> Self {
        Self {
            from_uri: from_uri.into(),
            steps,
            error_handler: None,
            circuit_breaker: None,
            unit_of_work: None,
            concurrency: None,
            route_id: String::new(), // Will be set by with_route_id()
            auto_startup: true,
            startup_order: 1000,
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

    /// Extract the metadata fields needed for introspection.
    /// This is used by RouteController to store route info without the non-Sync steps.
    pub fn to_info(&self) -> RouteDefinitionInfo {
        RouteDefinitionInfo {
            route_id: self.route_id.clone(),
            auto_startup: self.auto_startup,
            startup_order: self.startup_order,
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
        use camel_api::splitter::{split_body_lines, AggregationStrategy, SplitterConfig};
        use camel_api::{
            DynamicRouterConfig, Exchange, IdentityProcessor, RoutingSlipConfig, Value,
        };
        use std::sync::Arc;

        let expr = LanguageExpressionDef {
            language: "simple".into(),
            source: "${body}".into(),
        };

        let steps = vec![
            BuilderStep::Processor(BoxProcessor::new(IdentityProcessor)),
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
                    .build(),
            },
            BuilderStep::Filter {
                predicate: Arc::new(|_: &Exchange| true),
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
        use camel_api::{Exchange, FilterPredicate};
        use std::sync::Arc;

        fn always_true(_: &Exchange) -> bool {
            true
        }

        let step = BuilderStep::Choice {
            whens: vec![WhenStep {
                predicate: Arc::new(always_true) as FilterPredicate,
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
}
