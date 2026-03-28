//! Default implementation of RouteController.
//!
//! This module provides [`DefaultRouteController`], which manages route lifecycle
//! including starting, stopping, suspending, and resuming routes.

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower::{Layer, Service, ServiceExt};
use tracing::{error, info, warn};

use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::metrics::MetricsCollector;
use camel_api::{
    BoxProcessor, CamelError, Exchange, FilterPredicate, IdentityProcessor, ProducerContext,
    RouteController, RuntimeCommand, RuntimeHandle, Value, body::Body,
};
use camel_component::{ConcurrencyModel, ConsumerContext, consumer::ExchangeEnvelope};
use camel_endpoint::parse_uri;
use camel_language_api::{Expression, Language, LanguageError, Predicate};
use camel_processor::circuit_breaker::CircuitBreakerLayer;
use camel_processor::error_handler::ErrorHandlerLayer;
use camel_processor::script_mutator::ScriptMutator;
use camel_processor::{ChoiceService, WhenClause};

use crate::lifecycle::adapters::route_compiler::{compose_pipeline, compose_traced_pipeline};
use crate::lifecycle::application::route_definition::{
    BuilderStep, LanguageExpressionDef, RouteDefinition, RouteDefinitionInfo, ValueSourceDef,
};
use crate::shared::components::domain::Registry;
use crate::shared::observability::domain::{DetailLevel, TracerConfig};
use arc_swap::ArcSwap;
use camel_bean::BeanRegistry;

/// Notification sent when a route crashes.
///
/// Used by [`SupervisingRouteController`](crate::supervising_route_controller::SupervisingRouteController)
/// to monitor and restart failed routes.
#[derive(Debug, Clone)]
pub struct CrashNotification {
    /// The ID of the crashed route.
    pub route_id: String,
    /// The error that caused the crash.
    pub error: String,
}

/// Newtype to make BoxProcessor Sync-safe for ArcSwap.
///
/// # Safety
///
/// BoxProcessor (BoxCloneService) is Send but not Sync because the inner
/// Box<dyn CloneServiceInner> lacks a Sync bound. However:
///
/// 1. We ONLY access BoxProcessor via clone(), which is a read-only operation
///    (creates a new boxed service from the inner clone).
/// 2. The clone is owned by the calling thread and never shared.
/// 3. ArcSwap guarantees we only get & references (no &mut).
///
/// Therefore, concurrent access to &BoxProcessor for cloning is safe because
/// clone() does not mutate shared state and each thread gets an independent copy.
pub(crate) struct SyncBoxProcessor(pub(crate) BoxProcessor);
unsafe impl Sync for SyncBoxProcessor {}

type SharedPipeline = Arc<ArcSwap<SyncBoxProcessor>>;
pub type SharedLanguageRegistry = Arc<std::sync::Mutex<HashMap<String, Arc<dyn Language>>>>;

/// Internal trait extending [`RouteController`] with methods needed by [`CamelContext`]
/// that are not part of the public lifecycle API.
///
/// Both [`DefaultRouteController`] and the future `SupervisingRouteController` implement
/// this trait, allowing `CamelContext` to hold either as `Arc<Mutex<dyn RouteControllerInternal>>`.
#[async_trait::async_trait]
pub trait RouteControllerInternal: RouteController + Send {
    /// Add a route definition to the controller.
    fn add_route(&mut self, def: RouteDefinition) -> Result<(), CamelError>;

    /// Atomically swap the pipeline of a running route (for hot-reload).
    fn swap_pipeline(&self, route_id: &str, pipeline: BoxProcessor) -> Result<(), CamelError>;

    /// Returns the `from_uri` of a route by ID.
    fn route_from_uri(&self, route_id: &str) -> Option<String>;

    /// Set a global error handler applied to all routes.
    fn set_error_handler(&mut self, config: ErrorHandlerConfig);

    /// Set the self-reference needed to create `ProducerContext`.
    fn set_self_ref(&mut self, self_ref: Arc<Mutex<dyn RouteController>>);

    /// Set runtime handle for ProducerContext command/query access.
    fn set_runtime_handle(&mut self, runtime: Arc<dyn RuntimeHandle>);

    /// Returns the number of routes in the controller.
    fn route_count(&self) -> usize;

    /// Returns all route IDs.
    fn route_ids(&self) -> Vec<String>;

    /// Returns route IDs that should auto-start, sorted by startup order (ascending).
    fn auto_startup_route_ids(&self) -> Vec<String>;

    /// Returns route IDs sorted by shutdown order (startup order descending).
    fn shutdown_route_ids(&self) -> Vec<String>;

    /// Configure tracing from a [`TracerConfig`].
    fn set_tracer_config(&mut self, config: &TracerConfig);

    /// Compile a `RouteDefinition` into a `BoxProcessor` without inserting it into the route map.
    /// Used by hot-reload to prepare a new pipeline for atomic swap.
    fn compile_route_definition(&self, def: RouteDefinition) -> Result<BoxProcessor, CamelError>;

    /// Remove a route from the controller map (route must be stopped first).
    fn remove_route(&mut self, route_id: &str) -> Result<(), CamelError>;

    /// Start a route by ID (for use by hot-reload, where async_trait is required).
    async fn start_route_reload(&mut self, route_id: &str) -> Result<(), CamelError>;

    /// Stop a route by ID (for use by hot-reload, where async_trait is required).
    async fn stop_route_reload(&mut self, route_id: &str) -> Result<(), CamelError>;
}

/// Internal state for a managed route.
struct ManagedRoute {
    /// The route definition metadata (for introspection).
    definition: RouteDefinitionInfo,
    /// Source endpoint URI.
    from_uri: String,
    /// Resolved processor pipeline (wrapped for atomic swap).
    pipeline: SharedPipeline,
    /// Concurrency model override (if any).
    concurrency: Option<ConcurrencyModel>,
    /// Handle for the consumer task (if running).
    consumer_handle: Option<JoinHandle<()>>,
    /// Handle for the pipeline task (if running).
    pipeline_handle: Option<JoinHandle<()>>,
    /// Cancellation token for stopping the consumer task.
    /// This allows independent control of the consumer lifecycle (for suspend/resume).
    consumer_cancel_token: CancellationToken,
    /// Cancellation token for stopping the pipeline task.
    /// This allows independent control of the pipeline lifecycle (for suspend/resume).
    pipeline_cancel_token: CancellationToken,
    /// Channel sender for sending exchanges to the pipeline.
    /// Stored to allow resuming a suspended route without recreating the channel.
    channel_sender: Option<mpsc::Sender<ExchangeEnvelope>>,
}

fn handle_is_running(handle: &Option<JoinHandle<()>>) -> bool {
    handle.as_ref().is_some_and(|h| !h.is_finished())
}

