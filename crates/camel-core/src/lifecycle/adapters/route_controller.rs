//! Default implementation of RouteController.
//!
//! This module provides [`DefaultRouteController`], which manages route lifecycle
//! including starting, stopping, suspending, and resuming routes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower::{Layer, Service, ServiceExt};
use tracing::{error, info, warn};

use camel_api::UnitOfWorkConfig;
use camel_api::aggregator::AggregatorConfig;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::metrics::MetricsCollector;
use camel_api::{
    BoxProcessor, CamelError, Exchange, FunctionInvoker, IdentityProcessor, NoOpMetrics,
    NoopPlatformService, PlatformService, ProducerContext, RouteController, RuntimeCommand,
    RuntimeHandle,
};
use camel_auth::TokenAuthenticator;
use camel_component_api::{
    ComponentContext, ConcurrencyModel, ConsumerContext, consumer::ExchangeEnvelope,
};
use camel_endpoint::parse_uri;
pub use camel_processor::aggregator::SharedLanguageRegistry;
use camel_processor::aggregator::{AggregatorService, has_timeout_condition};
use camel_processor::circuit_breaker::CircuitBreakerLayer;
use camel_processor::error_handler::ErrorHandlerLayer;
use camel_processor::security_policy_layer::SecurityPolicyLayer;

use crate::health_registry::HealthCheckRegistry;
use crate::lifecycle::adapters::exchange_uow::ExchangeUoWLayer;
use crate::lifecycle::adapters::route_compiler::{
    compose_pipeline, compose_traced_pipeline_with_contracts,
};
use crate::lifecycle::application::route_definition::{
    BuilderStep, RouteDefinition, RouteDefinitionInfo,
};
use crate::shared::components::domain::Registry;
use crate::shared::observability::domain::{DetailLevel, TracerConfig};
use arc_swap::ArcSwap;
use camel_bean::BeanRegistry;

/// Notification sent when a route crashes.
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

