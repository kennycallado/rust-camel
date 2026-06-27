//! Default implementation of RouteController.
//!
//! This module provides [`DefaultRouteController`], which manages route lifecycle
//! including starting, stopping, suspending, and resuming routes.

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tower::{Layer, ServiceExt};
use tracing::{info, warn};

use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::metrics::MetricsCollector;
#[allow(unused_imports)]
use camel_api::{
    BoxProcessor, CamelError, Exchange, FunctionInvoker, IdentityProcessor, NoOpMetrics,
    NoopPlatformService, PlatformService, ProducerContext, RouteController, RuntimeHandle,
    StepLifecycle,
};
use camel_component_api::{Consumer, ConsumerContext, consumer::ExchangeEnvelope};
use camel_processor::aggregator::AggregatorService;
pub use camel_processor::aggregator::SharedLanguageRegistry;

use crate::health_registry::HealthCheckRegistry;
use crate::lifecycle::adapters::controller_component_context::ControllerComponentContext;
use crate::lifecycle::adapters::route_compiler::compose_pipeline;
use crate::lifecycle::adapters::route_compiler_ext::{RouteCompilerExt, build_eh_config_pipeline};
pub(crate) use crate::lifecycle::adapters::route_helpers::PreparedRoute;
use crate::lifecycle::adapters::route_helpers::{
    AggregateSplitInfo, CrashNotification, ManagedRoute, find_top_level_aggregate_requiring_split,
    handle_is_running, inferred_lifecycle_label, is_pending,
};
#[cfg(test)]
pub(super) use crate::lifecycle::adapters::route_helpers::{
    emit_start_route_event, set_start_route_event_hook,
};
use crate::lifecycle::adapters::route_registry::RouteRegistry;
use crate::lifecycle::adapters::route_runtime_state;
use crate::lifecycle::adapters::step_compilers::CompiledStep;
use crate::lifecycle::application::route_definition::{BuilderStep, RouteDefinition};
use crate::shared::components::domain::Registry;
use crate::shared::observability::domain::{DetailLevel, TracerConfig};
use camel_bean::BeanRegistry;

/// Default implementation of [`RouteController`].
///
/// Manages route lifecycle with support for:
/// - Starting/stopping individual routes
/// - Suspending and resuming routes
/// - Auto-startup with startup ordering
/// - Graceful shutdown
pub struct DefaultRouteController {
    /// Routes indexed by route ID.
    pub(super) routes: RouteRegistry,
    /// Reference to the component registry for resolving endpoints.
    pub(super) registry: Arc<std::sync::Mutex<Registry>>,
    /// Shared language registry for resolving declarative language expressions.
    pub(super) languages: SharedLanguageRegistry,
    /// Bean registry for bean method invocation.
    pub(super) beans: Arc<std::sync::Mutex<BeanRegistry>>,
    /// Runtime handle injected into ProducerContext for command/query operations.
    pub(super) runtime: Option<Weak<dyn RuntimeHandle>>,
    /// Optional global error handler applied to all routes without a per-route handler.
    pub(super) global_error_handler: Option<ErrorHandlerConfig>,
    /// Optional crash notifier for supervision.
    pub(super) crash_notifier: Option<mpsc::Sender<CrashNotification>>,
    /// Whether tracing is enabled for route pipelines.
    pub(super) tracing_enabled: bool,
    /// Detail level for tracing when enabled.
    pub(super) tracer_detail_level: DetailLevel,
    /// Metrics collector for tracing processor.
    pub(super) tracer_metrics: Option<Arc<dyn MetricsCollector>>,
    pub(super) platform_service: Arc<dyn PlatformService>,
    pub(super) function_invoker: Option<Arc<dyn FunctionInvoker>>,
    pub(super) health_registry: Option<Arc<HealthCheckRegistry>>,
    /// Shared idempotent repository registry. Defaults to an empty registry;
    /// the CamelContext builder installs a populated handle that includes the
    /// built-in `"memory"` repository.
    pub(super) idempotent_repositories: crate::SharedIdempotentRegistry,
    pub(super) claim_check_repositories: crate::SharedClaimCheckRegistry,
}