fn inferred_lifecycle_label(managed: &ManagedRoute) -> &'static str {
    match (
        handle_is_running(&managed.consumer_handle),
        handle_is_running(&managed.pipeline_handle),
    ) {
        (true, true) => "Started",
        (false, true) => "Suspended",
        (true, false) => "Stopping",
        (false, false) => "Stopped",
    }
}

/// Wait for a pipeline service to be ready with circuit breaker backoff.
///
/// This helper encapsulates the pattern of repeatedly calling `ready()` on a
/// service while handling `CircuitOpen` errors with a fixed 1-second backoff and
/// cancellation checks. It returns `Ok(())` when the service is ready, or
/// `Err(e)` if cancellation occurred or a fatal error was encountered.
async fn ready_with_backoff(
    pipeline: &mut BoxProcessor,
    cancel: &CancellationToken,
) -> Result<(), CamelError> {
    loop {
        match pipeline.ready().await {
            Ok(_) => return Ok(()),
            Err(CamelError::CircuitOpen(ref msg)) => {
                warn!("Circuit open, backing off: {msg}");
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {
                        continue;
                    }
                    _ = cancel.cancelled() => {
                        // Shutting down — don't retry.
                        return Err(CamelError::CircuitOpen(msg.clone()));
                    }
                }
            }
            Err(e) => {
                error!("Pipeline not ready: {e}");
                return Err(e);
            }
        }
    }
}

fn runtime_failure_command(route_id: &str, error: &str) -> RuntimeCommand {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    RuntimeCommand::FailRoute {
        route_id: route_id.to_string(),
        error: error.to_string(),
        command_id: format!("ctrl-fail-{route_id}-{stamp}"),
        causation_id: None,
    }
}

async fn publish_runtime_failure(
    runtime: Option<Weak<dyn RuntimeHandle>>,
    route_id: &str,
    error: &str,
) {
    let Some(runtime) = runtime.and_then(|weak| weak.upgrade()) else {
        return;
    };
    let command = runtime_failure_command(route_id, error);
    if let Err(runtime_error) = runtime.execute(command).await {
        warn!(
            route_id = %route_id,
            error = %runtime_error,
            "failed to synchronize route crash with runtime projection"
        );
    }
}

/// Default implementation of [`RouteController`].
///
/// Manages route lifecycle with support for:
/// - Starting/stopping individual routes
/// - Suspending and resuming routes
/// - Auto-startup with startup ordering
/// - Graceful shutdown
pub struct DefaultRouteController {
    /// Routes indexed by route ID.
    routes: HashMap<String, ManagedRoute>,
    /// Reference to the component registry for resolving endpoints.
    registry: Arc<std::sync::Mutex<Registry>>,
    /// Shared language registry for resolving declarative language expressions.
    languages: SharedLanguageRegistry,
    /// Bean registry for bean method invocation.
    beans: Arc<std::sync::Mutex<BeanRegistry>>,
    /// Self-reference for creating ProducerContext.
    /// Set after construction via `set_self_ref()`.
    self_ref: Option<Arc<Mutex<dyn RouteController>>>,
    /// Runtime handle injected into ProducerContext for command/query operations.
    runtime: Option<Weak<dyn RuntimeHandle>>,
    /// Optional global error handler applied to all routes without a per-route handler.
    global_error_handler: Option<ErrorHandlerConfig>,
    /// Optional crash notifier for supervision.
    crash_notifier: Option<mpsc::Sender<CrashNotification>>,
    /// Whether tracing is enabled for route pipelines.
    tracing_enabled: bool,
    /// Detail level for tracing when enabled.
    tracer_detail_level: DetailLevel,
    /// Metrics collector for tracing processor.
    tracer_metrics: Option<Arc<dyn MetricsCollector>>,
}

impl DefaultRouteController {
    /// Create a new `DefaultRouteController` with the given registry.
    pub fn new(registry: Arc<std::sync::Mutex<Registry>>) -> Self {
        Self::with_beans(
            registry,
            Arc::new(std::sync::Mutex::new(BeanRegistry::new())),
        )
    }

    /// Create a new `DefaultRouteController` with shared bean registry.
    pub fn with_beans(
        registry: Arc<std::sync::Mutex<Registry>>,
        beans: Arc<std::sync::Mutex<BeanRegistry>>,
    ) -> Self {
        Self {
            routes: HashMap::new(),
            registry,
            languages: Arc::new(std::sync::Mutex::new(HashMap::new())),
            beans,
            self_ref: None,
            runtime: None,
            global_error_handler: None,
            crash_notifier: None,
            tracing_enabled: false,
            tracer_detail_level: DetailLevel::Minimal,
            tracer_metrics: None,
        }
    }

    /// Create a new `DefaultRouteController` with shared language registry.
    pub fn with_languages(
        registry: Arc<std::sync::Mutex<Registry>>,
        languages: SharedLanguageRegistry,
    ) -> Self {
        Self {
            routes: HashMap::new(),
            registry,
            languages,
            beans: Arc::new(std::sync::Mutex::new(BeanRegistry::new())),
            self_ref: None,
            runtime: None,
            global_error_handler: None,
            crash_notifier: None,
            tracing_enabled: false,
            tracer_detail_level: DetailLevel::Minimal,
            tracer_metrics: None,
        }
    }

    /// Set the self-reference for creating ProducerContext.
    ///
    /// This must be called after wrapping the controller in `Arc<Mutex<>>`.
    pub fn set_self_ref(&mut self, self_ref: Arc<Mutex<dyn RouteController>>) {
        self.self_ref = Some(self_ref);
    }

    /// Set runtime handle for ProducerContext creation.
    pub fn set_runtime_handle(&mut self, runtime: Arc<dyn RuntimeHandle>) {
        self.runtime = Some(Arc::downgrade(&runtime));
    }

    /// Get the self-reference, if set.
    ///
    /// Used by [`SupervisingRouteController`](crate::supervising_route_controller::SupervisingRouteController)
    /// to spawn the supervision loop.
    pub fn self_ref_for_supervision(&self) -> Option<Arc<Mutex<dyn RouteController>>> {
        self.self_ref.clone()
    }

    /// Get runtime handle for supervision-triggered lifecycle commands, if set.
    pub fn runtime_handle_for_supervision(&self) -> Option<Arc<dyn RuntimeHandle>> {
        self.runtime.as_ref().and_then(Weak::upgrade)
    }

    /// Set the crash notifier for supervision.
    ///
    /// When set, the controller will send a [`CrashNotification`] whenever
    /// a consumer crashes.
    pub fn set_crash_notifier(&mut self, tx: mpsc::Sender<CrashNotification>) {
        self.crash_notifier = Some(tx);
    }

    /// Set a global error handler applied to all routes without a per-route handler.
    pub fn set_error_handler(&mut self, config: ErrorHandlerConfig) {
        self.global_error_handler = Some(config);
    }