#[cfg(test)]
type StartRouteEventHook = Arc<dyn Fn(&'static str) + Send + Sync + 'static>;

#[cfg(test)]
static START_ROUTE_EVENT_HOOK: std::sync::LazyLock<std::sync::Mutex<Option<StartRouteEventHook>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

#[cfg(test)]
fn set_start_route_event_hook(hook: Option<StartRouteEventHook>) {
    *START_ROUTE_EVENT_HOOK
        .lock()
        .expect("start route event hook lock") = hook;
}

#[cfg(test)]
fn emit_start_route_event(event: &'static str) {
    if let Some(hook) = START_ROUTE_EVENT_HOOK
        .lock()
        .expect("start route event hook lock")
        .as_ref()
    {
        hook(event);
    }
}

/// Internal state for a managed route.
pub(super) struct AggregateSplitInfo {
    pub(super) pre_pipeline: SharedPipeline,
    pub(super) agg_config: AggregatorConfig,
    pub(super) post_pipeline: SharedPipeline,
}

pub(super) struct ManagedRoute {
    /// The route definition metadata (for introspection).
    pub(super) definition: RouteDefinitionInfo,
    /// Source endpoint URI.
    pub(super) from_uri: String,
    /// Resolved processor pipeline (wrapped for atomic swap).
    pub(super) pipeline: SharedPipeline,
    /// Concurrency model override (if any).
    pub(super) concurrency: Option<ConcurrencyModel>,
    /// Handle for the consumer task (if running).
    pub(super) consumer_handle: Option<JoinHandle<()>>,
    /// Handle for the pipeline task (if running).
    pub(super) pipeline_handle: Option<JoinHandle<()>>,
    /// Cancellation token for stopping the consumer task.
    /// This allows independent control of the consumer lifecycle (for suspend/resume).
    pub(super) consumer_cancel_token: CancellationToken,
    /// Cancellation token for stopping the pipeline task.
    /// This allows independent control of the pipeline lifecycle (for suspend/resume).
    pub(super) pipeline_cancel_token: CancellationToken,
    /// Channel sender for sending exchanges to the pipeline.
    /// Stored to allow resuming a suspended route without recreating the channel.
    pub(super) channel_sender: Option<mpsc::Sender<ExchangeEnvelope>>,
    /// In-flight exchange counter. `None` when UoW is not configured for this route.
    pub(super) in_flight: Option<Arc<std::sync::atomic::AtomicU64>>,
    pub(super) aggregate_split: Option<AggregateSplitInfo>,
    pub(super) agg_service: Option<Arc<std::sync::Mutex<AggregatorService>>>,
    /// Stored security policy config for use when starting/resuming the consumer.
    pub(super) security_policy: Option<camel_api::security_policy::SecurityPolicyConfig>,
    /// Stored security authenticator for use when starting/resuming the consumer.
    pub(super) security_authenticator: Option<Arc<dyn TokenAuthenticator>>,
}

pub(crate) struct PreparedRoute {
    pub(crate) route_id: String,
    pub(super) managed: ManagedRoute,
}

pub(super) fn handle_is_running(handle: &Option<JoinHandle<()>>) -> bool {
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

fn find_top_level_aggregate_requiring_split(
    steps: &[BuilderStep],
) -> Option<(usize, AggregatorConfig)> {
    for (i, step) in steps.iter().enumerate() {
        if let BuilderStep::Aggregate { config } = step {
            if has_timeout_condition(&config.completion) || config.force_completion_on_stop {
                return Some((i, config.clone()));
            }
            break;
        }
    }
    None
}

pub(crate) struct ControllerComponentContext {
    registry: Arc<std::sync::Mutex<Registry>>,
    languages: SharedLanguageRegistry,
    metrics: Arc<dyn MetricsCollector>,
    platform_service: Arc<dyn PlatformService>,
    health_registry: Arc<HealthCheckRegistry>,
    route_id: Option<String>,
}

impl ControllerComponentContext {
    pub(crate) fn new(
        registry: Arc<std::sync::Mutex<Registry>>,
        languages: SharedLanguageRegistry,
        metrics: Arc<dyn MetricsCollector>,
        platform_service: Arc<dyn PlatformService>,
        health_registry: Arc<HealthCheckRegistry>,
        route_id: Option<String>,
    ) -> Self {
        Self {
            registry,
            languages,
            metrics,
            platform_service,
            health_registry,
            route_id,
        }
    }
}

impl ComponentContext for ControllerComponentContext {
    fn resolve_component(&self, scheme: &str) -> Option<Arc<dyn camel_component_api::Component>> {
        self.registry.lock().ok()?.get(scheme)
    }

    fn resolve_language(&self, name: &str) -> Option<Arc<dyn camel_language_api::Language>> {
        self.languages.lock().ok()?.get(name).cloned()
    }

    fn metrics(&self) -> Arc<dyn MetricsCollector> {
        Arc::clone(&self.metrics)
    }

    fn platform_service(&self) -> Arc<dyn PlatformService> {
        Arc::clone(&self.platform_service)
    }

    fn register_route_health_check(
        &self,
        route_id: &str,
        check: Arc<dyn camel_api::AsyncHealthCheck>,
    ) {
        self.health_registry.register_for_route(route_id, check);
    }

    fn unregister_route_health_check(&self, route_id: &str) {
        self.health_registry.unregister_for_route(route_id);
    }

    fn route_id(&self) -> Option<&str> {
        self.route_id.as_deref()
    }
}

fn is_pending(ex: &Exchange) -> bool {
    ex.property("CamelAggregatorPending")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
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
                // log-policy: system-broken
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

pub(super) async fn publish_runtime_failure(
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
    platform_service: Arc<dyn PlatformService>,
    function_invoker: Option<Arc<dyn FunctionInvoker>>,
    health_registry: Option<Arc<HealthCheckRegistry>>,
}

impl DefaultRouteController {
    fn health_registry(&self) -> Arc<HealthCheckRegistry> {
        self.health_registry.clone().unwrap_or_else(|| {
            warn!("health_registry not configured — creating isolated fallback");
            Arc::new(HealthCheckRegistry::new(Duration::from_secs(5)))
        })
    }

    /// Create a new `DefaultRouteController` with the given registry.
    pub fn new(
        registry: Arc<std::sync::Mutex<Registry>>,
        platform_service: Arc<dyn PlatformService>,
    ) -> Self {
        Self::with_beans_and_platform_service(
            registry,
            Arc::new(std::sync::Mutex::new(BeanRegistry::new())),
            platform_service,
        )
    }

    /// Create a new `DefaultRouteController` with shared bean registry.
    pub fn with_beans(
        registry: Arc<std::sync::Mutex<Registry>>,
        beans: Arc<std::sync::Mutex<BeanRegistry>>,
    ) -> Self {
        Self::with_beans_and_platform_service(
            registry,
            beans,
            Arc::new(NoopPlatformService::default()),
        )
    }

    fn with_beans_and_platform_service(
        registry: Arc<std::sync::Mutex<Registry>>,
        beans: Arc<std::sync::Mutex<BeanRegistry>>,
        platform_service: Arc<dyn PlatformService>,
    ) -> Self {
        Self {
            routes: HashMap::new(),
            registry,
            languages: Arc::new(std::sync::Mutex::new(HashMap::new())),
            beans,
            runtime: None,
            global_error_handler: None,
            crash_notifier: None,
            tracing_enabled: false,
            tracer_detail_level: DetailLevel::Minimal,
            tracer_metrics: None,
            platform_service,
            function_invoker: None,
            health_registry: None,
        }
    }

    /// Create a new `DefaultRouteController` with shared language registry.
    pub fn with_languages(
        registry: Arc<std::sync::Mutex<Registry>>,
        languages: SharedLanguageRegistry,
        platform_service: Arc<dyn PlatformService>,
    ) -> Self {
        Self {
            routes: HashMap::new(),
            registry,
            languages,
            beans: Arc::new(std::sync::Mutex::new(BeanRegistry::new())),
            runtime: None,
            global_error_handler: None,
            crash_notifier: None,
            tracing_enabled: false,
            tracer_detail_level: DetailLevel::Minimal,
            tracer_metrics: None,
            platform_service,
            function_invoker: None,
            health_registry: None,
        }
    }

    pub fn with_languages_and_beans(
        registry: Arc<std::sync::Mutex<Registry>>,
        languages: SharedLanguageRegistry,
        platform_service: Arc<dyn PlatformService>,
        beans: Arc<std::sync::Mutex<BeanRegistry>>,
    ) -> Self {
        Self {
            routes: HashMap::new(),
            registry,
            languages,
            beans,
            runtime: None,
            global_error_handler: None,
            crash_notifier: None,
            tracing_enabled: false,
            tracer_detail_level: DetailLevel::Minimal,
            tracer_metrics: None,
            platform_service,
            function_invoker: None,
            health_registry: None,
        }
    }

    pub fn with_function_invoker(mut self, function_invoker: Arc<dyn FunctionInvoker>) -> Self {
        self.function_invoker = Some(function_invoker);
        self
    }

    pub fn set_health_registry(&mut self, registry: Arc<HealthCheckRegistry>) {
        self.health_registry = Some(registry);
    }

    pub fn set_function_invoker(&mut self, invoker: Arc<dyn FunctionInvoker>) {
        self.function_invoker = Some(invoker);
    }

    /// Set runtime handle for ProducerContext creation.
    pub fn set_runtime_handle(&mut self, runtime: Arc<dyn RuntimeHandle>) {
        self.runtime = Some(Arc::downgrade(&runtime));
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

    fn build_producer_context(&self, route_id: &str) -> Result<ProducerContext, CamelError> {
        let mut producer_ctx = ProducerContext::new().with_route_id(route_id);
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
        rt: Arc<dyn camel_component_api::RuntimeObservability>,
        component_ctx: &dyn ComponentContext,
    ) -> Result<ErrorHandlerLayer, CamelError> {
        // Resolve DLC URI → producer.
        let dlc_producer = if let Some(ref uri) = config.dlc_uri {
            let parsed = parse_uri(uri)?;
            let component = component_ctx
                .resolve_component(&parsed.scheme)
                .ok_or_else(|| CamelError::ComponentNotFound(parsed.scheme.clone()))?;
            let endpoint = component.create_endpoint(uri, component_ctx)?;
            Some(endpoint.create_producer(Arc::clone(&rt), producer_ctx)?)
        } else {
            None
        };

        // Resolve per-policy `handled_by` URIs.
        let mut resolved_policies = Vec::new();
        for policy in config.policies {
            let handler_producer = if let Some(ref uri) = policy.handled_by {
                let parsed = parse_uri(uri)?;
                let component = component_ctx
                    .resolve_component(&parsed.scheme)
                    .ok_or_else(|| CamelError::ComponentNotFound(parsed.scheme.clone()))?;
                let endpoint = component.create_endpoint(uri, component_ctx)?;
                Some(endpoint.create_producer(Arc::clone(&rt), producer_ctx)?)
            } else {
                None
            };
            resolved_policies.push((policy, handler_producer));
        }

        Ok(ErrorHandlerLayer::new(dlc_producer, resolved_policies))
    }

    /// Resolve a `UnitOfWorkConfig` into an `(ExchangeUoWLayer, Arc<AtomicU64>)`.
    /// Returns `Err` if any hook URI cannot be resolved.
    fn resolve_uow_layer(
        &self,
        config: &UnitOfWorkConfig,
        producer_ctx: &ProducerContext,
        rt: Arc<dyn camel_component_api::RuntimeObservability>,
        component_ctx: &dyn ComponentContext,
        counter: Option<Arc<AtomicU64>>,
    ) -> Result<(ExchangeUoWLayer, Arc<AtomicU64>), CamelError> {
        let resolve_uri = |uri: &str| -> Result<BoxProcessor, CamelError> {
            let parsed = parse_uri(uri)?;
            let component = component_ctx
                .resolve_component(&parsed.scheme)
                .ok_or_else(|| CamelError::ComponentNotFound(parsed.scheme.clone()))?;
            let endpoint = component.create_endpoint(uri, component_ctx)?;
            endpoint
                .create_producer(Arc::clone(&rt), producer_ctx)
                .map_err(|e| {
                    CamelError::RouteError(format!(
                        "UoW hook URI '{uri}' could not be resolved: {e}"
                    ))
                })
        };

        let on_complete = config.on_complete.as_deref().map(resolve_uri).transpose()?;
        let on_failure = config.on_failure.as_deref().map(resolve_uri).transpose()?;

        let counter = counter.unwrap_or_else(|| Arc::new(AtomicU64::new(0)));
        let layer = ExchangeUoWLayer::new(Arc::clone(&counter), on_complete, on_failure);
        Ok((layer, counter))
    }

    /// Resolve BuilderSteps into BoxProcessors.
    pub(crate) fn resolve_steps(
        &self,
        steps: Vec<BuilderStep>,
        producer_ctx: &ProducerContext,
        registry: &Arc<std::sync::Mutex<Registry>>,
        route_id: Option<&str>,
        staging_mode: &super::step_resolution::FunctionStagingMode,
    ) -> Result<Vec<(BoxProcessor, Option<camel_api::BodyType>)>, CamelError> {
        let component_ctx = Arc::new(ControllerComponentContext::new(
            Arc::clone(registry),
            Arc::clone(&self.languages),
            self.tracer_metrics
                .clone()
                .unwrap_or_else(|| Arc::new(NoOpMetrics)),
            Arc::clone(&self.platform_service),
            self.health_registry(),
            route_id.map(|s| s.to_string()),
        ));
        let rt: Arc<dyn camel_component_api::RuntimeObservability> =
            Arc::clone(&component_ctx) as Arc<_>;

        super::step_resolution::resolve_steps(
            steps,
            producer_ctx,
            rt,
            registry,
            &self.languages,
            &self.beans,
            self.function_invoker.clone(),
            component_ctx,
            route_id,
            staging_mode,
        )
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
    pub async fn add_route(&mut self, definition: RouteDefinition) -> Result<(), CamelError> {
        let route_id = definition.route_id().to_string();

        if self.routes.contains_key(&route_id) {
            return Err(CamelError::RouteError(format!(
                "Route '{}' already exists",
                route_id
            )));
        }

        info!(route_id = %route_id, "Adding route to controller");

        let prepared = match self.build_managed_route(
            definition,
            &super::step_resolution::FunctionStagingMode::DirectAdd,
        ) {
            Ok(prepared) => prepared,
            Err(err) => {
                self.discard_function_staging();
                return Err(err);
            }
        };

        if let Some(invoker) = &self.function_invoker
            && let Err(err) = invoker.commit_staged().await
        {
            invoker.discard_staging(0);
            return Err(CamelError::Config(err.to_string()));
        }

        self.routes
            .insert(prepared.route_id.clone(), prepared.managed);

        Ok(())
    }

    fn build_managed_route(
        &self,
        definition: RouteDefinition,
        staging_mode: &super::step_resolution::FunctionStagingMode,
    ) -> Result<PreparedRoute, CamelError> {
        let route_id = definition.route_id().to_string();

        let definition_info = definition.to_info();
        let RouteDefinition {
            from_uri,
            steps,
            error_handler,
            circuit_breaker,
            security_policy,
            security_authenticator,
            unit_of_work,
            concurrency,
            ..
        } = definition;

        let producer_ctx = self.build_producer_context(&route_id)?;

        let mut aggregate_split: Option<AggregateSplitInfo> = None;
        let processors_with_contracts = match find_top_level_aggregate_requiring_split(&steps) {
            Some((idx, agg_config)) => {
                let mut pre_steps = steps;
                let mut rest = pre_steps.split_off(idx);
                let _agg_step = rest.remove(0);
                let post_steps = rest;

                let pre_pairs = self.resolve_steps(
                    pre_steps,
                    &producer_ctx,
                    &self.registry,
                    Some(&route_id),
                    staging_mode,
                )?;
                let pre_procs: Vec<BoxProcessor> = pre_pairs.into_iter().map(|(p, _)| p).collect();
                let pre_pipeline = Arc::new(ArcSwap::from_pointee(SyncBoxProcessor(
                    compose_pipeline(pre_procs),
                )));

                let post_pairs = self.resolve_steps(
                    post_steps,
                    &producer_ctx,
                    &self.registry,
                    Some(&route_id),
                    staging_mode,
                )?;
                let post_procs: Vec<BoxProcessor> =
                    post_pairs.into_iter().map(|(p, _)| p).collect();
                let post_pipeline = Arc::new(ArcSwap::from_pointee(SyncBoxProcessor(
                    compose_pipeline(post_procs),
                )));

                aggregate_split = Some(AggregateSplitInfo {
                    pre_pipeline,
                    agg_config,
                    post_pipeline,
                });

                vec![]
            }
            None => self.resolve_steps(
                steps,
                &producer_ctx,
                &self.registry,
                Some(&route_id),
                staging_mode,
            )?,
        };
        let route_id_for_tracing = route_id.clone();
        let mut pipeline = if processors_with_contracts.is_empty() {
            BoxProcessor::new(IdentityProcessor)
        } else {
            compose_traced_pipeline_with_contracts(
                processors_with_contracts,
                &route_id_for_tracing,
                self.tracing_enabled,
                self.tracer_detail_level.clone(),
                self.tracer_metrics.clone(),
            )
        };

        if let Some(cb_config) = circuit_breaker {
            let cb_layer = CircuitBreakerLayer::new(cb_config);
            pipeline = BoxProcessor::new(cb_layer.layer(pipeline));
        }

        if let Some(sp_config) = security_policy.clone() {
            let sp_layer = SecurityPolicyLayer::new(sp_config.policy);
            pipeline = BoxProcessor::new(sp_layer.layer(pipeline));
        }

        let eh_config = error_handler.or_else(|| self.global_error_handler.clone());

        if let Some(config) = eh_config {
            let component_ctx = Arc::new(ControllerComponentContext::new(
                Arc::clone(&self.registry),
                Arc::clone(&self.languages),
                self.tracer_metrics
                    .clone()
                    .unwrap_or_else(|| Arc::new(NoOpMetrics)),
                Arc::clone(&self.platform_service),
                self.health_registry(),
                Some(route_id.clone()),
            ));
            let rt: Arc<dyn camel_component_api::RuntimeObservability> =
                Arc::clone(&component_ctx) as Arc<_>;
            let layer =
                self.resolve_error_handler(config, &producer_ctx, rt, component_ctx.as_ref())?;
            pipeline = BoxProcessor::new(layer.layer(pipeline));
        }

        let uow_counter = if let Some(uow_config) = &unit_of_work {
            let component_ctx = Arc::new(ControllerComponentContext::new(
                Arc::clone(&self.registry),
                Arc::clone(&self.languages),
                self.tracer_metrics
                    .clone()
                    .unwrap_or_else(|| Arc::new(NoOpMetrics)),
                Arc::clone(&self.platform_service),
                self.health_registry(),
                Some(route_id.clone()),
            ));
            let rt: Arc<dyn camel_component_api::RuntimeObservability> =
                Arc::clone(&component_ctx) as Arc<_>;
            let (uow_layer, counter) = self.resolve_uow_layer(
                uow_config,
                &producer_ctx,
                rt,
                component_ctx.as_ref(),
                None,
            )?;
            pipeline = BoxProcessor::new(uow_layer.layer(pipeline));
            Some(counter)
        } else {
            None
        };

        Ok(PreparedRoute {
            route_id,
            managed: ManagedRoute {
                definition: definition_info,
                from_uri,
                pipeline: Arc::new(ArcSwap::from_pointee(SyncBoxProcessor(pipeline))),
                concurrency,
                consumer_handle: None,
                pipeline_handle: None,
                consumer_cancel_token: CancellationToken::new(),
                pipeline_cancel_token: CancellationToken::new(),
                channel_sender: None,
                in_flight: uow_counter,
                aggregate_split,
                agg_service: None,
                security_policy,
                security_authenticator,
            },
        })
    }

    pub(crate) fn insert_prepared_route(
        &mut self,
        prepared: PreparedRoute,
    ) -> Result<(), CamelError> {
        if self.routes.contains_key(&prepared.route_id) {
            return Err(CamelError::RouteError(format!(
                "Route '{}' already exists",
                prepared.route_id
            )));
        }
        self.routes
            .insert(prepared.route_id.clone(), prepared.managed);
        Ok(())
    }

    pub async fn add_route_with_generation(
        &mut self,
        definition: RouteDefinition,
        generation: u64,
    ) -> Result<(), CamelError> {
        let route_id = definition.route_id().to_string();

        if self.routes.contains_key(&route_id) {
            return Err(CamelError::RouteError(format!(
                "Route '{}' already exists",
                route_id
            )));
        }

        info!(route_id = %route_id, generation, "Adding route to controller with generation");

        let prepared = self.build_managed_route(
            definition,
            &super::step_resolution::FunctionStagingMode::HotReload { generation },
        )?;

        self.routes
            .insert(prepared.route_id.clone(), prepared.managed);

        Ok(())
    }

    pub(crate) fn prepare_route_definition_with_generation(
        &self,
        definition: RouteDefinition,
        generation: u64,
    ) -> Result<PreparedRoute, CamelError> {
        self.build_managed_route(
            definition,
            &super::step_resolution::FunctionStagingMode::HotReload { generation },
        )
    }

    pub async fn remove_route_preserving_functions(
        &mut self,
        route_id: &str,
    ) -> Result<(), CamelError> {
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
        if let Some(reg) = &self.health_registry {
            reg.unregister_for_route(route_id);
        }
        info!(route_id = %route_id, "Route removed from controller (functions preserved for reload finalize)");
        Ok(())
    }

    pub fn compile_route_definition(
        &self,
        def: RouteDefinition,
    ) -> Result<BoxProcessor, CamelError> {
        let route_id = def.route_id().to_string();

        let producer_ctx = self.build_producer_context(&route_id)?;

        let processors_with_contracts = self.resolve_steps(
            def.steps,
            &producer_ctx,
            &self.registry,
            Some(&route_id),
            &super::step_resolution::FunctionStagingMode::DryCompile,
        )?;
        let mut pipeline = compose_traced_pipeline_with_contracts(
            processors_with_contracts,
            &route_id,
            self.tracing_enabled,
            self.tracer_detail_level.clone(),
            self.tracer_metrics.clone(),
        );

        if let Some(cb_config) = def.circuit_breaker {
            let cb_layer = CircuitBreakerLayer::new(cb_config);
            pipeline = BoxProcessor::new(cb_layer.layer(pipeline));
        }

        if let Some(sp_config) = def.security_policy {
            let sp_layer = SecurityPolicyLayer::new(sp_config.policy);
            pipeline = BoxProcessor::new(sp_layer.layer(pipeline));
        }

        let eh_config = def
            .error_handler
            .clone()
            .or_else(|| self.global_error_handler.clone());
        if let Some(config) = eh_config {
            let component_ctx = Arc::new(ControllerComponentContext::new(
                Arc::clone(&self.registry),
                Arc::clone(&self.languages),
                self.tracer_metrics
                    .clone()
                    .unwrap_or_else(|| Arc::new(NoOpMetrics)),
                Arc::clone(&self.platform_service),
                self.health_registry(),
                Some(route_id.clone()),
            ));
            let rt: Arc<dyn camel_component_api::RuntimeObservability> =
                Arc::clone(&component_ctx) as Arc<_>;
            let layer =
                self.resolve_error_handler(config, &producer_ctx, rt, component_ctx.as_ref())?;
            pipeline = BoxProcessor::new(layer.layer(pipeline));
        }

        // Apply UoW layer outermost
        if let Some(uow_config) = &def.unit_of_work {
            let existing_counter = self
                .routes
                .get(&route_id)
                .and_then(|r| r.in_flight.as_ref().map(Arc::clone));

            let component_ctx = Arc::new(ControllerComponentContext::new(
                Arc::clone(&self.registry),
                Arc::clone(&self.languages),
                self.tracer_metrics
                    .clone()
                    .unwrap_or_else(|| Arc::new(NoOpMetrics)),
                Arc::clone(&self.platform_service),
                self.health_registry(),
                Some(route_id.clone()),
            ));
            let rt: Arc<dyn camel_component_api::RuntimeObservability> =
                Arc::clone(&component_ctx) as Arc<_>;

            let (uow_layer, _counter) = self.resolve_uow_layer(
                uow_config,
                &producer_ctx,
                rt,
                component_ctx.as_ref(),
                existing_counter,
            )?;

            pipeline = BoxProcessor::new(uow_layer.layer(pipeline));
        }

        Ok(pipeline)
    }

    pub fn compile_route_definition_with_generation(
        &self,
        def: RouteDefinition,
        generation: u64,
    ) -> Result<BoxProcessor, CamelError> {
        let route_id = def.route_id().to_string();

        let producer_ctx = self.build_producer_context(&route_id)?;

        let processors_with_contracts = self.resolve_steps(
            def.steps,
            &producer_ctx,
            &self.registry,
            Some(&route_id),
            &super::step_resolution::FunctionStagingMode::HotReload { generation },
        )?;
        let mut pipeline = compose_traced_pipeline_with_contracts(
            processors_with_contracts,
            &route_id,
            self.tracing_enabled,
            self.tracer_detail_level.clone(),
            self.tracer_metrics.clone(),
        );

        if let Some(cb_config) = def.circuit_breaker {
            let cb_layer = CircuitBreakerLayer::new(cb_config);
            pipeline = BoxProcessor::new(cb_layer.layer(pipeline));
        }

        if let Some(sp_config) = def.security_policy {
            let sp_layer = SecurityPolicyLayer::new(sp_config.policy);
            pipeline = BoxProcessor::new(sp_layer.layer(pipeline));
        }

        let eh_config = def
            .error_handler
            .clone()
            .or_else(|| self.global_error_handler.clone());
        if let Some(config) = eh_config {
            let component_ctx = Arc::new(ControllerComponentContext::new(
                Arc::clone(&self.registry),
                Arc::clone(&self.languages),
                self.tracer_metrics
                    .clone()
                    .unwrap_or_else(|| Arc::new(NoOpMetrics)),
                Arc::clone(&self.platform_service),
                self.health_registry(),
                Some(route_id.clone()),
            ));
            let rt: Arc<dyn camel_component_api::RuntimeObservability> =
                Arc::clone(&component_ctx) as Arc<_>;
            let layer =
                self.resolve_error_handler(config, &producer_ctx, rt, component_ctx.as_ref())?;
            pipeline = BoxProcessor::new(layer.layer(pipeline));
        }

        if let Some(uow_config) = &def.unit_of_work {
            let existing_counter = self
                .routes
                .get(&route_id)
                .and_then(|r| r.in_flight.as_ref().map(Arc::clone));

            let component_ctx = Arc::new(ControllerComponentContext::new(
                Arc::clone(&self.registry),
                Arc::clone(&self.languages),
                self.tracer_metrics
                    .clone()
                    .unwrap_or_else(|| Arc::new(NoOpMetrics)),
                Arc::clone(&self.platform_service),
                self.health_registry(),
                Some(route_id.clone()),
            ));
            let rt: Arc<dyn camel_component_api::RuntimeObservability> =
                Arc::clone(&component_ctx) as Arc<_>;

            let (uow_layer, _counter) = self.resolve_uow_layer(
                uow_config,
                &producer_ctx,
                rt,
                component_ctx.as_ref(),
                existing_counter,
            )?;

            pipeline = BoxProcessor::new(uow_layer.layer(pipeline));
        }

        Ok(pipeline)
    }

    /// Remove a route from the controller map.
    ///
    /// The route **must** be stopped before removal (status `Stopped` or `Failed`).
    /// Returns an error if the route is still running or does not exist.
    /// Does not cancel any running tasks — call `stop_route` first.
    pub async fn remove_route(&mut self, route_id: &str) -> Result<(), CamelError> {
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
        if let Some(invoker) = &self.function_invoker {
            for (id, rid) in self.collect_function_refs(route_id) {
                if let Err(e) = invoker.unregister(&id, rid.as_deref()).await {
                    warn!(route_id = %route_id, error = %e, "Failed to unregister function during route removal");
                }
            }
        }
        self.routes.remove(route_id);
        if let Some(reg) = &self.health_registry {
            reg.unregister_for_route(route_id);
        }
        info!(route_id = %route_id, "Route removed from controller");
        Ok(())
    }

    fn collect_function_refs(
        &self,
        route_id: &str,
    ) -> Vec<(camel_api::FunctionId, Option<String>)> {
        self.function_invoker
            .as_ref()
            .map(|invoker| invoker.function_refs_for_route(route_id))
            .unwrap_or_default()
    }

    fn discard_function_staging(&self) {
        if let Some(invoker) = &self.function_invoker {
            invoker.discard_staging(0);
        }
    }

    /// Returns the number of routes in the controller.
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    pub fn in_flight_count(&self, route_id: &str) -> Option<u64> {
        self.routes.get(route_id).map(|r| {
            r.in_flight
                .as_ref()
                .map_or(0, |c| c.load(Ordering::Relaxed))
        })
    }

    /// Returns `true` if a route with the given ID exists.
    pub fn route_exists(&self, route_id: &str) -> bool {
        self.routes.contains_key(route_id)
    }

    /// Returns all route IDs.
    pub fn route_ids(&self) -> Vec<String> {
        self.routes.keys().cloned().collect()
    }

    pub fn route_source_hash(&self, route_id: &str) -> Option<u64> {
        self.routes
            .get(route_id)
            .and_then(|m| m.definition.source_hash())
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

        if managed.aggregate_split.is_some() {
            tracing::warn!(
                route_id = %route_id,
                "swap_pipeline: aggregate routes with timeout do not support hot-reload of pre/post segments"
            );
        }

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
        super::consumer_management::stop_route_internal(
            &mut self.routes,
            route_id,
            DEFAULT_SHUTDOWN_TIMEOUT,
        )
        .await
    }

    pub async fn start_route_reload(&mut self, route_id: &str) -> Result<(), CamelError> {
        self.start_route(route_id).await
    }

    pub async fn stop_route_reload(&mut self, route_id: &str) -> Result<(), CamelError> {
        self.stop_route(route_id).await
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
                .expect("invariant: route must exist after prior existence check"); // allow-unwrap
            (
                managed.from_uri.clone(),
                Arc::clone(&managed.pipeline),
                managed.concurrency.clone(),
            )
        };

        // Clone crash notifier for consumer task
        let crash_notifier = self.crash_notifier.clone();
        let runtime_for_consumer = self.runtime.clone();

        let consumer_component_ctx = Arc::new(ControllerComponentContext::new(
            Arc::clone(&self.registry),
            Arc::clone(&self.languages),
            self.tracer_metrics
                .clone()
                .unwrap_or_else(|| Arc::new(NoOpMetrics)),
            Arc::clone(&self.platform_service),
            self.health_registry(),
            Some(route_id.to_string()),
        ));
        let consumer_rt: Arc<dyn camel_component_api::RuntimeObservability> =
            Arc::clone(&consumer_component_ctx) as Arc<_>;
        let (mut consumer, consumer_concurrency) =
            super::consumer_management::create_route_consumer(
                consumer_rt,
                &self.registry,
                &from_uri,
                consumer_component_ctx.as_ref(),
            )?;

        // Resolve effective concurrency: route override > consumer default
        let effective_concurrency = concurrency.unwrap_or(consumer_concurrency);

        // Get the managed route for mutation
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check"); // allow-unwrap

        // Wire security context before spawning consumer
        if let (Some(sp_config), Some(authenticator)) = (
            managed.security_policy.as_ref(),
            managed.security_authenticator.as_ref(),
        ) {
            use camel_component_api::SecurityContext;
            let sec_ctx =
                SecurityContext::from_arc(Arc::clone(&sp_config.policy), Arc::clone(authenticator));
            consumer.set_security_context(sec_ctx);
        }

        // Create channel for consumer to send exchanges
        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(256);
        // Create child tokens for independent lifecycle control
        let consumer_cancel = managed.consumer_cancel_token.child_token();
        let pipeline_cancel = managed.pipeline_cancel_token.child_token();
        // Clone sender for storage (to reuse on resume)
        let tx_for_storage = tx.clone();
        let consumer_ctx = ConsumerContext::new(tx, consumer_cancel.clone(), route_id.to_string());

        // --- Aggregator v2: check for aggregate route with timeout ---
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check"); // allow-unwrap

        if let Some(split) = managed.aggregate_split.as_ref() {
            let (late_tx, late_rx) = mpsc::channel::<Exchange>(256);

            let route_cancel_clone = pipeline_cancel.clone();
            let svc = AggregatorService::new(
                split.agg_config.clone(),
                late_tx,
                Arc::clone(&self.languages),
                route_cancel_clone,
            );
            let agg = Arc::new(std::sync::Mutex::new(svc));

            let pipeline_cancel_for_monitor = pipeline_cancel.clone();
            let agg_for_monitor = Arc::clone(&agg);

            managed.agg_service = Some(Arc::clone(&agg));

            let late_rx = Arc::new(tokio::sync::Mutex::new(late_rx));
            let pre_pipeline = Arc::clone(&split.pre_pipeline);
            let post_pipeline = Arc::clone(&split.post_pipeline);

            // Spawn biased select forward loop
            let pipeline_handle = tokio::spawn(async move {
                loop {
                    tokio::select! {
                        biased;

                        late_ex = async {
                            let mut rx = late_rx.lock().await;
                            rx.recv().await
                        } => {
                            match late_ex {
                                Some(ex) => {
                                    let pipe = post_pipeline.load();
                                    if let Err(e) = pipe.0.clone().oneshot(ex).await {
                                        tracing::warn!(error = %e, "late exchange post-pipeline failed");
                                    }
                                }
                                None => return,
                            }
                        }

                        envelope_opt = rx.recv() => {
                            match envelope_opt {
                                Some(envelope) => {
                                    let ExchangeEnvelope { exchange, reply_tx } = envelope;
                                    let pre_pipe = pre_pipeline.load();
                                    let ex = match pre_pipe.0.clone().oneshot(exchange).await {
                                        Ok(ex) => ex,
                                        Err(e) => {
                                            if let Some(tx) = reply_tx { let _ = tx.send(Err(e)); }
                                            continue;
                                        }
                                    };

                                    let ex = {
                                        let cloned_svc = agg
                                            .lock()
                                            .expect("mutex poisoned: another thread panicked while holding this lock") // allow-unwrap
                                            .clone();
                                        cloned_svc.oneshot(ex).await
                                    };

                                    match ex {
                                        Ok(ex) => {
                                            if !is_pending(&ex) {
                                                let post_pipe = post_pipeline.load();
                                                let out = post_pipe.0.clone().oneshot(ex).await;
                                                if let Some(tx) = reply_tx { let _ = tx.send(out); }
                                            } else if let Some(tx) = reply_tx {
                                                let _ = tx.send(Ok(ex));
                                            }
                                        }
                                        Err(e) => {
                                            if let Some(tx) = reply_tx { let _ = tx.send(Err(e)); }
                                        }
                                    }
                                }
                                None => return,
                            }
                        }

                        _ = pipeline_cancel.cancelled() => {
                            {
                                let guard = agg
                                    .lock()
                                    .expect("mutex poisoned: another thread panicked while holding this lock"); // allow-unwrap
                                guard.force_complete_all();
                            }
                            let mut rx_guard = late_rx.lock().await;
                            while let Ok(late_ex) = rx_guard.try_recv() {
                                let pipe = post_pipeline.load();
                                let _ = pipe.0.clone().oneshot(late_ex).await;
                            }
                            break;
                        }
                    }
                }
            });
            #[cfg(test)]
            emit_start_route_event("pipeline_spawned");

            // Start consumer after pipeline loop is spawned to avoid startup races
            // where consumers emit exchanges before the route pipeline begins polling.
            let consumer_handle = super::consumer_management::spawn_consumer_task(
                route_id.to_string(),
                consumer,
                consumer_ctx,
                crash_notifier,
                runtime_for_consumer,
                false,
            );

            // Extend the stored consumer handle through aggregate force-completion.
            // While this monitor drains pending buckets, handle_is_running still reports
            // the Route as running because forced exchanges may still be in post-pipeline.
            let force_on_stop = agg_for_monitor
                .lock()
                .expect("mutex poisoned: another thread panicked while holding this lock") // allow-unwrap
                .config()
                .force_completion_on_stop;
            let consumer_handle = tokio::spawn(async move {
                let _ = consumer_handle.await;
                if !pipeline_cancel_for_monitor.is_cancelled() {
                    let guard = agg_for_monitor
                        .lock()
                        .expect("mutex poisoned: another thread panicked while holding this lock"); // allow-unwrap
                    guard.force_complete_all();
                    drop(guard);
                    if force_on_stop {
                        pipeline_cancel_for_monitor.cancel();
                    }
                }
            });
            #[cfg(test)]
            emit_start_route_event("consumer_spawned");

            let managed = self
                .routes
                .get_mut(route_id)
                .expect("invariant: route must exist"); // allow-unwrap
            managed.consumer_handle = Some(consumer_handle);
            managed.pipeline_handle = Some(pipeline_handle);
            managed.channel_sender = Some(tx_for_storage);

            info!(route_id = %route_id, "Route started (aggregate with timeout)");
            return Ok(());
        }
        // --- End aggregator v2 branch ---

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
                            // log-policy: system-broken
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
                                Some(s) => Some(s.acquire().await.expect("semaphore closed")), // allow-unwrap
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
                                // log-policy: system-broken
                                error!("Pipeline error: {e}");
                            }
                        });
                    }
                })
            }
        };
        #[cfg(test)]
        emit_start_route_event("pipeline_spawned");

        // Start consumer after pipeline task is spawned to minimize the chance of
        // fire-and-forget events being produced before the pipeline loop is active.
        let consumer_handle = super::consumer_management::spawn_consumer_task(
            route_id.to_string(),
            consumer,
            consumer_ctx,
            crash_notifier,
            runtime_for_consumer,
            false,
        );
        #[cfg(test)]
        emit_start_route_event("consumer_spawned");

        // Store handles and update status
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check"); // allow-unwrap
        managed.consumer_handle = Some(consumer_handle);
        managed.pipeline_handle = Some(pipeline_handle);
        managed.channel_sender = Some(tx_for_storage);

        info!(route_id = %route_id, "Route started");
        Ok(())
    }

    async fn stop_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        self.stop_route_internal(route_id).await?;
        if let Some(reg) = &self.health_registry {
            reg.unregister_for_route(route_id);
        }
        Ok(())
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
            .expect("invariant: route must exist after prior existence check"); // allow-unwrap
        managed.consumer_cancel_token.cancel();

        // Take and join consumer handle
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check"); // allow-unwrap
        let consumer_handle = managed.consumer_handle.take();

        // Wait for consumer task to complete with timeout
        let timeout_result = tokio::time::timeout(DEFAULT_SHUTDOWN_TIMEOUT, async {
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
            .expect("invariant: route must exist after prior existence check"); // allow-unwrap

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

        let consumer_component_ctx = Arc::new(ControllerComponentContext::new(
            Arc::clone(&self.registry),
            Arc::clone(&self.languages),
            self.tracer_metrics
                .clone()
                .unwrap_or_else(|| Arc::new(NoOpMetrics)),
            Arc::clone(&self.platform_service),
            self.health_registry(),
            Some(route_id.to_string()),
        ));
        let consumer_rt: Arc<dyn camel_component_api::RuntimeObservability> =
            Arc::clone(&consumer_component_ctx) as Arc<_>;
        let (mut consumer, _) = super::consumer_management::create_route_consumer(
            consumer_rt,
            &self.registry,
            &from_uri,
            consumer_component_ctx.as_ref(),
        )?;

        // Wire security context before spawning consumer
        let managed = self
            .routes
            .get(route_id)
            .expect("invariant: route must exist after prior existence check"); // allow-unwrap
        if let (Some(sp_config), Some(authenticator)) = (
            managed.security_policy.as_ref(),
            managed.security_authenticator.as_ref(),
        ) {
            use camel_component_api::SecurityContext;
            let sec_ctx =
                SecurityContext::from_arc(Arc::clone(&sp_config.policy), Arc::clone(authenticator));
            consumer.set_security_context(sec_ctx);
        }

        // Get the managed route for mutation
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check"); // allow-unwrap

        // Create child token for consumer lifecycle
        let consumer_cancel = managed.consumer_cancel_token.child_token();

        let crash_notifier = self.crash_notifier.clone();
        let runtime_for_consumer = self.runtime.clone();

        // Create ConsumerContext with the stored sender
        let consumer_ctx =
            ConsumerContext::new(sender, consumer_cancel.clone(), route_id.to_string());

        // Spawn consumer task
        let consumer_handle = super::consumer_management::spawn_consumer_task(
            route_id.to_string(),
            consumer,
            consumer_ctx,
            crash_notifier,
            runtime_for_consumer,
            true,
        );

        // Store consumer handle and update status
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check"); // allow-unwrap
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

#[cfg(test)]
#[path = "route_controller_tests.rs"]
mod tests;
