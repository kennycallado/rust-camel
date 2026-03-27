// application/route_types.rs
// Route types that require Tower framework types.
// Lives in application/ because BuilderStep::Processor(BoxProcessor) requires tower::Service,
// which cannot live in the pure domain layer.

use camel_api::circuit_breaker::CircuitBreakerConfig;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{AggregatorConfig, BoxProcessor, FilterPredicate, MulticastConfig, SplitterConfig};
use camel_component::ConcurrencyModel;

/// A Route defines a message flow: from a source endpoint, through a composed
/// Tower Service pipeline.
pub struct Route {
    /// The source endpoint URI.
    pub(crate) from_uri: String,
    /// The composed processor pipeline as a type-erased Tower Service.
    pub(crate) pipeline: BoxProcessor,
    /// Optional per-route concurrency model override.
    /// When `None`, the consumer's default concurrency model is used.
    pub(crate) concurrency: Option<ConcurrencyModel>,
}

impl Route {
    /// Create a new route from the given source URI and processor pipeline.
    pub fn new(from_uri: impl Into<String>, pipeline: BoxProcessor) -> Self {
        Self {
            from_uri: from_uri.into(),
            pipeline,
            concurrency: None,
        }
    }

    /// The source endpoint URI.
    pub fn from_uri(&self) -> &str {
        &self.from_uri
    }

    /// Consume the route and return its pipeline.
    pub fn into_pipeline(self) -> BoxProcessor {
        self.pipeline
    }

    /// Set a concurrency model override for this route.
    pub fn with_concurrency(mut self, model: ConcurrencyModel) -> Self {
        self.concurrency = Some(model);
        self
    }

    /// Get the concurrency model override, if any.
    pub fn concurrency_override(&self) -> Option<&ConcurrencyModel> {
        self.concurrency.as_ref()
    }

    /// Consume the route, returning the pipeline and optional concurrency override.
    pub fn into_parts(self) -> (BoxProcessor, Option<ConcurrencyModel>) {
        (self.pipeline, self.concurrency)
    }
}

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
    DeclarativeSetHeader { key: String, value: ValueSourceDef },
    /// Declarative set_body (literal or language-based value), resolved at route-add time.
    DeclarativeSetBody { value: ValueSourceDef },
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
    DeclarativeScript { expression: LanguageExpressionDef },
    /// Declarative split using a language expression, resolved at route-add time.
    DeclarativeSplit {
        expression: LanguageExpressionDef,
        aggregation: camel_api::splitter::AggregationStrategy,
        parallel: bool,
        parallel_limit: Option<usize>,
        stop_on_exception: bool,
        steps: Vec<BuilderStep>,
    },
    /// A Splitter sub-pipeline: config + nested steps to execute per fragment.
    Split {
        config: SplitterConfig,
        steps: Vec<BuilderStep>,
    },
    /// An Aggregator step: collects exchanges by correlation key, emits when complete.
    Aggregate { config: AggregatorConfig },
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
    WireTap { uri: String },
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
    Bean { name: String, method: String },
    /// Script step: executes a script that can mutate the exchange.
    /// The script has access to `headers`, `properties`, and `body`.
    Script { language: String, script: String },
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

    /// Set a circuit breaker for this route.
    pub fn with_circuit_breaker(mut self, config: CircuitBreakerConfig) -> Self {
        self.circuit_breaker = Some(config);
        self
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
}