    /// Configure tracing for this route controller.
    pub fn set_tracer_config(&mut self, config: &TracerConfig) {
        self.tracing_enabled = config.enabled;
        self.tracer_detail_level = config.detail_level.clone();
        self.tracer_metrics = config.metrics_collector.clone();
    }

    fn build_producer_context(&self) -> Result<ProducerContext, CamelError> {
        let mut producer_ctx = ProducerContext::new();
        if let Some(runtime) = self.runtime.as_ref().and_then(Weak::upgrade) {
            producer_ctx = producer_ctx.with_runtime(runtime);
        }
        Ok(producer_ctx)
    }

    /// Resolve an `ErrorHandlerConfig` into an `ErrorHandlerLayer`.
    fn resolve_error_handler(
        &self,
        config: ErrorHandlerConfig,
        producer_ctx: &ProducerContext,
        registry: &Registry,
    ) -> Result<ErrorHandlerLayer, CamelError> {
        // Resolve DLC URI → producer.
        let dlc_producer = if let Some(ref uri) = config.dlc_uri {
            let parsed = parse_uri(uri)?;
            let component = registry.get_or_err(&parsed.scheme)?;
            let endpoint = component.create_endpoint(uri)?;
            Some(endpoint.create_producer(producer_ctx)?)
        } else {
            None
        };

        // Resolve per-policy `handled_by` URIs.
        let mut resolved_policies = Vec::new();
        for policy in config.policies {
            let handler_producer = if let Some(ref uri) = policy.handled_by {
                let parsed = parse_uri(uri)?;
                let component = registry.get_or_err(&parsed.scheme)?;
                let endpoint = component.create_endpoint(uri)?;
                Some(endpoint.create_producer(producer_ctx)?)
            } else {
                None
            };
            resolved_policies.push((policy, handler_producer));
        }

        Ok(ErrorHandlerLayer::new(dlc_producer, resolved_policies))
    }

    fn resolve_language(&self, language: &str) -> Result<Arc<dyn Language>, CamelError> {
        let guard = self
            .languages
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock");
        guard.get(language).cloned().ok_or_else(|| {
            CamelError::RouteError(format!(
                "language `{language}` is not registered in CamelContext"
            ))
        })
    }

    fn compile_language_expression(
        &self,
        expression: &LanguageExpressionDef,
    ) -> Result<Arc<dyn Expression>, CamelError> {
        let language = self.resolve_language(&expression.language)?;
        let compiled = language
            .create_expression(&expression.source)
            .map_err(|e| {
                CamelError::RouteError(format!(
                    "failed to compile {} expression `{}`: {e}",
                    expression.language, expression.source
                ))
            })?;
        Ok(Arc::from(compiled))
    }

    fn compile_language_predicate(
        &self,
        expression: &LanguageExpressionDef,
    ) -> Result<Arc<dyn Predicate>, CamelError> {
        let language = self.resolve_language(&expression.language)?;
        let compiled = language.create_predicate(&expression.source).map_err(|e| {
            CamelError::RouteError(format!(
                "failed to compile {} predicate `{}`: {e}",
                expression.language, expression.source
            ))
        })?;
        Ok(Arc::from(compiled))
    }

    fn compile_filter_predicate(
        &self,
        expression: &LanguageExpressionDef,
    ) -> Result<FilterPredicate, CamelError> {
        let predicate = self.compile_language_predicate(expression)?;
        Ok(Arc::new(move |exchange: &Exchange| {
            predicate.matches(exchange).unwrap_or(false)
        }))
    }

    fn value_to_body(value: Value) -> Body {
        match value {
            Value::Null => Body::Empty,
            Value::String(text) => Body::Text(text),
            other => Body::Json(other),
        }
    }

