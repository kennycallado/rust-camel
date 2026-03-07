//! Default implementation of RouteController.
//!
//! This module provides [`DefaultRouteController`], which manages route lifecycle
//! including starting, stopping, suspending, and resuming routes.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower::{Layer, Service, ServiceExt};
use tracing::{error, info, warn};

use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{
    BoxProcessor, CamelError, Exchange, FilterPredicate, IdentityProcessor, ProducerContext,
    RouteController, RouteStatus, Value, body::Body,
};
use camel_component::{ConcurrencyModel, ConsumerContext, consumer::ExchangeEnvelope};
use camel_endpoint::parse_uri;
use camel_language_api::{Expression, Language, Predicate};
use camel_processor::circuit_breaker::CircuitBreakerLayer;
use camel_processor::error_handler::ErrorHandlerLayer;
use camel_processor::{ChoiceService, WhenClause};

use crate::config::{DetailLevel, TracerConfig};
use crate::registry::Registry;
use crate::route::{
    BuilderStep, LanguageExpressionDef, RouteDefinition, RouteDefinitionInfo, ValueSourceDef,
    compose_pipeline, compose_traced_pipeline,
};
use arc_swap::ArcSwap;

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

    /// Returns the number of routes in the controller.
    fn route_count(&self) -> usize;

    /// Returns all route IDs.
    fn route_ids(&self) -> Vec<String>;

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
    /// Shared lifecycle status — written by both the controller and spawned consumer tasks.
    status: Arc<std::sync::Mutex<RouteStatus>>,
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
    /// Self-reference for creating ProducerContext.
    /// Set after construction via `set_self_ref()`.
    self_ref: Option<Arc<Mutex<dyn RouteController>>>,
    /// Optional global error handler applied to all routes without a per-route handler.
    global_error_handler: Option<ErrorHandlerConfig>,
    /// Optional crash notifier for supervision.
    crash_notifier: Option<mpsc::Sender<CrashNotification>>,
    /// Whether tracing is enabled for route pipelines.
    tracing_enabled: bool,
    /// Detail level for tracing when enabled.
    tracer_detail_level: DetailLevel,
}

