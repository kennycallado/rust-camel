use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;
use tower::ServiceExt;

use camel_api::circuit_breaker::CircuitBreakerConfig;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{
    AggregatorConfig, BoxProcessor, CamelError, Exchange, FilterPredicate, IdentityProcessor,
    MulticastConfig, SplitterConfig,
};
use camel_component::ConcurrencyModel;

use crate::config::DetailLevel;
use crate::tracer::TracingProcessor;

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
}

impl std::fmt::Debug for BuilderStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuilderStep::Processor(_) => write!(f, "BuilderStep::Processor(...)"),
            BuilderStep::To(uri) => write!(f, "BuilderStep::To({uri:?})"),
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

/// Compose a list of BoxProcessors into a single pipeline that runs them sequentially.
pub fn compose_pipeline(processors: Vec<BoxProcessor>) -> BoxProcessor {
    if processors.is_empty() {
        return BoxProcessor::new(IdentityProcessor);
    }
    BoxProcessor::new(SequentialPipeline { steps: processors })
}

/// Compose a list of BoxProcessors into a traced pipeline.
///
/// Each processor is wrapped with TracingProcessor to emit spans for observability.
/// When tracing is disabled, falls back to plain compose_pipeline with zero overhead.
pub fn compose_traced_pipeline(
    processors: Vec<BoxProcessor>,
    route_id: &str,
    trace_enabled: bool,
    detail_level: DetailLevel,
) -> BoxProcessor {
    if !trace_enabled {
        return compose_pipeline(processors);
    }

    if processors.is_empty() {
        return BoxProcessor::new(IdentityProcessor);
    }

    // Wrap each processor with TracingProcessor
    let wrapped: Vec<BoxProcessor> = processors
        .into_iter()
        .enumerate()
        .map(|(idx, processor)| {
            BoxProcessor::new(TracingProcessor::new(
                processor,
                route_id.to_string(),
                idx,
                detail_level.clone(),
            ))
        })
        .collect();

    BoxProcessor::new(SequentialPipeline { steps: wrapped })
}

/// A service that executes a sequence of BoxProcessors in order.
#[derive(Clone)]
struct SequentialPipeline {
    steps: Vec<BoxProcessor>,
}

impl Service<Exchange> for SequentialPipeline {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if let Some(first) = self.steps.first_mut() {
            first.poll_ready(cx)
        } else {
            Poll::Ready(Ok(()))
        }
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let mut steps = self.steps.clone();
        Box::pin(async move {
            let mut ex = exchange;
            for step in &mut steps {
                ex = step.ready().await?.call(ex).await?;
            }
            Ok(ex)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::BoxProcessorExt;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A service that returns `Pending` on the first `poll_ready`, then `Ready`.
    #[derive(Clone)]
    struct DelayedReadyService {
        ready: Arc<AtomicBool>,
    }

    impl Service<Exchange> for DelayedReadyService {
        type Response = Exchange;
        type Error = CamelError;
        type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

        fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            if self.ready.fetch_or(true, Ordering::SeqCst) {
                // Already marked ready (second+ call) → Ready
                Poll::Ready(Ok(()))
            } else {
                // First call → Pending, schedule a wake
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }

        fn call(&mut self, ex: Exchange) -> Self::Future {
            Box::pin(async move { Ok(ex) })
        }
    }

    #[test]
    fn test_pipeline_poll_ready_delegates_to_first_step() {
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        let inner = DelayedReadyService {
            ready: Arc::new(AtomicBool::new(false)),
        };
        let boxed = BoxProcessor::new(inner);
        let mut pipeline = SequentialPipeline { steps: vec![boxed] };

        // First poll_ready: inner returns Pending, so pipeline must too.
        let first = pipeline.poll_ready(&mut cx);
        assert!(first.is_pending(), "expected Pending on first poll_ready");

        // Second poll_ready: inner returns Ready, so pipeline must too.
        let second = pipeline.poll_ready(&mut cx);
        assert!(second.is_ready(), "expected Ready on second poll_ready");
    }

    #[test]
    fn test_pipeline_poll_ready_with_empty_steps() {
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        let mut pipeline = SequentialPipeline { steps: vec![] };

        // Empty pipeline should be immediately ready.
        let result = pipeline.poll_ready(&mut cx);
        assert!(result.is_ready(), "expected Ready for empty pipeline");
    }

    // When a step in the pipeline returns Err(CamelError::Stopped), the pipeline
    // should halt further steps and propagate Err(Stopped). The context loop is
    // responsible for silencing Stopped (treating it as a graceful halt, not an error).
    #[tokio::test]
    async fn test_pipeline_stops_gracefully_on_stopped_error() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        // A flag to detect if a step AFTER stop() was called.
        let after_called = Arc::new(AtomicBool::new(false));
        let after_called_clone = after_called.clone();

        let stop_step = BoxProcessor::from_fn(|_ex| Box::pin(async { Err(CamelError::Stopped) }));
        let after_step = BoxProcessor::from_fn(move |ex| {
            after_called_clone.store(true, Ordering::SeqCst);
            Box::pin(async move { Ok(ex) })
        });

        let mut pipeline = SequentialPipeline {
            steps: vec![stop_step, after_step],
        };

        let ex = Exchange::new(camel_api::Message::new("hello"));
        let result = pipeline.call(ex).await;

        // Pipeline propagates Stopped — callers (context loop) are responsible for silencing it.
        assert!(
            matches!(result, Err(CamelError::Stopped)),
            "expected Err(Stopped), got: {:?}",
            result
        );
        // The step after stop must NOT have been called.
        assert!(
            !after_called.load(Ordering::SeqCst),
            "step after stop should not be called"
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
    fn test_choice_builder_step_debug() {
        use camel_api::{Exchange, FilterPredicate};
        use std::sync::Arc;

        fn always_true(_: &Exchange) -> bool {
            true
        }

        let step = BuilderStep::Choice {
            whens: vec![crate::route::WhenStep {
                predicate: Arc::new(always_true) as FilterPredicate,
                steps: vec![BuilderStep::To("mock:a".into())],
            }],
            otherwise: None,
        };
        let debug = format!("{step:?}");
        assert!(debug.contains("Choice"));
    }

    #[tokio::test]
    async fn test_compose_traced_pipeline_disabled() {
        let pipeline = compose_traced_pipeline(vec![], "test-route", false, DetailLevel::Minimal);
        // Should behave like identity
        let ex = Exchange::new(camel_api::Message::new("hello"));
        let result = tower::ServiceExt::oneshot(pipeline, ex).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_compose_traced_pipeline_enabled() {
        use camel_api::BoxProcessorExt;

        let step = BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }));
        let pipeline =
            compose_traced_pipeline(vec![step], "test-route", true, DetailLevel::Minimal);
        let ex = Exchange::new(camel_api::Message::new("hello"));
        let result = tower::ServiceExt::oneshot(pipeline, ex).await;
        assert!(result.is_ok());
    }
}