    /// Resolve BuilderSteps into BoxProcessors.
    pub(crate) fn resolve_steps(
        &self,
        steps: Vec<BuilderStep>,
        producer_ctx: &ProducerContext,
        registry: Arc<std::sync::Mutex<Registry>>,
    ) -> Result<Vec<BoxProcessor>, CamelError> {
        let resolve_producer = |uri: &str| -> Result<BoxProcessor, CamelError> {
            let parsed = parse_uri(uri)?;
            let registry_guard = registry
                .lock()
                .expect("mutex poisoned: another thread panicked while holding this lock");
            let component = registry_guard.get_or_err(&parsed.scheme)?;
            let endpoint = component.create_endpoint(uri)?;
            endpoint.create_producer(producer_ctx)
        };

        let mut processors: Vec<BoxProcessor> = Vec::new();
        for step in steps {
            match step {
                BuilderStep::Processor(svc) => {
                    processors.push(svc);
                }
                BuilderStep::To(uri) => {
                    let producer = resolve_producer(&uri)?;
                    processors.push(producer);
                }
                BuilderStep::Stop => {
                    processors.push(BoxProcessor::new(camel_processor::StopService));
                }
                BuilderStep::Log { level, message } => {
                    let svc = camel_processor::LogProcessor::new(level, message);
                    processors.push(BoxProcessor::new(svc));
                }
                BuilderStep::DeclarativeSetHeader { key, value } => match value {
                    ValueSourceDef::Literal(value) => {
                        let svc = camel_processor::SetHeader::new(IdentityProcessor, key, value);
                        processors.push(BoxProcessor::new(svc));
                    }
                    ValueSourceDef::Expression(expression) => {
                        let expression = self.compile_language_expression(&expression)?;
                        let svc = camel_processor::DynamicSetHeader::new(
                            IdentityProcessor,
                            key,
                            move |exchange: &Exchange| {
                                expression.evaluate(exchange).unwrap_or(Value::Null)
                            },
                        );
                        processors.push(BoxProcessor::new(svc));
                    }
                },
                BuilderStep::DeclarativeSetBody { value } => match value {
                    ValueSourceDef::Literal(value) => {
                        let body = Self::value_to_body(value);
                        let svc = camel_processor::SetBody::new(
                            IdentityProcessor,
                            move |_exchange: &Exchange| body.clone(),
                        );
                        processors.push(BoxProcessor::new(svc));
                    }
                    ValueSourceDef::Expression(expression) => {
                        let expression = self.compile_language_expression(&expression)?;
                        let svc = camel_processor::SetBody::new(
                            IdentityProcessor,
                            move |exchange: &Exchange| {
                                let value = expression.evaluate(exchange).unwrap_or(Value::Null);
                                Self::value_to_body(value)
                            },
                        );
                        processors.push(BoxProcessor::new(svc));
                    }
                },
                BuilderStep::DeclarativeFilter { predicate, steps } => {
                    let predicate = self.compile_filter_predicate(&predicate)?;
                    let sub_processors =
                        self.resolve_steps(steps, producer_ctx, registry.clone())?;
                    let sub_pipeline = compose_pipeline(sub_processors);
                    let svc =
                        camel_processor::FilterService::from_predicate(predicate, sub_pipeline);
                    processors.push(BoxProcessor::new(svc));
                }
                BuilderStep::DeclarativeChoice { whens, otherwise } => {
                    let mut when_clauses = Vec::new();
                    for when_step in whens {
                        let predicate = self.compile_filter_predicate(&when_step.predicate)?;
                        let sub_processors =
                            self.resolve_steps(when_step.steps, producer_ctx, registry.clone())?;
                        let pipeline = compose_pipeline(sub_processors);
                        when_clauses.push(WhenClause {
                            predicate,
                            pipeline,
                        });
                    }
                    let otherwise_pipeline = if let Some(otherwise_steps) = otherwise {
                        let sub_processors =
                            self.resolve_steps(otherwise_steps, producer_ctx, registry.clone())?;
                        Some(compose_pipeline(sub_processors))
                    } else {
                        None
                    };
                    let svc = ChoiceService::new(when_clauses, otherwise_pipeline);
                    processors.push(BoxProcessor::new(svc));
                }
                BuilderStep::DeclarativeScript { expression } => {
                    let expression = self.compile_language_expression(&expression)?;
                    let svc = camel_processor::SetBody::new(
                        IdentityProcessor,
                        move |exchange: &Exchange| {
                            let value = expression.evaluate(exchange).unwrap_or(Value::Null);
                            Self::value_to_body(value)
                        },
                    );
                    processors.push(BoxProcessor::new(svc));
                }
                BuilderStep::Split { config, steps } => {
                    let sub_processors =
                        self.resolve_steps(steps, producer_ctx, registry.clone())?;
                    let sub_pipeline = compose_pipeline(sub_processors);
                    let splitter =
                        camel_processor::splitter::SplitterService::new(config, sub_pipeline);
                    processors.push(BoxProcessor::new(splitter));
                }
                BuilderStep::DeclarativeSplit {
                    expression,
                    aggregation,
                    parallel,
                    parallel_limit,
                    stop_on_exception,
                    steps,
                } => {
                    let lang_expr = self.compile_language_expression(&expression)?;
                    let split_fn = move |exchange: &Exchange| {
                        let value = lang_expr.evaluate(exchange).unwrap_or(Value::Null);
                        match value {
                            Value::String(s) => s
                                .lines()
                                .filter(|line| !line.is_empty())
                                .map(|line| {
                                    let mut fragment = exchange.clone();
                                    fragment.input.body = Body::from(line.to_string());
                                    fragment
                                })
                                .collect(),
                            Value::Array(arr) => arr
                                .into_iter()
                                .map(|v| {
                                    let mut fragment = exchange.clone();
                                    fragment.input.body = Body::from(v);
                                    fragment
                                })
                                .collect(),
                            _ => vec![exchange.clone()],
                        }
                    };

                    let mut config = camel_api::splitter::SplitterConfig::new(Arc::new(split_fn))
                        .aggregation(aggregation)
                        .parallel(parallel)
                        .stop_on_exception(stop_on_exception);
                    if let Some(limit) = parallel_limit {
                        config = config.parallel_limit(limit);
                    }

                    let sub_processors =
                        self.resolve_steps(steps, producer_ctx, registry.clone())?;
                    let sub_pipeline = compose_pipeline(sub_processors);
                    let splitter =
                        camel_processor::splitter::SplitterService::new(config, sub_pipeline);
                    processors.push(BoxProcessor::new(splitter));
                }
                BuilderStep::Aggregate { config } => {
                    let svc = camel_processor::AggregatorService::new(config);
                    processors.push(BoxProcessor::new(svc));
                }
                BuilderStep::Filter { predicate, steps } => {
                    let sub_processors =
                        self.resolve_steps(steps, producer_ctx, registry.clone())?;
                    let sub_pipeline = compose_pipeline(sub_processors);
                    let svc =
                        camel_processor::FilterService::from_predicate(predicate, sub_pipeline);
                    processors.push(BoxProcessor::new(svc));
                }
                BuilderStep::Choice { whens, otherwise } => {
                    // Resolve each when clause's sub-steps into a pipeline.
                    let mut when_clauses = Vec::new();
                    for when_step in whens {
                        let sub_processors =
                            self.resolve_steps(when_step.steps, producer_ctx, registry.clone())?;
                        let pipeline = compose_pipeline(sub_processors);
                        when_clauses.push(WhenClause {
                            predicate: when_step.predicate,
                            pipeline,
                        });
                    }
                    // Resolve otherwise branch (if present).
                    let otherwise_pipeline = if let Some(otherwise_steps) = otherwise {
                        let sub_processors =
                            self.resolve_steps(otherwise_steps, producer_ctx, registry.clone())?;
                        Some(compose_pipeline(sub_processors))
                    } else {
                        None
                    };
                    let svc = ChoiceService::new(when_clauses, otherwise_pipeline);
                    processors.push(BoxProcessor::new(svc));
                }
                BuilderStep::WireTap { uri } => {
                    let producer = resolve_producer(&uri)?;
                    let svc = camel_processor::WireTapService::new(producer);
                    processors.push(BoxProcessor::new(svc));
                }
                BuilderStep::Multicast { config, steps } => {
                    // Each top-level step in the multicast scope becomes an independent endpoint.
                    let mut endpoints = Vec::new();
                    for step in steps {
                        let sub_processors =
                            self.resolve_steps(vec![step], producer_ctx, registry.clone())?;
                        let endpoint = compose_pipeline(sub_processors);
                        endpoints.push(endpoint);
                    }
                    let svc = camel_processor::MulticastService::new(endpoints, config);
                    processors.push(BoxProcessor::new(svc));
                }
                BuilderStep::DeclarativeLog { level, message } => {
                    let ValueSourceDef::Expression(expression) = message else {
                        // Literal case is already converted to a Processor in compile.rs;
                        // this arm should never be reached for literals.
                        unreachable!(
                            "DeclarativeLog with Literal should have been compiled to a Processor"
                        );
                    };
                    let expression = self.compile_language_expression(&expression)?;
                    let svc =
                        camel_processor::log::DynamicLog::new(level, move |exchange: &Exchange| {
                            expression
                                .evaluate(exchange)
                                .unwrap_or_else(|e| {
                                    warn!(error = %e, "log expression evaluation failed");
                                    Value::Null
                                })
                                .to_string()
                        });
                    processors.push(BoxProcessor::new(svc));
                }
                BuilderStep::Bean { name, method } => {
                    // Lock beans registry to lookup bean
                    let beans = self.beans.lock().expect(
                        "beans mutex poisoned: another thread panicked while holding this lock",
                    );

                    // Lookup bean by name
                    let bean = beans.get(&name).ok_or_else(|| {
                        CamelError::ProcessorError(format!("Bean not found: {}", name))
                    })?;

                    // Clone Arc for async closure (release lock before async)
                    let bean_clone = Arc::clone(&bean);
                    let method = method.clone();

                    // Create processor that invokes bean method
                    let processor = tower::service_fn(move |mut exchange: Exchange| {
                        let bean = Arc::clone(&bean_clone);
                        let method = method.clone();

                        async move {
                            bean.call(&method, &mut exchange).await?;
                            Ok(exchange)
                        }
                    });

                    processors.push(BoxProcessor::new(processor));
                }
                BuilderStep::Script { language, script } => {
                    let lang = self.resolve_language(&language)?;
                    match lang.create_mutating_expression(&script) {
                        Ok(mut_expr) => {
                            processors.push(BoxProcessor::new(ScriptMutator::new(mut_expr)));
                        }
                        Err(LanguageError::NotSupported {
                            feature,
                            language: ref lang_name,
                        }) => {
                            return Err(CamelError::RouteError(format!(
                                "Language '{}' does not support {} (required for .script() step)",
                                lang_name, feature
                            )));
                        }
                        Err(e) => {
                            return Err(CamelError::RouteError(format!(
                                "Failed to create mutating expression for language '{}': {}",
                                language, e
                            )));
                        }
                    }
                }
                BuilderStep::Throttle { config, steps } => {
                    let sub_processors =
                        self.resolve_steps(steps, producer_ctx, registry.clone())?;
                    let sub_pipeline = compose_pipeline(sub_processors);
                    let svc =
                        camel_processor::throttler::ThrottlerService::new(config, sub_pipeline);
                    processors.push(BoxProcessor::new(svc));
                }
                BuilderStep::LoadBalance { config, steps } => {
                    // Each top-level step in the load_balance scope becomes an independent endpoint.
                    let mut endpoints = Vec::new();
                    for step in steps {
                        let sub_processors =
                            self.resolve_steps(vec![step], producer_ctx, registry.clone())?;
                        let endpoint = compose_pipeline(sub_processors);
                        endpoints.push(endpoint);
                    }
                    let svc =
                        camel_processor::load_balancer::LoadBalancerService::new(endpoints, config);
                    processors.push(BoxProcessor::new(svc));
                }
                BuilderStep::DynamicRouter { config } => {
                    use camel_processor::dynamic_router::EndpointResolver;

                    let producer_ctx_clone = producer_ctx.clone();
                    let registry_clone = registry.clone();
                    let resolver: EndpointResolver = Arc::new(move |uri: &str| {
                        let parsed = match parse_uri(uri) {
                            Ok(p) => p,
                            Err(_) => return None,
                        };
                        let registry_guard = match registry_clone.lock() {
                            Ok(g) => g,
                            Err(_) => return None, // mutex poisoned
                        };
                        let component = match registry_guard.get_or_err(&parsed.scheme) {
                            Ok(c) => c,
                            Err(_) => return None,
                        };
                        let endpoint = match component.create_endpoint(uri) {
                            Ok(e) => e,
                            Err(_) => return None,
                        };
                        let producer = match endpoint.create_producer(&producer_ctx_clone) {
                            Ok(p) => p,
                            Err(_) => return None,
                        };
                        Some(BoxProcessor::new(producer))
                    });
                    let svc = camel_processor::dynamic_router::DynamicRouterService::new(
                        config, resolver,
                    );
                    processors.push(BoxProcessor::new(svc));
                }
            }
        }
        Ok(processors)
    }