impl DefaultRouteController {
    pub(super) fn health_registry(&self) -> Arc<HealthCheckRegistry> {
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
            routes: RouteRegistry::new(),
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
            idempotent_repositories: Arc::new(crate::IdempotentRegistry::new()),
            claim_check_repositories: Arc::new(crate::ClaimCheckRegistry::new()),
        }
    }

    /// Create a new `DefaultRouteController` with shared language registry.
    pub fn with_languages(
        registry: Arc<std::sync::Mutex<Registry>>,
        languages: SharedLanguageRegistry,
        platform_service: Arc<dyn PlatformService>,
    ) -> Self {
        Self {
            routes: RouteRegistry::new(),
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
            idempotent_repositories: Arc::new(crate::IdempotentRegistry::new()),
            claim_check_repositories: Arc::new(crate::ClaimCheckRegistry::new()),
        }
    }

    pub fn with_languages_and_beans(
        registry: Arc<std::sync::Mutex<Registry>>,
        languages: SharedLanguageRegistry,
        platform_service: Arc<dyn PlatformService>,
        beans: Arc<std::sync::Mutex<BeanRegistry>>,
    ) -> Self {
        Self {
            routes: RouteRegistry::new(),
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
            idempotent_repositories: Arc::new(crate::IdempotentRegistry::new()),
            claim_check_repositories: Arc::new(crate::ClaimCheckRegistry::new()),
        }
    }

    pub fn with_function_invoker(mut self, function_invoker: Arc<dyn FunctionInvoker>) -> Self {
        self.function_invoker = Some(function_invoker);
        self
    }

    pub(crate) fn set_idempotent_repositories(
        &mut self,
        repositories: crate::SharedIdempotentRegistry,
    ) {
        self.idempotent_repositories = repositories;
    }

    pub(crate) fn set_claim_check_repositories(
        &mut self,
        repositories: crate::SharedClaimCheckRegistry,
    ) {
        self.claim_check_repositories = repositories;
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

    /// Create a transient [`RouteCompilerExt`] from this controller's fields.
    fn route_compiler_ext(&self) -> RouteCompilerExt<'_> {
        RouteCompilerExt {
            registry: &self.registry,
            languages: &self.languages,
            beans: &self.beans,
            function_invoker: &self.function_invoker,
            tracing_enabled: self.tracing_enabled,
            tracer_detail_level: &self.tracer_detail_level,
            tracer_metrics: &self.tracer_metrics,
            platform_service: &self.platform_service,
            runtime: &self.runtime,
            global_error_handler: &self.global_error_handler,
            health_registry: &self.health_registry,
            route_registry: &self.routes,
            idempotent_repositories: Arc::clone(&self.idempotent_repositories),
            claim_check_repositories: Arc::clone(&self.claim_check_repositories),
        }
    }

    /// Resolve BuilderSteps into BoxProcessors.
    pub(crate) fn resolve_steps(
        &self,
        steps: Vec<BuilderStep>,
        producer_ctx: &ProducerContext,
        registry: &Arc<std::sync::Mutex<Registry>>,
        route_id: Option<&str>,
        staging_mode: &super::step_resolution::FunctionStagingMode,
    ) -> Result<Vec<CompiledStep>, CamelError> {
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
            &self.idempotent_repositories,
            &self.claim_check_repositories,
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
                let pre_pipeline =
                    super::pipeline_runtime::new_shared_pipeline(compose_pipeline(pre_pairs));

                let post_pairs = self.resolve_steps(
                    post_steps,
                    &producer_ctx,
                    &self.registry,
                    Some(&route_id),
                    staging_mode,
                )?;
                let post_pipeline =
                    super::pipeline_runtime::new_shared_pipeline(compose_pipeline(post_pairs));

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
        let lifecycle = super::route_helpers::collect_lifecycle(&processors_with_contracts);
        let route_id_for_tracing = route_id.clone();
        let eh_config = error_handler.or_else(|| self.global_error_handler.clone());

        let mut pipeline = build_eh_config_pipeline(
            eh_config.as_ref(),
            Arc::clone(&self.registry),
            Arc::clone(&self.languages),
            self.tracer_metrics.clone(),
            Arc::clone(&self.platform_service),
            self.health_registry(),
            &route_id_for_tracing,
            &producer_ctx,
            processors_with_contracts,
            self.tracing_enabled,
            self.tracer_detail_level.clone(),
            security_policy.clone(),
            circuit_breaker,
        )?;

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
            let (uow_layer, counter) = super::route_compiler_ext::resolve_uow_layer(
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
                pipeline: super::pipeline_runtime::new_shared_pipeline_with_lifecycle(
                    pipeline, lifecycle,
                ),
                concurrency,
                consumer_handle: None,
                pipeline_handle: None,
                consumer_cancel_token: CancellationToken::new(),
                pipeline_cancel_token: CancellationToken::new(),
                channel_sender: None,
                in_flight: uow_counter,
                aggregate_split,
                agg_service: None,
                compiled: route_runtime_state::CompiledRoute {
                    security_policy,
                    security_authenticator,
                },
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

    /// Compile a route definition into a processor pipeline, without adding it
    /// to the controller. Used for validation and testing.
    pub fn compile_route_definition(
        &self,
        def: RouteDefinition,
    ) -> Result<BoxProcessor, CamelError> {
        self.route_compiler_ext().compile_route_definition(def)
    }

    /// Compile a route definition with a specific generation (for hot-reload).
    pub fn compile_route_definition_with_generation(
        &self,
        def: RouteDefinition,
        generation: u64,
    ) -> Result<BoxProcessor, CamelError> {
        self.route_compiler_ext()
            .compile_route_definition_with_generation(def, generation)
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
        self.routes.route_count()
    }

    pub fn in_flight_count(&self, route_id: &str) -> Option<u64> {
        self.routes.in_flight_count(route_id)
    }

    /// Returns `true` if a route with the given ID exists.
    pub fn route_exists(&self, route_id: &str) -> bool {
        self.routes.route_exists(route_id)
    }

    /// Returns all route IDs.
    pub fn route_ids(&self) -> Vec<String> {
        self.routes.route_ids()
    }

    pub fn route_source_hash(&self, route_id: &str) -> Option<u64> {
        self.routes.route_source_hash(route_id)
    }

    /// Returns route IDs that should auto-start, sorted by startup order (ascending).
    pub fn auto_startup_route_ids(&self) -> Vec<String> {
        self.routes.auto_startup_route_ids()
    }

    /// Returns route IDs sorted by shutdown order (startup order descending).
    pub fn shutdown_route_ids(&self) -> Vec<String> {
        self.routes.shutdown_route_ids()
    }

    /// Atomically swap the pipeline of a route (zero-downtime).
    ///
    /// In-flight requests finish with the old pipeline (kept alive by Arc).
    /// New requests immediately use the new pipeline.
    ///
    /// ## Rejection policy
    ///
    /// Returns an error if the route has lifecycle-bearing steps or an active
    /// aggregate — these require the **Restart path** (stop → swap → start).
    ///
    /// The caller (e.g. `reload_actions::apply_swap`) MUST catch this rejection
    /// and fall back to:
    /// 1. `stop_route_reload` — drain lifecycle, stop consumer
    /// 2. `swap_pipeline_raw` — bypass the lifecycle check (route is stopped)
    /// 3. `start_route_reload` — re-create consumer with the new pipeline
    ///
    /// This is the "reject, don't defer" policy (oracle Fix 3): the swap is
    /// refused upfront rather than silently deferring or partially swapping.
    pub fn swap_pipeline(
        &self,
        route_id: &str,
        new_pipeline: BoxProcessor,
    ) -> Result<(), CamelError> {
        let managed = self
            .routes
            .get(route_id)
            .ok_or_else(|| CamelError::RouteError(format!("Route '{}' not found", route_id)))?;

        let assembly = managed.pipeline.load();
        let has_lifecycle = !assembly.lifecycle.is_empty();

        if has_lifecycle || managed.agg_service.is_some() {
            warn!(
                route_id = %route_id,
                "Hot-swap rejected — route has lifecycle/agg steps; use Restart path"
            );
            return Err(CamelError::RouteError(format!(
                "Route '{}' contains stateful steps (lifecycle-bearing). Hot-swap not supported — use restart.",
                route_id
            )));
        }

        drop(assembly);

        if managed.aggregate_split.is_some() {
            warn!(
                route_id = %route_id,
                "swap_pipeline: aggregate routes with timeout do not support hot-reload of pre/post segments"
            );
        }

        super::pipeline_runtime::swap_pipeline_raw(&managed.pipeline, new_pipeline);
        info!(route_id = %route_id, "Pipeline swapped atomically");
        Ok(())
    }

    /// Non-checking raw pipeline swap — bypasses lifecycle/aggregate rejection.
    ///
    /// Only for use after the route has been stopped (Restart path).
    /// Does NOT check for lifecycle handles or aggregate service — the caller
    /// is responsible for ensuring the route is safe to swap.
    pub(crate) fn swap_pipeline_raw(
        &self,
        route_id: &str,
        new_pipeline: BoxProcessor,
    ) -> Result<(), CamelError> {
        let managed = self
            .routes
            .get(route_id)
            .ok_or_else(|| CamelError::RouteError(format!("Route '{}' not found", route_id)))?;
        super::pipeline_runtime::swap_pipeline_raw(&managed.pipeline, new_pipeline);
        info!(route_id = %route_id, "Pipeline swapped (raw — lifecycle bypass)");
        Ok(())
    }

    /// Returns the from_uri of a route, if it exists.
    pub fn route_from_uri(&self, route_id: &str) -> Option<String> {
        self.routes.route_from_uri(route_id)
    }

    /// Get a clone of the current pipeline for a route.
    ///
    /// This is useful for testing and introspection.
    /// Returns `None` if the route doesn't exist.
    pub fn get_pipeline(&self, route_id: &str) -> Option<BoxProcessor> {
        self.routes.get_pipeline(route_id)
    }

    /// Internal stop implementation that can set custom status.
    pub(super) async fn stop_route_internal(&mut self, route_id: &str) -> Result<(), CamelError> {
        self.routes.stop_route(route_id).await
    }

    pub async fn start_route_reload(&mut self, route_id: &str) -> Result<(), CamelError> {
        self.start_route(route_id).await
    }

    pub async fn stop_route_reload(&mut self, route_id: &str) -> Result<(), CamelError> {
        self.stop_route(route_id).await
    }
}

// ── Aggregator route helpers ──

impl DefaultRouteController {
    /// Start a route with an aggregate split (pre-pipeline → aggregator → post-pipeline).
    ///
    /// Spawns a biased-select forward loop that routes exchanges through the
    /// pre-pipeline, aggregator, and post-pipeline in sequence, with late-exchange
    /// handling and force-completion on stop.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn start_aggregate_route(
        &mut self,
        route_id: &str,
        split: AggregateSplitInfo,
        consumer: Box<dyn Consumer>,
        consumer_ctx: ConsumerContext,
        mut rx: mpsc::Receiver<ExchangeEnvelope>,
        crash_notifier: Option<mpsc::Sender<CrashNotification>>,
        runtime_for_consumer: Option<Weak<dyn RuntimeHandle>>,
        tx_for_storage: mpsc::Sender<ExchangeEnvelope>,
        // Pipeline cancellation — a child of the managed route's pipeline_cancel_token.
        pipeline_cancel: CancellationToken,
    ) -> Result<(), CamelError> {
        let (late_tx, late_rx) = mpsc::channel::<Exchange>(256);

        let route_cancel_clone = pipeline_cancel.clone();
        let svc = AggregatorService::new(
            split.agg_config.clone(),
            late_tx,
            Arc::clone(&self.languages),
            route_cancel_clone,
        );
        let agg = Arc::new(svc);

        let pipeline_cancel_for_monitor = pipeline_cancel.clone();
        let agg_for_monitor = Arc::clone(&agg);

        {
            let managed = self
                .routes
                .get_mut(route_id)
                .expect("invariant: route must exist"); // allow-unwrap
            managed.agg_service = Some(Arc::clone(&agg));
        }

        let late_rx = Arc::new(tokio::sync::Mutex::new(late_rx));
        let pre_pipeline = split.pre_pipeline;
        let post_pipeline = split.post_pipeline;

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
                                if let Err(e) = pipe.processor.clone_inner().oneshot(ex).await {
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
                                let ex = match pre_pipe.processor.clone_inner().oneshot(exchange).await {
                                    Ok(ex) => ex,
                                    Err(e) => {
                                        if let Some(tx) = reply_tx { let _ = tx.send(Err(e)); }
                                        continue;
                                    }
                                };

                                let ex = {
                                    let cloned_svc = agg.as_ref().clone();
                                    cloned_svc.oneshot(ex).await
                                };

                                match ex {
                                    Ok(ex) => {
                                        if !is_pending(&ex) {
                                            let post_pipe = post_pipeline.load();
                                            let out = post_pipe.processor.clone_inner().oneshot(ex).await;
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
                        agg.force_complete_all();
                        let mut rx_guard = late_rx.lock().await;
                        while let Ok(late_ex) = rx_guard.try_recv() {
                            let pipe = post_pipeline.load();
                            let _ = pipe.processor.clone_inner().oneshot(late_ex).await;
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
        let force_on_stop = agg_for_monitor.config().force_completion_on_stop;
        let consumer_handle = tokio::spawn(async move {
            let _ = consumer_handle.await;
            if !pipeline_cancel_for_monitor.is_cancelled() {
                agg_for_monitor.force_complete_all();
                if force_on_stop {
                    pipeline_cancel_for_monitor.cancel();
                }
            }
        });
        #[cfg(test)]
        emit_start_route_event("consumer_spawned");

        {
            let managed = self
                .routes
                .get_mut(route_id)
                .expect("invariant: route must exist"); // allow-unwrap
            managed.consumer_handle = Some(consumer_handle);
            managed.pipeline_handle = Some(pipeline_handle);
            managed.channel_sender = Some(tx_for_storage);
        }

        info!(route_id = %route_id, "Route started (aggregate with timeout)");
        Ok(())
    }

    /// Test-only: inject lifecycle handles into an existing route's pipeline
    /// assembly.  This makes the route lifecycle-bearing so that swap_pipeline
    /// rejects it, forcing callers (like reload_actions::apply_swap) to take
    /// the Restart path instead.
    #[cfg(test)]
    pub(crate) fn set_route_lifecycle_for_test(
        &mut self,
        route_id: &str,
        lifecycle: Vec<Arc<dyn StepLifecycle>>,
    ) -> Result<(), CamelError> {
        use super::pipeline_runtime::PipelineAssembly;
        use camel_api::SyncBoxProcessor;
        use std::sync::Arc;

        let managed = self
            .routes
            .get_mut(route_id)
            .ok_or_else(|| CamelError::RouteError(format!("Route '{}' not found", route_id)))?;
        let old_processor = managed.pipeline.load().processor.clone_inner();
        managed.pipeline.store(Arc::new(PipelineAssembly::new(
            SyncBoxProcessor::new(old_processor),
            lifecycle,
        )));
        Ok(())
    }
}

#[cfg(test)]
#[path = "route_controller_tests.rs"]
mod tests;