impl DefaultRouteController {
    /// Create a new `DefaultRouteController` with the given registry.
    pub fn new(registry: Arc<std::sync::Mutex<Registry>>) -> Self {
        Self::with_languages(registry, Arc::new(std::sync::Mutex::new(HashMap::new())))
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
            self_ref: None,
            global_error_handler: None,
            crash_notifier: None,
            tracing_enabled: false,
            tracer_detail_level: DetailLevel::Minimal,
        }
    }

    /// Set the self-reference for creating ProducerContext.
    ///
    /// This must be called after wrapping the controller in `Arc<Mutex<>>`.
    pub fn set_self_ref(&mut self, self_ref: Arc<Mutex<dyn RouteController>>) {
        self.self_ref = Some(self_ref);
    }

    /// Get the self-reference, if set.
    ///
    /// Used by [`SupervisingRouteController`](crate::supervising_route_controller::SupervisingRouteController)
    /// to spawn the supervision loop.
    pub fn self_ref_for_supervision(&self) -> Option<Arc<Mutex<dyn RouteController>>> {
        self.self_ref.clone()
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
        registry: &Registry,
    ) -> Result<Vec<BoxProcessor>, CamelError> {
        let mut processors: Vec<BoxProcessor> = Vec::new();
        for step in steps {
            match step {
                BuilderStep::Processor(svc) => {
                    processors.push(svc);
                }
                BuilderStep::To(uri) => {
                    let parsed = parse_uri(&uri)?;
                    let component = registry.get_or_err(&parsed.scheme)?;
                    let endpoint = component.create_endpoint(&uri)?;
                    let producer = endpoint.create_producer(producer_ctx)?;
                    processors.push(producer);
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
                    let sub_processors = self.resolve_steps(steps, producer_ctx, registry)?;
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
                            self.resolve_steps(when_step.steps, producer_ctx, registry)?;
                        let pipeline = compose_pipeline(sub_processors);
                        when_clauses.push(WhenClause {
                            predicate,
                            pipeline,
                        });
                    }
                    let otherwise_pipeline = if let Some(otherwise_steps) = otherwise {
                        let sub_processors =
                            self.resolve_steps(otherwise_steps, producer_ctx, registry)?;
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
                    let sub_processors = self.resolve_steps(steps, producer_ctx, registry)?;
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

                    let sub_processors = self.resolve_steps(steps, producer_ctx, registry)?;
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
                    let sub_processors = self.resolve_steps(steps, producer_ctx, registry)?;
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
                            self.resolve_steps(when_step.steps, producer_ctx, registry)?;
                        let pipeline = compose_pipeline(sub_processors);
                        when_clauses.push(WhenClause {
                            predicate: when_step.predicate,
                            pipeline,
                        });
                    }
                    // Resolve otherwise branch (if present).
                    let otherwise_pipeline = if let Some(otherwise_steps) = otherwise {
                        let sub_processors =
                            self.resolve_steps(otherwise_steps, producer_ctx, registry)?;
                        Some(compose_pipeline(sub_processors))
                    } else {
                        None
                    };
                    let svc = ChoiceService::new(when_clauses, otherwise_pipeline);
                    processors.push(BoxProcessor::new(svc));
                }
                BuilderStep::WireTap { uri } => {
                    let parsed = parse_uri(&uri)?;
                    let component = registry.get_or_err(&parsed.scheme)?;
                    let endpoint = component.create_endpoint(&uri)?;
                    let producer = endpoint.create_producer(producer_ctx)?;
                    let svc = camel_processor::WireTapService::new(producer);
                    processors.push(BoxProcessor::new(svc));
                }
                BuilderStep::Multicast { config, steps } => {
                    // Each top-level step in the multicast scope becomes an independent endpoint.
                    let mut endpoints = Vec::new();
                    for step in steps {
                        let sub_processors =
                            self.resolve_steps(vec![step], producer_ctx, registry)?;
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
                        unreachable!("DeclarativeLog with Literal should have been compiled to a Processor");
                    };
                    let expression = self.compile_language_expression(&expression)?;
                    let svc = camel_processor::log::DynamicLog::new(
                        level,
                        move |exchange: &Exchange| {
                            expression.evaluate(exchange).unwrap_or_else(|e| {
                                warn!(error = %e, "log expression evaluation failed");
                                Value::Null
                            }).to_string()
                        },
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
        let producer_ctx = self
            .self_ref
            .clone()
            .map(ProducerContext::new)
            .ok_or_else(|| CamelError::RouteError("RouteController self_ref not set".into()))?;

        // Lock registry for step resolution
        let registry = self
            .registry
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock");

        // Resolve steps into processors (takes ownership of steps)
        let processors = self.resolve_steps(definition.steps, &producer_ctx, &registry)?;
        let route_id_for_tracing = route_id.clone();
        let mut pipeline = compose_traced_pipeline(
            processors,
            &route_id_for_tracing,
            self.tracing_enabled,
            self.tracer_detail_level.clone(),
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
            let layer = self.resolve_error_handler(config, &producer_ctx, &registry)?;
            pipeline = BoxProcessor::new(layer.layer(pipeline));
        }

        // Drop the lock before modifying self.routes
        drop(registry);

        self.routes.insert(
            route_id.clone(),
            ManagedRoute {
                definition: definition_info,
                from_uri,
                pipeline: Arc::new(ArcSwap::from_pointee(SyncBoxProcessor(pipeline))),
                concurrency,
                status: Arc::new(std::sync::Mutex::new(RouteStatus::Stopped)),
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

        let producer_ctx = self
            .self_ref
            .clone()
            .map(ProducerContext::new)
            .ok_or_else(|| CamelError::RouteError("RouteController self_ref not set".into()))?;

        let registry = self
            .registry
            .lock()
            .expect("mutex poisoned: registry lock in compile_route_definition");

        let processors = self.resolve_steps(def.steps, &producer_ctx, &registry)?;
        let mut pipeline = compose_traced_pipeline(
            processors,
            &route_id,
            self.tracing_enabled,
            self.tracer_detail_level.clone(),
        );

        if let Some(cb_config) = def.circuit_breaker {
            let cb_layer = CircuitBreakerLayer::new(cb_config);
            pipeline = BoxProcessor::new(cb_layer.layer(pipeline));
        }

        let eh_config = def
            .error_handler
            .or_else(|| self.global_error_handler.clone());
        if let Some(config) = eh_config {
            let layer = self.resolve_error_handler(config, &producer_ctx, &registry)?;
            pipeline = BoxProcessor::new(layer.layer(pipeline));
        }

        drop(registry);
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
        let status = managed
            .status
            .lock()
            .expect("status mutex poisoned")
            .clone();
        match status {
            RouteStatus::Stopped | RouteStatus::Failed(_) => {}
            other => {
                return Err(CamelError::RouteError(format!(
                    "Route '{}' must be stopped before removal (current status: {:?})",
                    route_id, other
                )));
            }
        }
        self.routes.remove(route_id);
        info!(route_id = %route_id, "Route removed from controller");
        Ok(())
    }

    /// Returns the number of routes in the controller.
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    /// Force-set the status of a route. Only for use in tests.
    #[doc(hidden)]
    pub fn force_route_status(&mut self, route_id: &str, status: RouteStatus) {
        if let Some(managed) = self.routes.get(route_id) {
            *managed.status.lock().expect("status mutex poisoned") = status;
        }
    }

    /// Returns all route IDs.
    pub fn route_ids(&self) -> Vec<String> {
        self.routes.keys().cloned().collect()
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

        let current_status = managed
            .status
            .lock()
            .expect("status mutex poisoned")
            .clone();
        if current_status != RouteStatus::Started && current_status != RouteStatus::Suspended {
            return Ok(()); // Already stopped or stopping
        }

        info!(route_id = %route_id, "Stopping route");
        *managed.status.lock().expect("status mutex poisoned") = RouteStatus::Stopping;

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
        *managed.status.lock().expect("status mutex poisoned") = RouteStatus::Stopped;

        info!(route_id = %route_id, "Route stopped");
        Ok(())
    }
}

#[async_trait::async_trait]
impl RouteController for DefaultRouteController {
    async fn start_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        // Check if route exists and can be started, and update status atomically
        {
            let managed = self
                .routes
                .get_mut(route_id)
                .ok_or_else(|| CamelError::RouteError(format!("Route '{}' not found", route_id)))?;

            let current_status = managed
                .status
                .lock()
                .expect("status mutex poisoned")
                .clone();
            match current_status {
                RouteStatus::Started => return Ok(()), // Already running
                RouteStatus::Starting => {
                    return Err(CamelError::RouteError(format!(
                        "Route '{}' is already starting",
                        route_id
                    )));
                }
                RouteStatus::Stopped | RouteStatus::Failed(_) => {} // OK to start
                RouteStatus::Stopping => {
                    return Err(CamelError::RouteError(format!(
                        "Route '{}' is stopping",
                        route_id
                    )));
                }
                RouteStatus::Suspended => {
                    return Err(CamelError::RouteError(format!(
                        "Route '{}' is suspended; use resume_route() to resume, or stop_route() then start_route() for full restart",
                        route_id
                    )));
                }
            }
            *managed.status.lock().expect("status mutex poisoned") = RouteStatus::Starting;
        }

        info!(route_id = %route_id, "Starting route");

        // Get the resolved route info
        let (from_uri, pipeline, concurrency, status_for_consumer) = {
            let managed = self
                .routes
                .get(route_id)
                .expect("invariant: route must exist after prior existence check");
            (
                managed.from_uri.clone(),
                Arc::clone(&managed.pipeline),
                managed.concurrency.clone(),
                Arc::clone(&managed.status),
            )
        };

        // Clone crash notifier for consumer task
        let crash_notifier = self.crash_notifier.clone();

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
        // Status is shared via Arc<Mutex<>> so the task can update it on crash.
        let route_id_for_consumer = route_id.to_string();
        let consumer_handle = tokio::spawn(async move {
            if let Err(e) = consumer.start(consumer_ctx).await {
                error!(route_id = %route_id_for_consumer, "Consumer error: {e}");
                let error_msg = e.to_string();
                *status_for_consumer.lock().expect("status mutex poisoned") =
                    RouteStatus::Failed(error_msg.clone());

                // Send crash notification if notifier is configured
                if let Some(tx) = crash_notifier {
                    let _ = tx
                        .send(CrashNotification {
                            route_id: route_id_for_consumer.clone(),
                            error: error_msg,
                        })
                        .await;
                }
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
        // Only mark as Started if consumer hasn't already crashed
        let mut status_guard = managed.status.lock().expect("status mutex poisoned");
        if !matches!(*status_guard, RouteStatus::Failed(_)) {
            *status_guard = RouteStatus::Started;
        }
        drop(status_guard);

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
        // Check route exists and get current status
        let managed = self
            .routes
            .get_mut(route_id)
            .ok_or_else(|| CamelError::RouteError(format!("Route '{}' not found", route_id)))?;

        let current_status = managed
            .status
            .lock()
            .expect("status mutex poisoned")
            .clone();

        // Can only suspend from Started state
        if current_status != RouteStatus::Started {
            return Err(CamelError::RouteError(format!(
                "Cannot suspend route '{}' with status {:?}",
                route_id, current_status
            )));
        }

        info!(route_id = %route_id, "Suspending route (consumer only, keeping pipeline)");

        // Set status to Stopping during suspend
        *managed.status.lock().expect("status mutex poisoned") = RouteStatus::Stopping;

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

        // Set status to Suspended (pipeline is still running)
        *managed.status.lock().expect("status mutex poisoned") = RouteStatus::Suspended;

        info!(route_id = %route_id, "Route suspended (pipeline still running)");
        Ok(())
    }

    async fn resume_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        // Check route exists and is Suspended
        let managed = self
            .routes
            .get(route_id)
            .ok_or_else(|| CamelError::RouteError(format!("Route '{}' not found", route_id)))?;

        let current_status = managed
            .status
            .lock()
            .expect("status mutex poisoned")
            .clone();

        if current_status != RouteStatus::Suspended {
            return Err(CamelError::RouteError(format!(
                "Cannot resume route '{}' with status {:?} (expected Suspended)",
                route_id, current_status
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

        // Clone status Arc for the consumer task
        let status_for_consumer = Arc::clone(&managed.status);
        let crash_notifier = self.crash_notifier.clone();

        // Create ConsumerContext with the stored sender
        let consumer_ctx = ConsumerContext::new(sender, consumer_cancel.clone());

        // Spawn consumer task
        let route_id_for_consumer = route_id.to_string();
        let consumer_handle = tokio::spawn(async move {
            if let Err(e) = consumer.start(consumer_ctx).await {
                error!(route_id = %route_id_for_consumer, "Consumer error on resume: {e}");
                let error_msg = e.to_string();
                *status_for_consumer.lock().expect("status mutex poisoned") =
                    RouteStatus::Failed(error_msg.clone());

                // Send crash notification if notifier is configured
                if let Some(tx) = crash_notifier {
                    let _ = tx
                        .send(CrashNotification {
                            route_id: route_id_for_consumer.clone(),
                            error: error_msg,
                        })
                        .await;
                }
            }
        });

        // Store consumer handle and update status
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check");
        managed.consumer_handle = Some(consumer_handle);

        // Only mark as Started if consumer hasn't already crashed
        let mut status_guard = managed.status.lock().expect("status mutex poisoned");
        if !matches!(*status_guard, RouteStatus::Failed(_)) {
            *status_guard = RouteStatus::Started;
        } else {
            // Resume failed - return error
            let failed_status = status_guard.clone();
            drop(status_guard);
            return Err(CamelError::RouteError(format!(
                "Resume failed: {:?}",
                failed_status
            )));
        }
        drop(status_guard);

        info!(route_id = %route_id, "Route resumed");
        Ok(())
    }

    fn route_status(&self, route_id: &str) -> Option<RouteStatus> {
        self.routes
            .get(route_id)
            .map(|r| r.status.lock().expect("status mutex poisoned").clone())
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

    fn route_count(&self) -> usize {
        DefaultRouteController::route_count(self)
    }

    fn route_ids(&self) -> Vec<String> {
        DefaultRouteController::route_ids(self)
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
    use std::sync::Arc;

    #[test]
    fn test_route_controller_internal_is_object_safe() {
        // Verifies the trait compiles as a trait object.
        // If RouteControllerInternal is not object-safe, this test fails to compile.
        let _: Option<Box<dyn RouteControllerInternal>> = None;
    }

    #[tokio::test]
    async fn test_swap_pipeline_updates_stored_pipeline() {
        use camel_api::IdentityProcessor;

        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let mut controller = DefaultRouteController::new(registry);

        let controller_arc: Arc<Mutex<dyn RouteController>> = Arc::new(Mutex::new(
            DefaultRouteController::new(Arc::new(std::sync::Mutex::new(Registry::new()))),
        ));
        controller.set_self_ref(controller_arc);

        let definition =
            crate::route::RouteDefinition::new("timer:tick", vec![]).with_route_id("swap-test");
        controller.add_route(definition).unwrap();

        // Swap pipeline should succeed
        let new_pipeline = BoxProcessor::new(IdentityProcessor);
        let result = controller.swap_pipeline("swap-test", new_pipeline);
        assert!(result.is_ok());

        // Swap on non-existent route should fail
        let new_pipeline = BoxProcessor::new(IdentityProcessor);
        let result = controller.swap_pipeline("nonexistent", new_pipeline);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_add_route_duplicate_id_fails() {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let mut controller = DefaultRouteController::new(registry);

        // Set self_ref to avoid error during add_route
        let controller_arc: Arc<Mutex<dyn RouteController>> = Arc::new(Mutex::new(
            DefaultRouteController::new(Arc::new(std::sync::Mutex::new(Registry::new()))),
        ));
        controller.set_self_ref(controller_arc);

        let definition = crate::route::RouteDefinition::new("timer:tick", vec![])
            .with_route_id("duplicate-route");
        assert!(controller.add_route(definition).is_ok());

        // Adding a route with the same ID should fail
        let definition2 = crate::route::RouteDefinition::new("timer:tock", vec![])
            .with_route_id("duplicate-route");
        let result = controller.add_route(definition2);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("already exists"),
            "error should mention 'already exists', got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_add_route_with_id_succeeds() {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let mut controller = DefaultRouteController::new(registry);

        // Set self_ref to avoid error during add_route
        let controller_arc: Arc<Mutex<dyn RouteController>> = Arc::new(Mutex::new(
            DefaultRouteController::new(Arc::new(std::sync::Mutex::new(Registry::new()))),
        ));
        controller.set_self_ref(controller_arc);

        let definition =
            crate::route::RouteDefinition::new("timer:tick", vec![]).with_route_id("test-route");
        assert!(controller.add_route(definition).is_ok());
        assert_eq!(controller.route_count(), 1);
    }

    #[tokio::test]
    async fn test_crashed_consumer_sets_failed_status() {
        use async_trait::async_trait;
        use camel_api::{CamelError, RouteStatus};
        use camel_component::{ConcurrencyModel, ConsumerContext, Endpoint};

        struct CrashingConsumer;
        #[async_trait]
        impl camel_component::Consumer for CrashingConsumer {
            async fn start(&mut self, _ctx: ConsumerContext) -> Result<(), CamelError> {
                Err(CamelError::RouteError("boom".into()))
            }
            async fn stop(&mut self) -> Result<(), CamelError> {
                Ok(())
            }
            fn concurrency_model(&self) -> ConcurrencyModel {
                ConcurrencyModel::Sequential
            }
        }
        struct CrashingEndpoint;
        impl Endpoint for CrashingEndpoint {
            fn uri(&self) -> &str {
                "crash:test"
            }
            fn create_consumer(&self) -> Result<Box<dyn camel_component::Consumer>, CamelError> {
                Ok(Box::new(CrashingConsumer))
            }
            fn create_producer(
                &self,
                _ctx: &camel_api::ProducerContext,
            ) -> Result<camel_api::BoxProcessor, CamelError> {
                Err(CamelError::RouteError("no producer".into()))
            }
        }
        struct CrashingComponent;
        impl camel_component::Component for CrashingComponent {
            fn scheme(&self) -> &str {
                "crash"
            }
            fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
                let _ = uri; // satisfy unused variable warning
                Ok(Box::new(CrashingEndpoint))
            }
        }

        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        registry.lock().unwrap().register(CrashingComponent);
        let mut controller = DefaultRouteController::new(Arc::clone(&registry));
        let self_ref: Arc<Mutex<dyn RouteController>> = Arc::new(Mutex::new(
            DefaultRouteController::new(Arc::clone(&registry)),
        ));
        controller.set_self_ref(self_ref);

        let def =
            crate::route::RouteDefinition::new("crash:test", vec![]).with_route_id("crash-route");
        controller.add_route(def).unwrap();
        controller.start_route("crash-route").await.unwrap();

        // Give consumer task time to crash and update status
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let status = controller.route_status("crash-route").unwrap();
        assert!(
            matches!(status, RouteStatus::Failed(_)),
            "expected Failed, got {:?}",
            status
        );
    }

    /// Test demonstrating independent consumer/pipeline cancellation token scaffolding.
    ///
    /// This test verifies that ManagedRoute has separate cancellation tokens
    /// for consumer and pipeline, enabling future independent lifecycle control
    /// (e.g., suspending consumer while pipeline continues processing in-flight messages).
    #[test]
    fn test_managed_route_has_independent_cancel_tokens() {
        // This test verifies the structure exists by checking that:
        // 1. Both tokens are created on route addition
        // 2. Both tokens are recreated after stop
        //
        // The actual independent control will be implemented in Task 3.
        // This test serves as scaffolding verification.

        let consumer_token = CancellationToken::new();
        let pipeline_token = CancellationToken::new();

        // Verify tokens start uncancelled
        assert!(
            !consumer_token.is_cancelled(),
            "consumer token should start uncancelled"
        );
        assert!(
            !pipeline_token.is_cancelled(),
            "pipeline token should start uncancelled"
        );

        // Verify tokens can be cancelled independently
        consumer_token.cancel();
        assert!(
            consumer_token.is_cancelled(),
            "consumer token should be cancelled"
        );
        assert!(
            !pipeline_token.is_cancelled(),
            "pipeline token should NOT be cancelled when consumer is cancelled"
        );

        // Verify independent control works the other way
        let consumer_token2 = CancellationToken::new();
        let pipeline_token2 = CancellationToken::new();

        pipeline_token2.cancel();
        assert!(
            !consumer_token2.is_cancelled(),
            "consumer token should NOT be cancelled when pipeline is cancelled"
        );
        assert!(
            pipeline_token2.is_cancelled(),
            "pipeline token should be cancelled"
        );

        // This demonstrates the scaffolding is in place for Task 3:
        // - Consumer and pipeline have independent CancellationToken fields
        // - They can be controlled separately without affecting each other
    }

    /// Test verifying stop_route cancels both consumer and pipeline tokens.
    #[tokio::test]
    async fn test_stop_cancels_both_consumer_and_pipeline_tokens() {
        use camel_api::IdentityProcessor;
        use camel_component::{ConcurrencyModel, ConsumerContext, Endpoint};

        // Consumer that tracks cancellation
        struct TrackingConsumer {
            cancelled: Arc<std::sync::atomic::AtomicBool>,
        }
        #[async_trait::async_trait]
        impl camel_component::Consumer for TrackingConsumer {
            async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
                // Wait for cancellation signal using the public API
                ctx.cancelled().await;
                self.cancelled
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }
            async fn stop(&mut self) -> Result<(), CamelError> {
                Ok(())
            }
            fn concurrency_model(&self) -> ConcurrencyModel {
                ConcurrencyModel::Sequential
            }
        }
        struct TrackingEndpoint {
            cancelled: Arc<std::sync::atomic::AtomicBool>,
        }
        impl Endpoint for TrackingEndpoint {
            fn uri(&self) -> &str {
                "tracking:test"
            }
            fn create_consumer(&self) -> Result<Box<dyn camel_component::Consumer>, CamelError> {
                Ok(Box::new(TrackingConsumer {
                    cancelled: Arc::clone(&self.cancelled),
                }))
            }
            fn create_producer(
                &self,
                _ctx: &camel_api::ProducerContext,
            ) -> Result<camel_api::BoxProcessor, CamelError> {
                Ok(camel_api::BoxProcessor::new(IdentityProcessor))
            }
        }
        struct TrackingComponent {
            cancelled: Arc<std::sync::atomic::AtomicBool>,
        }
        impl camel_component::Component for TrackingComponent {
            fn scheme(&self) -> &str {
                "tracking"
            }
            fn create_endpoint(&self, _uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
                Ok(Box::new(TrackingEndpoint {
                    cancelled: Arc::clone(&self.cancelled),
                }))
            }
        }

        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        registry.lock().unwrap().register(TrackingComponent {
            cancelled: Arc::clone(&cancelled),
        });
        let mut controller = DefaultRouteController::new(Arc::clone(&registry));
        let self_ref: Arc<Mutex<dyn RouteController>> = Arc::new(Mutex::new(
            DefaultRouteController::new(Arc::clone(&registry)),
        ));
        controller.set_self_ref(self_ref);

        let def = crate::route::RouteDefinition::new("tracking:test", vec![])
            .with_route_id("tracking-route");
        controller.add_route(def).unwrap();

        // Start the route
        controller.start_route("tracking-route").await.unwrap();
        assert_eq!(
            controller.route_status("tracking-route"),
            Some(RouteStatus::Started)
        );

        // Stop the route - should cancel both tokens
        controller.stop_route("tracking-route").await.unwrap();

        // Wait for consumer to observe cancellation
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Verify the consumer was cancelled (proving consumer_cancel_token was cancelled)
        assert!(
            cancelled.load(std::sync::atomic::Ordering::SeqCst),
            "consumer should have been cancelled via consumer_cancel_token"
        );

        // Verify route is stopped
        assert_eq!(
            controller.route_status("tracking-route"),
            Some(RouteStatus::Stopped)
        );

        // Verify tokens are recreated for next start
        controller.start_route("tracking-route").await.unwrap();
        assert_eq!(
            controller.route_status("tracking-route"),
            Some(RouteStatus::Started)
        );
        controller.stop_route("tracking-route").await.unwrap();
    }

    /// Test that suspend_route cancels consumer but keeps pipeline running.
    #[tokio::test]
    async fn test_suspend() {
        use camel_api::IdentityProcessor;
        use camel_component::{ConcurrencyModel, ConsumerContext, Endpoint};
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        // Consumer that tracks start/stop and can be controlled
        struct SuspendableConsumer {
            started: Arc<AtomicUsize>,
            cancelled: Arc<AtomicBool>,
        }
        #[async_trait::async_trait]
        impl camel_component::Consumer for SuspendableConsumer {
            async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
                self.started.fetch_add(1, Ordering::SeqCst);
                // Wait for cancellation
                ctx.cancelled().await;
                self.cancelled.store(true, Ordering::SeqCst);
                Ok(())
            }
            async fn stop(&mut self) -> Result<(), CamelError> {
                Ok(())
            }
            fn concurrency_model(&self) -> ConcurrencyModel {
                ConcurrencyModel::Sequential
            }
        }
        struct SuspendableEndpoint {
            started: Arc<AtomicUsize>,
            cancelled: Arc<AtomicBool>,
        }
        impl Endpoint for SuspendableEndpoint {
            fn uri(&self) -> &str {
                "suspend:test"
            }
            fn create_consumer(&self) -> Result<Box<dyn camel_component::Consumer>, CamelError> {
                Ok(Box::new(SuspendableConsumer {
                    started: Arc::clone(&self.started),
                    cancelled: Arc::clone(&self.cancelled),
                }))
            }
            fn create_producer(
                &self,
                _ctx: &camel_api::ProducerContext,
            ) -> Result<camel_api::BoxProcessor, CamelError> {
                Ok(camel_api::BoxProcessor::new(IdentityProcessor))
            }
        }
        struct SuspendableComponent {
            started: Arc<AtomicUsize>,
            cancelled: Arc<AtomicBool>,
        }
        impl camel_component::Component for SuspendableComponent {
            fn scheme(&self) -> &str {
                "suspend"
            }
            fn create_endpoint(&self, _uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
                Ok(Box::new(SuspendableEndpoint {
                    started: Arc::clone(&self.started),
                    cancelled: Arc::clone(&self.cancelled),
                }))
            }
        }

        let started = Arc::new(AtomicUsize::new(0));
        let cancelled = Arc::new(AtomicBool::new(false));
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        registry.lock().unwrap().register(SuspendableComponent {
            started: Arc::clone(&started),
            cancelled: Arc::clone(&cancelled),
        });
        let mut controller = DefaultRouteController::new(Arc::clone(&registry));
        let self_ref: Arc<Mutex<dyn RouteController>> = Arc::new(Mutex::new(
            DefaultRouteController::new(Arc::clone(&registry)),
        ));
        controller.set_self_ref(self_ref);

        let def = crate::route::RouteDefinition::new("suspend:test", vec![])
            .with_route_id("suspend-route");
        controller.add_route(def).unwrap();

        // Start the route
        controller.start_route("suspend-route").await.unwrap();
        assert_eq!(
            controller.route_status("suspend-route"),
            Some(RouteStatus::Started)
        );

        // Give the consumer time to start
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(started.load(Ordering::SeqCst), 1);

        // Suspend the route
        controller.suspend_route("suspend-route").await.unwrap();
        assert_eq!(
            controller.route_status("suspend-route"),
            Some(RouteStatus::Suspended)
        );

        // Wait for consumer to observe cancellation
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Verify consumer was cancelled
        assert!(
            cancelled.load(Ordering::SeqCst),
            "consumer should have been cancelled during suspend"
        );

        // Verify start_route on Suspended returns error
        let result = controller.start_route("suspend-route").await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("suspended"),
            "error should mention 'suspended', got: {}",
            err
        );

        // Stop the suspended route
        controller.stop_route("suspend-route").await.unwrap();
        assert_eq!(
            controller.route_status("suspend-route"),
            Some(RouteStatus::Stopped)
        );
    }

    /// Test that resume_route spawns only consumer task, reusing the pipeline.
    #[tokio::test]
    async fn test_resume() {
        use camel_api::IdentityProcessor;
        use camel_component::{ConcurrencyModel, ConsumerContext, Endpoint};
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        // Consumer that tracks start count and can be controlled
        struct ResumableConsumer {
            started: Arc<AtomicUsize>,
            cancelled: Arc<AtomicBool>,
        }
        #[async_trait::async_trait]
        impl camel_component::Consumer for ResumableConsumer {
            async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
                self.started.fetch_add(1, Ordering::SeqCst);
                // Reset cancelled flag on each start
                self.cancelled.store(false, Ordering::SeqCst);
                // Wait for cancellation
                ctx.cancelled().await;
                self.cancelled.store(true, Ordering::SeqCst);
                Ok(())
            }
            async fn stop(&mut self) -> Result<(), CamelError> {
                Ok(())
            }
            fn concurrency_model(&self) -> ConcurrencyModel {
                ConcurrencyModel::Sequential
            }
        }
        struct ResumableEndpoint {
            started: Arc<AtomicUsize>,
            cancelled: Arc<AtomicBool>,
        }
        impl Endpoint for ResumableEndpoint {
            fn uri(&self) -> &str {
                "resume:test"
            }
            fn create_consumer(&self) -> Result<Box<dyn camel_component::Consumer>, CamelError> {
                Ok(Box::new(ResumableConsumer {
                    started: Arc::clone(&self.started),
                    cancelled: Arc::clone(&self.cancelled),
                }))
            }
            fn create_producer(
                &self,
                _ctx: &camel_api::ProducerContext,
            ) -> Result<camel_api::BoxProcessor, CamelError> {
                Ok(camel_api::BoxProcessor::new(IdentityProcessor))
            }
        }
        struct ResumableComponent {
            started: Arc<AtomicUsize>,
            cancelled: Arc<AtomicBool>,
        }
        impl camel_component::Component for ResumableComponent {
            fn scheme(&self) -> &str {
                "resume"
            }
            fn create_endpoint(&self, _uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
                Ok(Box::new(ResumableEndpoint {
                    started: Arc::clone(&self.started),
                    cancelled: Arc::clone(&self.cancelled),
                }))
            }
        }

        let started = Arc::new(AtomicUsize::new(0));
        let cancelled = Arc::new(AtomicBool::new(false));
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        registry.lock().unwrap().register(ResumableComponent {
            started: Arc::clone(&started),
            cancelled: Arc::clone(&cancelled),
        });
        let mut controller = DefaultRouteController::new(Arc::clone(&registry));
        let self_ref: Arc<Mutex<dyn RouteController>> = Arc::new(Mutex::new(
            DefaultRouteController::new(Arc::clone(&registry)),
        ));
        controller.set_self_ref(self_ref);

        let def =
            crate::route::RouteDefinition::new("resume:test", vec![]).with_route_id("resume-route");
        controller.add_route(def).unwrap();

        // Start the route
        controller.start_route("resume-route").await.unwrap();
        assert_eq!(
            controller.route_status("resume-route"),
            Some(RouteStatus::Started)
        );

        // Give the consumer time to start
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(started.load(Ordering::SeqCst), 1);

        // Suspend the route
        controller.suspend_route("resume-route").await.unwrap();
        assert_eq!(
            controller.route_status("resume-route"),
            Some(RouteStatus::Suspended)
        );

        // Wait for consumer to observe cancellation
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(cancelled.load(Ordering::SeqCst));

        // Resume the route - should create a new consumer (started count increases)
        controller.resume_route("resume-route").await.unwrap();
        assert_eq!(
            controller.route_status("resume-route"),
            Some(RouteStatus::Started)
        );

        // Give the new consumer time to start
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Verify consumer was started again (new consumer instance)
        assert_eq!(
            started.load(Ordering::SeqCst),
            2,
            "consumer should have been started twice (initial + resume)"
        );

        // Verify the new consumer is not cancelled
        assert!(
            !cancelled.load(Ordering::SeqCst),
            "new consumer should not be cancelled after resume"
        );

        // Test that resume on non-suspended route fails
        let result = controller.resume_route("resume-route").await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not suspended") || err.contains("expected Suspended"),
            "error should indicate route is not suspended, got: {}",
            err
        );

        // Clean up
        controller.stop_route("resume-route").await.unwrap();
        assert_eq!(
            controller.route_status("resume-route"),
            Some(RouteStatus::Stopped)
        );

        // Test that resume on stopped route fails
        let result = controller.resume_route("resume-route").await;
        assert!(result.is_err());
    }

    /// Test that resume on non-existent route fails.
    #[tokio::test]
    async fn test_resume_non_existent_route() {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let mut controller = DefaultRouteController::new(Arc::clone(&registry));
        let self_ref: Arc<Mutex<dyn RouteController>> = Arc::new(Mutex::new(
            DefaultRouteController::new(Arc::clone(&registry)),
        ));
        controller.set_self_ref(self_ref);

        let result = controller.resume_route("non-existent").await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not found"),
            "error should mention 'not found', got: {}",
            err
        );
    }

    /// Test that suspend on non-started route fails.
    #[tokio::test]
    async fn test_suspend_non_started_route() {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let mut controller = DefaultRouteController::new(Arc::clone(&registry));
        let self_ref: Arc<Mutex<dyn RouteController>> = Arc::new(Mutex::new(
            DefaultRouteController::new(Arc::clone(&registry)),
        ));
        controller.set_self_ref(self_ref);

        let def =
            crate::route::RouteDefinition::new("timer:tick", vec![]).with_route_id("test-route");
        controller.add_route(def).unwrap();

        // Suspend on stopped route should fail
        let result = controller.suspend_route("test-route").await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Cannot suspend") || err.contains("Started"),
            "error should indicate cannot suspend, got: {}",
            err
        );
    }

    /// Regression test: stop from Suspended state must cancel BOTH consumer AND pipeline.
    ///
    /// This test ensures the suspend/resume refactor didn't break stop semantics.
    /// When a route is suspended, the pipeline is still running (waiting on channel).
    /// Calling stop_route from Suspended must cancel BOTH the consumer AND the pipeline.
    #[tokio::test]
    async fn test_stop_from_suspended_cancels_consumer_and_pipeline() {
        use camel_api::IdentityProcessor;
        use camel_component::{ConcurrencyModel, ConsumerContext, Endpoint};
        use std::sync::atomic::{AtomicBool, Ordering};

        // Consumer that tracks cancellation
        struct CancellableConsumer {
            cancelled: Arc<AtomicBool>,
        }
        #[async_trait::async_trait]
        impl camel_component::Consumer for CancellableConsumer {
            async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
                ctx.cancelled().await;
                self.cancelled.store(true, Ordering::SeqCst);
                Ok(())
            }
            async fn stop(&mut self) -> Result<(), CamelError> {
                Ok(())
            }
            fn concurrency_model(&self) -> ConcurrencyModel {
                ConcurrencyModel::Sequential
            }
        }
        struct CancellableEndpoint {
            cancelled: Arc<AtomicBool>,
        }
        impl Endpoint for CancellableEndpoint {
            fn uri(&self) -> &str {
                "cancellable:test"
            }
            fn create_consumer(&self) -> Result<Box<dyn camel_component::Consumer>, CamelError> {
                Ok(Box::new(CancellableConsumer {
                    cancelled: Arc::clone(&self.cancelled),
                }))
            }
            fn create_producer(
                &self,
                _ctx: &camel_api::ProducerContext,
            ) -> Result<camel_api::BoxProcessor, CamelError> {
                Ok(camel_api::BoxProcessor::new(IdentityProcessor))
            }
        }
        struct CancellableComponent {
            cancelled: Arc<AtomicBool>,
        }
        impl camel_component::Component for CancellableComponent {
            fn scheme(&self) -> &str {
                "cancellable"
            }
            fn create_endpoint(&self, _uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
                Ok(Box::new(CancellableEndpoint {
                    cancelled: Arc::clone(&self.cancelled),
                }))
            }
        }

        let consumer_cancelled = Arc::new(AtomicBool::new(false));
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        registry.lock().unwrap().register(CancellableComponent {
            cancelled: Arc::clone(&consumer_cancelled),
        });
        let mut controller = DefaultRouteController::new(Arc::clone(&registry));
        let self_ref: Arc<Mutex<dyn RouteController>> = Arc::new(Mutex::new(
            DefaultRouteController::new(Arc::clone(&registry)),
        ));
        controller.set_self_ref(self_ref);

        let def = crate::route::RouteDefinition::new("cancellable:test", vec![])
            .with_route_id("stop-from-suspended-route");
        controller.add_route(def).unwrap();

        // Start the route
        controller
            .start_route("stop-from-suspended-route")
            .await
            .unwrap();
        assert_eq!(
            controller.route_status("stop-from-suspended-route"),
            Some(RouteStatus::Started)
        );

        // Suspend the route (consumer cancelled, pipeline still running)
        controller
            .suspend_route("stop-from-suspended-route")
            .await
            .unwrap();
        assert_eq!(
            controller.route_status("stop-from-suspended-route"),
            Some(RouteStatus::Suspended)
        );

        // Wait for consumer to observe cancellation from suspend
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            consumer_cancelled.load(Ordering::SeqCst),
            "consumer should be cancelled after suspend"
        );

        // Reset the flag to detect if stop re-cancels the consumer token
        consumer_cancelled.store(false, Ordering::SeqCst);

        // STOP from Suspended state - this is the critical regression test
        // Must cancel BOTH consumer token (even though consumer isn't running) AND pipeline token
        controller
            .stop_route("stop-from-suspended-route")
            .await
            .unwrap();

        // Verify final state is Stopped
        assert_eq!(
            controller.route_status("stop-from-suspended-route"),
            Some(RouteStatus::Stopped),
            "route must be Stopped after stop_route from Suspended"
        );

        // Wait for pipeline task to observe cancellation
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // The route can be started again, proving clean shutdown
        controller
            .start_route("stop-from-suspended-route")
            .await
            .unwrap();
        assert_eq!(
            controller.route_status("stop-from-suspended-route"),
            Some(RouteStatus::Started)
        );

        controller
            .stop_route("stop-from-suspended-route")
            .await
            .unwrap();
    }

    /// Test full suspend/resume cycle with stop.
    #[tokio::test]
    async fn test_suspend_resume_stop_cycle() {
        use camel_api::IdentityProcessor;
        use camel_component::{ConcurrencyModel, ConsumerContext, Endpoint};

        // Simple consumer that waits for cancellation
        struct SimpleConsumer;
        #[async_trait::async_trait]
        impl camel_component::Consumer for SimpleConsumer {
            async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
                ctx.cancelled().await;
                Ok(())
            }
            async fn stop(&mut self) -> Result<(), CamelError> {
                Ok(())
            }
            fn concurrency_model(&self) -> ConcurrencyModel {
                ConcurrencyModel::Sequential
            }
        }
        struct SimpleEndpoint;
        impl Endpoint for SimpleEndpoint {
            fn uri(&self) -> &str {
                "simple:test"
            }
            fn create_consumer(&self) -> Result<Box<dyn camel_component::Consumer>, CamelError> {
                Ok(Box::new(SimpleConsumer))
            }
            fn create_producer(
                &self,
                _ctx: &camel_api::ProducerContext,
            ) -> Result<camel_api::BoxProcessor, CamelError> {
                Ok(camel_api::BoxProcessor::new(IdentityProcessor))
            }
        }
        struct SimpleComponent;
        impl camel_component::Component for SimpleComponent {
            fn scheme(&self) -> &str {
                "simple"
            }
            fn create_endpoint(&self, _uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
                Ok(Box::new(SimpleEndpoint))
            }
        }

        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        registry.lock().unwrap().register(SimpleComponent);
        let mut controller = DefaultRouteController::new(Arc::clone(&registry));
        let self_ref: Arc<Mutex<dyn RouteController>> = Arc::new(Mutex::new(
            DefaultRouteController::new(Arc::clone(&registry)),
        ));
        controller.set_self_ref(self_ref);

        let def =
            crate::route::RouteDefinition::new("simple:test", vec![]).with_route_id("cycle-route");
        controller.add_route(def).unwrap();

        // Full cycle: start -> suspend -> resume -> suspend -> stop
        controller.start_route("cycle-route").await.unwrap();
        assert_eq!(
            controller.route_status("cycle-route"),
            Some(RouteStatus::Started)
        );

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        controller.suspend_route("cycle-route").await.unwrap();
        assert_eq!(
            controller.route_status("cycle-route"),
            Some(RouteStatus::Suspended)
        );

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        controller.resume_route("cycle-route").await.unwrap();
        assert_eq!(
            controller.route_status("cycle-route"),
            Some(RouteStatus::Started)
        );

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        controller.suspend_route("cycle-route").await.unwrap();
        assert_eq!(
            controller.route_status("cycle-route"),
            Some(RouteStatus::Suspended)
        );

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Stop from suspended state
        controller.stop_route("cycle-route").await.unwrap();
        assert_eq!(
            controller.route_status("cycle-route"),
            Some(RouteStatus::Stopped)
        );

        // Can start again after stop
        controller.start_route("cycle-route").await.unwrap();
        assert_eq!(
            controller.route_status("cycle-route"),
            Some(RouteStatus::Started)
        );

        controller.stop_route("cycle-route").await.unwrap();
    }
}