    /// Add a route definition to the controller.
    ///
    /// Steps are resolved immediately using the registry.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - A route with the same ID already exists
    /// - Step resolution fails
    pub fn add_route(&mut self, definition: RouteDefinition) -> Result<(), CamelError> {
        let route_id = definition.route_id().to_string();

        if self.routes.contains_key(&route_id) {
            return Err(CamelError::RouteError(format!(
                "Route '{}' already exists",
                route_id
            )));
        }

        info!(route_id = %route_id, "Adding route to controller");

        // Extract definition info for storage before steps are consumed
        let definition_info = definition.to_info();
        let from_uri = definition.from_uri.to_string();
        let concurrency = definition.concurrency;

        // Create ProducerContext from self_ref for step resolution
        let producer_ctx = self.build_producer_context()?;

        // Resolve steps into processors (takes ownership of steps)
        let processors =
            self.resolve_steps(definition.steps, &producer_ctx, self.registry.clone())?;
        let route_id_for_tracing = route_id.clone();
        let mut pipeline = compose_traced_pipeline(
            processors,
            &route_id_for_tracing,
            self.tracing_enabled,
            self.tracer_detail_level.clone(),
            self.tracer_metrics.clone(),
        );

        // Apply circuit breaker if configured
        if let Some(cb_config) = definition.circuit_breaker {
            let cb_layer = CircuitBreakerLayer::new(cb_config);
            pipeline = BoxProcessor::new(cb_layer.layer(pipeline));
        }

        // Determine which error handler config to use (per-route takes precedence)
        let eh_config = definition
            .error_handler
            .or_else(|| self.global_error_handler.clone());

        if let Some(config) = eh_config {
            // Lock registry for error handler resolution
            let registry = self
                .registry
                .lock()
                .expect("mutex poisoned: another thread panicked while holding this lock");
            let layer = self.resolve_error_handler(config, &producer_ctx, &registry)?;
            pipeline = BoxProcessor::new(layer.layer(pipeline));
        }

        self.routes.insert(
            route_id.clone(),
            ManagedRoute {
                definition: definition_info,
                from_uri,
                pipeline: Arc::new(ArcSwap::from_pointee(SyncBoxProcessor(pipeline))),
                concurrency,
                consumer_handle: None,
                pipeline_handle: None,
                consumer_cancel_token: CancellationToken::new(),
                pipeline_cancel_token: CancellationToken::new(),
                channel_sender: None,
            },
        );

        Ok(())
    }

    /// Compile a `RouteDefinition` into a `BoxProcessor` without inserting into the route map.
    ///
    /// Used by hot-reload to prepare a new pipeline for atomic swap without disrupting
    /// the running route. The caller is responsible for swapping via `swap_pipeline`.
    pub fn compile_route_definition(
        &self,
        def: RouteDefinition,
    ) -> Result<BoxProcessor, CamelError> {
        let route_id = def.route_id().to_string();

        let producer_ctx = self.build_producer_context()?;

        let processors = self.resolve_steps(def.steps, &producer_ctx, self.registry.clone())?;
        let mut pipeline = compose_traced_pipeline(
            processors,
            &route_id,
            self.tracing_enabled,
            self.tracer_detail_level.clone(),
            self.tracer_metrics.clone(),
        );

        if let Some(cb_config) = def.circuit_breaker {
            let cb_layer = CircuitBreakerLayer::new(cb_config);
            pipeline = BoxProcessor::new(cb_layer.layer(pipeline));
        }

        let eh_config = def
            .error_handler
            .or_else(|| self.global_error_handler.clone());
        if let Some(config) = eh_config {
            // Lock registry for error handler resolution
            let registry = self
                .registry
                .lock()
                .expect("mutex poisoned: registry lock in compile_route_definition");
            let layer = self.resolve_error_handler(config, &producer_ctx, &registry)?;
            pipeline = BoxProcessor::new(layer.layer(pipeline));
        }

        Ok(pipeline)
    }

    /// Remove a route from the controller map.
    ///
    /// The route **must** be stopped before removal (status `Stopped` or `Failed`).
    /// Returns an error if the route is still running or does not exist.
    /// Does not cancel any running tasks — call `stop_route` first.
    pub fn remove_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        let managed = self.routes.get(route_id).ok_or_else(|| {
            CamelError::RouteError(format!("Route '{}' not found for removal", route_id))
        })?;
        if handle_is_running(&managed.consumer_handle)
            || handle_is_running(&managed.pipeline_handle)
        {
            return Err(CamelError::RouteError(format!(
                "Route '{}' must be stopped before removal (current execution lifecycle: {})",
                route_id,
                inferred_lifecycle_label(managed)
            )));
        }
        self.routes.remove(route_id);
        info!(route_id = %route_id, "Route removed from controller");
        Ok(())
    }

    /// Returns the number of routes in the controller.
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    /// Returns all route IDs.
    pub fn route_ids(&self) -> Vec<String> {
        self.routes.keys().cloned().collect()
    }

    /// Returns route IDs that should auto-start, sorted by startup order (ascending).
    pub fn auto_startup_route_ids(&self) -> Vec<String> {
        let mut pairs: Vec<(String, i32)> = self
            .routes
            .iter()
            .filter(|(_, managed)| managed.definition.auto_startup())
            .map(|(id, managed)| (id.clone(), managed.definition.startup_order()))
            .collect();
        pairs.sort_by_key(|(_, order)| *order);
        pairs.into_iter().map(|(id, _)| id).collect()
    }

    /// Returns route IDs sorted by shutdown order (startup order descending).
    pub fn shutdown_route_ids(&self) -> Vec<String> {
        let mut pairs: Vec<(String, i32)> = self
            .routes
            .iter()
            .map(|(id, managed)| (id.clone(), managed.definition.startup_order()))
            .collect();
        pairs.sort_by_key(|(_, order)| std::cmp::Reverse(*order));
        pairs.into_iter().map(|(id, _)| id).collect()
    }

    /// Atomically swap the pipeline of a route.
    ///
    /// In-flight requests finish with the old pipeline (kept alive by Arc).
    /// New requests immediately use the new pipeline.
    pub fn swap_pipeline(
        &self,
        route_id: &str,
        new_pipeline: BoxProcessor,
    ) -> Result<(), CamelError> {
        let managed = self
            .routes
            .get(route_id)
            .ok_or_else(|| CamelError::RouteError(format!("Route '{}' not found", route_id)))?;

        managed
            .pipeline
            .store(Arc::new(SyncBoxProcessor(new_pipeline)));
        info!(route_id = %route_id, "Pipeline swapped atomically");
        Ok(())
    }

    /// Returns the from_uri of a route, if it exists.
    pub fn route_from_uri(&self, route_id: &str) -> Option<String> {
        self.routes.get(route_id).map(|r| r.from_uri.clone())
    }

    /// Get a clone of the current pipeline for a route.
    ///
    /// This is useful for testing and introspection.
    /// Returns `None` if the route doesn't exist.
    pub fn get_pipeline(&self, route_id: &str) -> Option<BoxProcessor> {
        self.routes
            .get(route_id)
            .map(|r| r.pipeline.load().0.clone())
    }

    /// Internal stop implementation that can set custom status.
    async fn stop_route_internal(&mut self, route_id: &str) -> Result<(), CamelError> {
        let managed = self
            .routes
            .get_mut(route_id)
            .ok_or_else(|| CamelError::RouteError(format!("Route '{}' not found", route_id)))?;

        if !handle_is_running(&managed.consumer_handle)
            && !handle_is_running(&managed.pipeline_handle)
        {
            return Ok(());
        }

        info!(route_id = %route_id, "Stopping route");

        // Cancel both tokens to signal shutdown for consumer and pipeline independently
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check");
        managed.consumer_cancel_token.cancel();
        managed.pipeline_cancel_token.cancel();

        // Take handles directly (no Arc<Mutex> wrapper needed)
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check");
        let consumer_handle = managed.consumer_handle.take();
        let pipeline_handle = managed.pipeline_handle.take();

        // IMPORTANT: Drop channel_sender early so rx.recv() returns None
        // This ensures the pipeline task can exit even if idle on recv()
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check");
        managed.channel_sender = None;

        // Wait for tasks to complete with timeout
        // The CancellationToken already signaled tasks to stop gracefully.
        // Combined with the select! in pipeline loops, this should exit quickly.
        let timeout_result = tokio::time::timeout(Duration::from_secs(30), async {
            match (consumer_handle, pipeline_handle) {
                (Some(c), Some(p)) => {
                    let _ = tokio::join!(c, p);
                }
                (Some(c), None) => {
                    let _ = c.await;
                }
                (None, Some(p)) => {
                    let _ = p.await;
                }
                (None, None) => {}
            }
        })
        .await;

        if timeout_result.is_err() {
            warn!(route_id = %route_id, "Route shutdown timed out after 30s — tasks may still be running");
        }

        // Get the managed route again (can't hold across await)
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check");

        // Create fresh cancellation tokens for next start
        managed.consumer_cancel_token = CancellationToken::new();
        managed.pipeline_cancel_token = CancellationToken::new();

        info!(route_id = %route_id, "Route stopped");
        Ok(())
    }
}

#[async_trait::async_trait]
impl RouteController for DefaultRouteController {
    async fn start_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        // Check if route exists and can be started.
        {
            let managed = self
                .routes
                .get_mut(route_id)
                .ok_or_else(|| CamelError::RouteError(format!("Route '{}' not found", route_id)))?;

            let consumer_running = handle_is_running(&managed.consumer_handle);
            let pipeline_running = handle_is_running(&managed.pipeline_handle);
            if consumer_running && pipeline_running {
                return Ok(());
            }
            if !consumer_running && pipeline_running {
                return Err(CamelError::RouteError(format!(
                    "Route '{}' is suspended; use resume_route() to resume, or stop_route() then start_route() for full restart",
                    route_id
                )));
            }
            if consumer_running && !pipeline_running {
                return Err(CamelError::RouteError(format!(
                    "Route '{}' has inconsistent execution state; stop_route() then retry start_route()",
                    route_id
                )));
            }
        }

        info!(route_id = %route_id, "Starting route");

        // Get the resolved route info
        let (from_uri, pipeline, concurrency) = {
            let managed = self
                .routes
                .get(route_id)
                .expect("invariant: route must exist after prior existence check");
            (
                managed.from_uri.clone(),
                Arc::clone(&managed.pipeline),
                managed.concurrency.clone(),
            )
        };

        // Clone crash notifier for consumer task
        let crash_notifier = self.crash_notifier.clone();
        let runtime_for_consumer = self.runtime.clone();

        // Parse from URI and create consumer (lock registry for lookup)
        let parsed = parse_uri(&from_uri)?;
        let registry = self
            .registry
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock");
        let component = registry.get_or_err(&parsed.scheme)?;
        let endpoint = component.create_endpoint(&from_uri)?;
        let mut consumer = endpoint.create_consumer()?;
        let consumer_concurrency = consumer.concurrency_model();
        // Drop the lock before spawning tasks
        drop(registry);

        // Resolve effective concurrency: route override > consumer default
        let effective_concurrency = concurrency.unwrap_or(consumer_concurrency);

        // Get the managed route for mutation
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check");

        // Create channel for consumer to send exchanges
        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(256);
        // Create child tokens for independent lifecycle control
        let consumer_cancel = managed.consumer_cancel_token.child_token();
        let pipeline_cancel = managed.pipeline_cancel_token.child_token();
        // Clone sender for storage (to reuse on resume)
        let tx_for_storage = tx.clone();
        let consumer_ctx = ConsumerContext::new(tx, consumer_cancel.clone());

        // Start consumer in background task.
        let route_id_for_consumer = route_id.to_string();
        let consumer_handle = tokio::spawn(async move {
            if let Err(e) = consumer.start(consumer_ctx).await {
                error!(route_id = %route_id_for_consumer, "Consumer error: {e}");
                let error_msg = e.to_string();

                // Send crash notification if notifier is configured
                if let Some(tx) = crash_notifier {
                    let _ = tx
                        .send(CrashNotification {
                            route_id: route_id_for_consumer.clone(),
                            error: error_msg.clone(),
                        })
                        .await;
                }

                publish_runtime_failure(runtime_for_consumer, &route_id_for_consumer, &error_msg)
                    .await;
            }
        });

        // Spawn pipeline task with its own cancellation token
        let pipeline_handle = match effective_concurrency {
            ConcurrencyModel::Sequential => {
                tokio::spawn(async move {
                    loop {
                        // Use select! to exit promptly on cancellation even when idle
                        let envelope = tokio::select! {
                            envelope = rx.recv() => match envelope {
                                Some(e) => e,
                                None => return, // Channel closed
                            },
                            _ = pipeline_cancel.cancelled() => {
                                // Cancellation requested - exit gracefully
                                return;
                            }
                        };
                        let ExchangeEnvelope { exchange, reply_tx } = envelope;

                        // Load current pipeline from ArcSwap (picks up hot-reloaded pipelines)
                        let mut pipeline = pipeline.load().0.clone();

                        if let Err(e) = ready_with_backoff(&mut pipeline, &pipeline_cancel).await {
                            if let Some(tx) = reply_tx {
                                let _ = tx.send(Err(e));
                            }
                            return;
                        }

                        let result = pipeline.call(exchange).await;
                        if let Some(tx) = reply_tx {
                            let _ = tx.send(result);
                        } else if let Err(ref e) = result
                            && !matches!(e, CamelError::Stopped)
                        {
                            error!("Pipeline error: {e}");
                        }
                    }
                })
            }
            ConcurrencyModel::Concurrent { max } => {
                let sem = max.map(|n| Arc::new(tokio::sync::Semaphore::new(n)));
                tokio::spawn(async move {
                    loop {
                        // Use select! to exit promptly on cancellation even when idle
                        let envelope = tokio::select! {
                            envelope = rx.recv() => match envelope {
                                Some(e) => e,
                                None => return, // Channel closed
                            },
                            _ = pipeline_cancel.cancelled() => {
                                // Cancellation requested - exit gracefully
                                return;
                            }
                        };
                        let ExchangeEnvelope { exchange, reply_tx } = envelope;
                        let pipe_ref = Arc::clone(&pipeline);
                        let sem = sem.clone();
                        let cancel = pipeline_cancel.clone();
                        tokio::spawn(async move {
                            // Acquire semaphore permit if bounded
                            let _permit = match &sem {
                                Some(s) => Some(s.acquire().await.expect("semaphore closed")),
                                None => None,
                            };

                            // Load current pipeline from ArcSwap
                            let mut pipe = pipe_ref.load().0.clone();

                            // Wait for service ready with circuit breaker backoff
                            if let Err(e) = ready_with_backoff(&mut pipe, &cancel).await {
                                if let Some(tx) = reply_tx {
                                    let _ = tx.send(Err(e));
                                }
                                return;
                            }

                            let result = pipe.call(exchange).await;
                            if let Some(tx) = reply_tx {
                                let _ = tx.send(result);
                            } else if let Err(ref e) = result
                                && !matches!(e, CamelError::Stopped)
                            {
                                error!("Pipeline error: {e}");
                            }
                        });
                    }
                })
            }
        };

        // Store handles and update status
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check");
        managed.consumer_handle = Some(consumer_handle);
        managed.pipeline_handle = Some(pipeline_handle);
        managed.channel_sender = Some(tx_for_storage);

        info!(route_id = %route_id, "Route started");
        Ok(())
    }

    async fn stop_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        self.stop_route_internal(route_id).await
    }

    async fn restart_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        self.stop_route(route_id).await?;
        tokio::time::sleep(Duration::from_millis(100)).await;
        self.start_route(route_id).await
    }

    async fn suspend_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        // Check route exists and state.
        let managed = self
            .routes
            .get_mut(route_id)
            .ok_or_else(|| CamelError::RouteError(format!("Route '{}' not found", route_id)))?;

        let consumer_running = handle_is_running(&managed.consumer_handle);
        let pipeline_running = handle_is_running(&managed.pipeline_handle);

        // Can only suspend from active started state.
        if !consumer_running || !pipeline_running {
            return Err(CamelError::RouteError(format!(
                "Cannot suspend route '{}' with execution lifecycle {}",
                route_id,
                inferred_lifecycle_label(managed)
            )));
        }

        info!(route_id = %route_id, "Suspending route (consumer only, keeping pipeline)");

        // Cancel consumer token only (keep pipeline running)
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check");
        managed.consumer_cancel_token.cancel();

        // Take and join consumer handle
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check");
        let consumer_handle = managed.consumer_handle.take();

        // Wait for consumer task to complete with timeout
        let timeout_result = tokio::time::timeout(Duration::from_secs(30), async {
            if let Some(handle) = consumer_handle {
                let _ = handle.await;
            }
        })
        .await;

        if timeout_result.is_err() {
            warn!(route_id = %route_id, "Consumer shutdown timed out during suspend");
        }

        // Get the managed route again (can't hold across await)
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check");

        // Create fresh cancellation token for consumer (for resume)
        managed.consumer_cancel_token = CancellationToken::new();

        info!(route_id = %route_id, "Route suspended (pipeline still running)");
        Ok(())
    }

    async fn resume_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        // Check route exists and is Suspended-equivalent execution state.
        let managed = self
            .routes
            .get(route_id)
            .ok_or_else(|| CamelError::RouteError(format!("Route '{}' not found", route_id)))?;

        let consumer_running = handle_is_running(&managed.consumer_handle);
        let pipeline_running = handle_is_running(&managed.pipeline_handle);
        if consumer_running || !pipeline_running {
            return Err(CamelError::RouteError(format!(
                "Cannot resume route '{}' with execution lifecycle {} (expected Suspended)",
                route_id,
                inferred_lifecycle_label(managed)
            )));
        }

        // Get the stored channel sender (must exist for a suspended route)
        let sender = managed.channel_sender.clone().ok_or_else(|| {
            CamelError::RouteError("Suspended route has no channel sender".into())
        })?;

        // Get from_uri and concurrency for creating new consumer
        let from_uri = managed.from_uri.clone();

        info!(route_id = %route_id, "Resuming route (spawning consumer only)");

        // Parse from URI and create consumer (lock registry for lookup)
        let parsed = parse_uri(&from_uri)?;
        let registry = self
            .registry
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock");
        let component = registry.get_or_err(&parsed.scheme)?;
        let endpoint = component.create_endpoint(&from_uri)?;
        let mut consumer = endpoint.create_consumer()?;
        // Drop the lock before spawning tasks
        drop(registry);

        // Get the managed route for mutation
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check");

        // Create child token for consumer lifecycle
        let consumer_cancel = managed.consumer_cancel_token.child_token();

        let crash_notifier = self.crash_notifier.clone();
        let runtime_for_consumer = self.runtime.clone();

        // Create ConsumerContext with the stored sender
        let consumer_ctx = ConsumerContext::new(sender, consumer_cancel.clone());

        // Spawn consumer task
        let route_id_for_consumer = route_id.to_string();
        let consumer_handle = tokio::spawn(async move {
            if let Err(e) = consumer.start(consumer_ctx).await {
                error!(route_id = %route_id_for_consumer, "Consumer error on resume: {e}");
                let error_msg = e.to_string();

                // Send crash notification if notifier is configured
                if let Some(tx) = crash_notifier {
                    let _ = tx
                        .send(CrashNotification {
                            route_id: route_id_for_consumer.clone(),
                            error: error_msg.clone(),
                        })
                        .await;
                }

                publish_runtime_failure(runtime_for_consumer, &route_id_for_consumer, &error_msg)
                    .await;
            }
        });

        // Store consumer handle and update status
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check");
        managed.consumer_handle = Some(consumer_handle);

        info!(route_id = %route_id, "Route resumed");
        Ok(())
    }

    async fn start_all_routes(&mut self) -> Result<(), CamelError> {
        // Only start routes where auto_startup() == true
        // Sort by startup_order() ascending before starting
        let route_ids: Vec<String> = {
            let mut pairs: Vec<_> = self
                .routes
                .iter()
                .filter(|(_, r)| r.definition.auto_startup())
                .map(|(id, r)| (id.clone(), r.definition.startup_order()))
                .collect();
            pairs.sort_by_key(|(_, order)| *order);
            pairs.into_iter().map(|(id, _)| id).collect()
        };

        info!("Starting {} auto-startup routes", route_ids.len());

        // Collect errors but continue starting remaining routes
        let mut errors: Vec<String> = Vec::new();
        for route_id in route_ids {
            if let Err(e) = self.start_route(&route_id).await {
                errors.push(format!("Route '{}': {}", route_id, e));
            }
        }

        if !errors.is_empty() {
            return Err(CamelError::RouteError(format!(
                "Failed to start routes: {}",
                errors.join(", ")
            )));
        }

        info!("All auto-startup routes started");
        Ok(())
    }

    async fn stop_all_routes(&mut self) -> Result<(), CamelError> {
        // Sort by startup_order descending (reverse order)
        let route_ids: Vec<String> = {
            let mut pairs: Vec<_> = self
                .routes
                .iter()
                .map(|(id, r)| (id.clone(), r.definition.startup_order()))
                .collect();
            pairs.sort_by_key(|(_, order)| std::cmp::Reverse(*order));
            pairs.into_iter().map(|(id, _)| id).collect()
        };

        info!("Stopping {} routes", route_ids.len());

        for route_id in route_ids {
            let _ = self.stop_route(&route_id).await;
        }

        info!("All routes stopped");
        Ok(())
    }
}

#[async_trait::async_trait]
impl RouteControllerInternal for DefaultRouteController {
    fn add_route(&mut self, def: RouteDefinition) -> Result<(), CamelError> {
        DefaultRouteController::add_route(self, def)
    }

    fn swap_pipeline(&self, route_id: &str, pipeline: BoxProcessor) -> Result<(), CamelError> {
        DefaultRouteController::swap_pipeline(self, route_id, pipeline)
    }

    fn route_from_uri(&self, route_id: &str) -> Option<String> {
        // Call the inherent method which now returns Option<String>
        DefaultRouteController::route_from_uri(self, route_id)
    }

    fn set_error_handler(&mut self, config: ErrorHandlerConfig) {
        DefaultRouteController::set_error_handler(self, config)
    }

    fn set_self_ref(&mut self, self_ref: Arc<Mutex<dyn RouteController>>) {
        DefaultRouteController::set_self_ref(self, self_ref)
    }

    fn set_runtime_handle(&mut self, runtime: Arc<dyn RuntimeHandle>) {
        DefaultRouteController::set_runtime_handle(self, runtime)
    }

    fn route_count(&self) -> usize {
        DefaultRouteController::route_count(self)
    }

    fn route_ids(&self) -> Vec<String> {
        DefaultRouteController::route_ids(self)
    }

    fn auto_startup_route_ids(&self) -> Vec<String> {
        DefaultRouteController::auto_startup_route_ids(self)
    }

    fn shutdown_route_ids(&self) -> Vec<String> {
        DefaultRouteController::shutdown_route_ids(self)
    }

    fn set_tracer_config(&mut self, config: &TracerConfig) {
        DefaultRouteController::set_tracer_config(self, config)
    }

    fn compile_route_definition(&self, def: RouteDefinition) -> Result<BoxProcessor, CamelError> {
        DefaultRouteController::compile_route_definition(self, def)
    }

    fn remove_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        DefaultRouteController::remove_route(self, route_id)
    }

    async fn start_route_reload(&mut self, route_id: &str) -> Result<(), CamelError> {
        DefaultRouteController::start_route(self, route_id).await
    }

    async fn stop_route_reload(&mut self, route_id: &str) -> Result<(), CamelError> {
        DefaultRouteController::stop_route(self, route_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_route_controller_internal_is_object_safe() {
        let _: Option<Box<dyn RouteControllerInternal>> = None;
    }
}
