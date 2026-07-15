use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, trace, warn};

#[cfg(test)]
use camel_api::StepLifecycle;
use camel_api::component_metadata::ComponentMetadata;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{
    CamelError, FunctionInvoker, HealthReport, Lifecycle, MetricsCollector, PlatformIdentity,
    PlatformService, ReadinessGate, RouteTemplateSpec, RuntimeCommandBus, RuntimeQueryBus,
    TemplateInstanceRecord,
};
use camel_component_api::{Component, ComponentContext, ComponentRegistrar};
use camel_language_api::Language;

use crate::health_registry::HealthCheckRegistry;
use crate::lifecycle::adapters::controller_actor::RouteControllerHandle;
use crate::lifecycle::adapters::route_controller::SharedLanguageRegistry;
use crate::lifecycle::application::route_definition::RouteDefinition;
use crate::lifecycle::application::runtime_bus::RuntimeBus;
use crate::lifecycle::domain::LanguageRegistryError;
use crate::registry::RegistryError;
use crate::shared::components::domain::Registry;
use crate::shared::observability::domain::TracerConfig;
use crate::startup_validation::{ConfigCheck, run_startup_validation};
use crate::template::TemplateRegistry;

static CONTEXT_COMMAND_SEQ: AtomicU64 = AtomicU64::new(0);

pub use crate::context_builder::CamelContextBuilder;

/// The CamelContext is the runtime engine that manages components, routes, and their lifecycle.
///
/// # Lifecycle
///
/// Call [`start()`](Self::start) to launch routes, then [`stop()`](Self::stop)
/// or [`abort()`](Self::abort) to shut down. A stopped context can be restarted
/// by calling `start()` again — the controller actor stays alive across stop/start
/// cycles; only [`abort()`](Self::abort) is destructive.
pub struct CamelContext {
    registry: Arc<std::sync::Mutex<Registry>>,
    route_controller: RouteControllerHandle,
    actor_join: Option<tokio::task::JoinHandle<()>>,
    supervision_join: Option<tokio::task::JoinHandle<()>>,
    runtime: Arc<RuntimeBus>,
    cancel_token: CancellationToken,
    metrics: Arc<dyn MetricsCollector>,
    // Platform ports
    platform_service: Arc<dyn PlatformService>,
    languages: SharedLanguageRegistry,
    shutdown_timeout: std::time::Duration,
    services: Vec<Box<dyn Lifecycle>>,
    health_registry: Arc<HealthCheckRegistry>,
    component_configs: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    function_invoker: Option<Arc<dyn FunctionInvoker>>,
    template_registry: Arc<TemplateRegistry>,
    idempotent_repositories: crate::registry::SharedIdempotentRegistry,
    claim_check_repositories: crate::registry::SharedClaimCheckRegistry,
    /// Fail-closed startup validation registry (ADR-0033). Checks are drained
    /// and executed synchronously at the head of [`start()`](Self::start) before
    /// any route consumer is started.
    startup_checks: Vec<Box<dyn ConfigCheck>>,
}

/// Parts bag used by [`CamelContextBuilder::build`] to construct a [`CamelContext`]
/// without accessing private fields from a sibling module.
pub(crate) struct FromParts {
    pub(crate) registry: Arc<std::sync::Mutex<Registry>>,
    pub(crate) route_controller: RouteControllerHandle,
    pub(crate) _actor_join: tokio::task::JoinHandle<()>,
    pub(crate) supervision_join: Option<tokio::task::JoinHandle<()>>,
    pub(crate) runtime: Arc<RuntimeBus>,
    pub(crate) cancel_token: CancellationToken,
    pub(crate) metrics: Arc<dyn MetricsCollector>,
    pub(crate) platform_service: Arc<dyn PlatformService>,
    pub(crate) languages: SharedLanguageRegistry,
    pub(crate) shutdown_timeout: std::time::Duration,
    pub(crate) services: Vec<Box<dyn Lifecycle>>,
    pub(crate) health_registry: Arc<HealthCheckRegistry>,
    pub(crate) component_configs: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    pub(crate) function_invoker: Option<Arc<dyn FunctionInvoker>>,
    pub(crate) template_registry: Arc<TemplateRegistry>,
    pub(crate) idempotent_repositories: crate::registry::SharedIdempotentRegistry,
    pub(crate) claim_check_repositories: crate::registry::SharedClaimCheckRegistry,
    pub(crate) startup_checks: Vec<Box<dyn ConfigCheck>>,
}

impl CamelContext {
    pub(crate) fn from_parts(parts: FromParts) -> Self {
        Self {
            registry: parts.registry,
            route_controller: parts.route_controller,
            actor_join: Some(parts._actor_join),
            supervision_join: parts.supervision_join,
            runtime: parts.runtime,
            cancel_token: parts.cancel_token,
            metrics: parts.metrics,
            platform_service: parts.platform_service,
            languages: parts.languages,
            shutdown_timeout: parts.shutdown_timeout,
            services: parts.services,
            health_registry: parts.health_registry,
            component_configs: parts.component_configs,
            function_invoker: parts.function_invoker,
            template_registry: parts.template_registry,
            idempotent_repositories: parts.idempotent_repositories,
            claim_check_repositories: parts.claim_check_repositories,
            startup_checks: parts.startup_checks,
        }
    }
}

/// Opaque handle for runtime side-effect execution operations.
///
/// This intentionally does not expose direct lifecycle mutation APIs to callers.
#[derive(Clone)]
pub struct RuntimeExecutionHandle {
    pub(crate) controller: RouteControllerHandle,
    pub(crate) runtime: Arc<RuntimeBus>,
    pub(crate) function_invoker: Option<Arc<dyn FunctionInvoker>>,
    /// Lifecycle handles to inject into the compiled pipeline during
    /// `apply_swap`.  Used in tests to simulate lifecycle-bearing routes
    /// (e.g. resequencer).  Always `None` in production.
    #[cfg(test)]
    #[allow(clippy::type_complexity)]
    pub(crate) test_lifecycle_inject: Arc<std::sync::Mutex<Option<Vec<Arc<dyn StepLifecycle>>>>>,
}

impl RuntimeExecutionHandle {
    pub(crate) async fn add_route_definition(
        &self,
        definition: RouteDefinition,
    ) -> Result<(), CamelError> {
        use crate::lifecycle::ports::RouteRegistrationPort;
        self.runtime
            .register_route(definition)
            .await
            .map_err(Into::into)
    }

    /// Compile a route definition into a bare BoxProcessor (no lifecycle).
    /// Kept for tests; hot-reload uses the lifecycle-preserving variant instead.
    #[allow(dead_code)]
    pub(crate) async fn compile_route_definition(
        &self,
        definition: RouteDefinition,
    ) -> Result<camel_api::BoxProcessor, CamelError> {
        self.controller.compile_route_definition(definition).await
    }

    #[allow(dead_code)] // kept for potential future hot-reload paths
    pub(crate) async fn compile_route_definition_with_generation(
        &self,
        definition: RouteDefinition,
        generation: u64,
    ) -> Result<camel_api::BoxProcessor, CamelError> {
        self.controller
            .compile_route_definition_with_generation(definition, generation)
            .await
    }

    pub(crate) async fn compile_route_definition_pipeline(
        &self,
        definition: RouteDefinition,
        generation: u64,
    ) -> Result<crate::lifecycle::adapters::route_helpers::CompiledPipeline, CamelError> {
        self.controller
            .compile_route_definition_pipeline(definition, generation)
            .await
    }

    /// Compile without function generation, returning full CompiledPipeline.
    /// Oracle Fix 1: stateless hot-reload path preserves lifecycle handles.
    pub(crate) async fn compile_route_definition_dry_pipeline(
        &self,
        definition: RouteDefinition,
    ) -> Result<crate::lifecycle::adapters::route_helpers::CompiledPipeline, CamelError> {
        self.controller
            .compile_route_definition_dry_pipeline(definition)
            .await
    }

    pub(crate) async fn prepare_route_definition_with_generation(
        &self,
        definition: RouteDefinition,
        generation: u64,
    ) -> Result<crate::lifecycle::adapters::route_controller::PreparedRoute, CamelError> {
        self.controller
            .prepare_route_definition_with_generation(definition, generation)
            .await
    }

    pub(crate) async fn insert_prepared_route(
        &self,
        prepared: crate::lifecycle::adapters::route_controller::PreparedRoute,
    ) -> Result<(), CamelError> {
        self.controller.insert_prepared_route(prepared).await
    }

    pub(crate) async fn remove_route_preserving_functions(
        &self,
        route_id: String,
    ) -> Result<(), CamelError> {
        self.controller
            .remove_route_preserving_functions(route_id)
            .await
    }

    pub(crate) async fn register_route_aggregate(
        &self,
        route_id: String,
    ) -> Result<(), CamelError> {
        self.runtime.register_aggregate_only(route_id).await
    }

    pub(crate) async fn swap_route_pipeline(
        &self,
        route_id: &str,
        pipeline: camel_api::BoxProcessor,
    ) -> Result<(), CamelError> {
        self.controller.swap_pipeline(route_id, pipeline).await
    }

    /// Stop the route via the reload path (graceful lifecycle drain).
    pub(crate) async fn stop_route_reload(&self, route_id: &str) -> Result<(), CamelError> {
        self.controller.stop_route_reload(route_id).await
    }

    /// Start the route via the reload path (re-create consumer).
    pub(crate) async fn start_route_reload(&self, route_id: &str) -> Result<(), CamelError> {
        self.controller.start_route_reload(route_id).await
    }

    /// Raw pipeline swap — bypasses the lifecycle/aggregate rejection check.
    /// Only safe after the route has been stopped (Restart path).
    pub(crate) async fn swap_route_pipeline_raw(
        &self,
        route_id: &str,
        pipeline: camel_api::BoxProcessor,
        lifecycle: Vec<Arc<dyn camel_api::StepLifecycle>>,
    ) -> Result<(), CamelError> {
        self.controller
            .swap_pipeline_raw(route_id, pipeline, lifecycle)
            .await
    }

    pub(crate) async fn execute_runtime_command(
        &self,
        cmd: camel_api::RuntimeCommand,
    ) -> Result<camel_api::RuntimeCommandResult, CamelError> {
        self.runtime.execute(cmd).await
    }

    pub(crate) async fn runtime_route_status(
        &self,
        route_id: &str,
    ) -> Result<Option<String>, CamelError> {
        match self
            .runtime
            .ask(camel_api::RuntimeQuery::GetRouteStatus {
                route_id: route_id.to_string(),
            })
            .await
        {
            Ok(camel_api::RuntimeQueryResult::RouteStatus { status, .. }) => Ok(Some(status)),
            Ok(_) => Err(CamelError::RouteError(
                "unexpected runtime query response for route status".to_string(),
            )),
            Err(CamelError::RouteError(msg)) if msg.contains("not found") => Ok(None),
            Err(err) => Err(err),
        }
    }

    pub(crate) async fn runtime_route_ids(&self) -> Result<Vec<String>, CamelError> {
        match self.runtime.ask(camel_api::RuntimeQuery::ListRoutes).await {
            Ok(camel_api::RuntimeQueryResult::Routes { route_ids }) => Ok(route_ids),
            Ok(_) => Err(CamelError::RouteError(
                "unexpected runtime query response for route listing".to_string(),
            )),
            Err(err) => Err(err),
        }
    }

    pub(crate) async fn route_source_hash(&self, route_id: &str) -> Option<u64> {
        self.controller.route_source_hash(route_id).await
    }

    pub(crate) async fn in_flight_count(&self, route_id: &str) -> Result<u64, CamelError> {
        if !self.controller.route_exists(route_id).await? {
            return Err(CamelError::RouteError(format!(
                "Route '{}' not found",
                route_id
            )));
        }
        Ok(self
            .controller
            .in_flight_count(route_id)
            .await?
            .unwrap_or(0))
    }

    /// Check whether the running route has lifecycle-bearing steps.
    pub(crate) async fn route_has_lifecycle(&self, route_id: &str) -> bool {
        self.controller
            .route_has_lifecycle(route_id)
            .await
            .unwrap_or(false)
    }

    pub(crate) fn function_invoker(&self) -> Option<Arc<dyn FunctionInvoker>> {
        self.function_invoker.clone()
    }

    #[cfg(test)]
    pub(crate) async fn force_start_route_for_test(
        &self,
        route_id: &str,
    ) -> Result<(), CamelError> {
        self.controller.start_route(route_id).await
    }

    pub async fn controller_route_count_for_test(&self) -> usize {
        self.controller.route_count().await.unwrap_or(0)
    }
}

impl CamelContext {
    pub fn builder() -> CamelContextBuilder {
        CamelContextBuilder::new()
    }

    /// Set a global error handler applied to all routes without a per-route handler.
    pub async fn set_error_handler(&mut self, config: ErrorHandlerConfig) {
        let _ = self.route_controller.set_error_handler(config).await;
    }

    /// Enable or disable tracing globally.
    pub async fn set_tracing(&mut self, enabled: bool) {
        let _ = self
            .route_controller
            .set_tracer_config(TracerConfig {
                enabled,
                ..Default::default()
            })
            .await;
    }

    /// Configure tracing with full config.
    pub async fn set_tracer_config(&mut self, config: TracerConfig) {
        // Inject metrics collector if not already set
        let config = if config.metrics_collector.is_none() {
            TracerConfig {
                metrics_collector: Some(Arc::clone(&self.metrics)),
                ..config
            }
        } else {
            config
        };

        let _ = self.route_controller.set_tracer_config(config).await;
    }

    /// Builder-style: enable tracing with default config.
    pub async fn with_tracing(mut self) -> Self {
        self.set_tracing(true).await;
        self
    }

    /// Builder-style: configure tracing with custom config.
    /// Note: tracing subscriber initialization (stdout/file output) is handled
    /// separately via init_tracing_subscriber (called in camel-config bridge).
    pub async fn with_tracer_config(mut self, config: TracerConfig) -> Self {
        self.set_tracer_config(config).await;
        self
    }

    /// Register a lifecycle service (Apache Camel: addService pattern)
    ///
    /// For services exposing `as_function_invoker()`, the invoker is propagated
    /// to the route controller so that subsequent route definitions with function
    /// steps work correctly.
    ///
    /// Prefer [`CamelContextBuilder::with_lifecycle`] when possible, which wires
    /// the invoker at build time before any routes are added.
    pub fn with_lifecycle<L: Lifecycle + 'static>(mut self, service: L) -> Self {
        if let Some(collector) = service.as_metrics_collector() {
            self.metrics = collector;
        }
        if let Some(invoker) = service.as_function_invoker() {
            self.function_invoker = Some(invoker.clone());
            if let Err(e) = self.route_controller.try_set_function_invoker(invoker) {
                tracing::debug!("Failed to propagate function invoker to route controller: {e}");
            }
        }

        self.services.push(Box::new(service));
        self
    }

    /// Register a component with this context.
    ///
    /// Delegates to [`register_component_dyn`](Self::register_component_dyn)
    /// so metadata harvesting happens regardless of which entry point
    /// is used.
    pub fn register_component<C: Component + 'static>(&mut self, component: C) {
        self.register_component_dyn(Arc::new(component));
    }

    /// Register a startup `ConfigCheck` to be evaluated at the head of
    /// [`start()`](Self::start). Established by ADR-0033.
    ///
    /// Checks are drained and executed synchronously before any route consumer
    /// is started. If any check returns `Err`, `start()` fails closed with
    /// `CamelError::Config(_)` and no route is started. The check list is
    /// consumed (moved) during `start()` so this method may be called multiple
    /// times to register an arbitrary number of checks.
    pub fn add_startup_check(&mut self, check: Box<dyn ConfigCheck>) {
        self.startup_checks.push(check);
    }

    /// Register a language with this context, keyed by name.
    ///
    /// Returns `Err(LanguageRegistryError::AlreadyRegistered)` if a language
    /// with the same name is already registered. Use
    /// [`resolve_language`](Self::resolve_language) to check before
    /// registering, or choose a distinct name.
    pub fn register_language(
        &mut self,
        name: impl Into<String>,
        lang: Box<dyn Language>,
    ) -> Result<(), LanguageRegistryError> {
        let name = name.into();
        let mut languages = self
            .languages
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock"); // allow-unwrap
        if languages.contains_key(&name) {
            return Err(LanguageRegistryError::AlreadyRegistered { name });
        }
        languages.insert(name, Arc::from(lang));
        Ok(())
    }

    /// Resolve a language by name. Returns `None` if not registered.
    pub fn resolve_language(&self, name: &str) -> Option<Arc<dyn Language>> {
        let languages = self
            .languages
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock"); // allow-unwrap
        languages.get(name).cloned()
    }

    /// Add a route definition to this context.
    ///
    /// The route must have an ID. Steps are resolved immediately using registered components.
    pub async fn add_route_definition(
        &self,
        definition: RouteDefinition,
    ) -> Result<(), CamelError> {
        use crate::lifecycle::ports::RouteRegistrationPort;
        debug!(
            from = definition.from_uri(),
            route_id = %definition.route_id(),
            "Adding route definition"
        );
        self.runtime
            .register_route(definition)
            .await
            .map_err(Into::into)
    }

    fn next_context_command_id(op: &str, route_id: &str) -> String {
        let seq = CONTEXT_COMMAND_SEQ.fetch_add(1, Ordering::Relaxed);
        format!("context:{op}:{route_id}:{seq}")
    }

    /// Access the component registry.
    pub fn registry(&self) -> std::sync::MutexGuard<'_, Registry> {
        self.registry
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock") // allow-unwrap
    }

    /// Access the shared component registry Arc.
    pub fn registry_arc(&self) -> Arc<std::sync::Mutex<Registry>> {
        Arc::clone(&self.registry)
    }

    /// Get runtime execution handle for file-watcher integrations.
    pub fn runtime_execution_handle(&self) -> RuntimeExecutionHandle {
        RuntimeExecutionHandle {
            controller: self.route_controller.clone(),
            runtime: Arc::clone(&self.runtime),
            function_invoker: self.function_invoker.clone(),
            #[cfg(test)]
            test_lifecycle_inject: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Get the metrics collector.
    pub fn metrics(&self) -> Arc<dyn MetricsCollector> {
        Arc::clone(&self.metrics)
    }

    /// Get the platform service.
    pub fn platform_service(&self) -> Arc<dyn PlatformService> {
        Arc::clone(&self.platform_service)
    }

    /// Get the readiness gate port.
    pub fn readiness_gate(&self) -> Arc<dyn ReadinessGate> {
        self.platform_service.readiness_gate()
    }

    /// Get the platform identity.
    pub fn platform_identity(&self) -> PlatformIdentity {
        self.platform_service.identity()
    }

    /// Get the leadership service port.
    pub fn leadership(&self) -> Arc<dyn camel_api::LeadershipService> {
        self.platform_service.leadership()
    }

    /// Get runtime command/query bus handle.
    pub fn runtime(&self) -> Arc<dyn camel_api::RuntimeHandle> {
        self.runtime.clone()
    }

    /// Build a producer context wired to this runtime.
    pub fn producer_context(&self) -> camel_api::ProducerContext {
        camel_api::ProducerContext::new().with_runtime(self.runtime())
    }

    /// Query route status via runtime read-model.
    pub async fn runtime_route_status(&self, route_id: &str) -> Result<Option<String>, CamelError> {
        match self
            .runtime()
            .ask(camel_api::RuntimeQuery::GetRouteStatus {
                route_id: route_id.to_string(),
            })
            .await
        {
            Ok(camel_api::RuntimeQueryResult::RouteStatus { status, .. }) => Ok(Some(status)),
            Ok(_) => Err(CamelError::RouteError(
                "unexpected runtime query response for route status".to_string(),
            )),
            Err(CamelError::RouteError(msg)) if msg.contains("not found") => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// Start all routes. Each route's consumer will begin producing exchanges.
    ///
    /// Only routes with `auto_startup == true` will be started, in order of their
    /// `startup_order` (lower values start first).
    pub async fn start(&mut self) -> Result<(), CamelError> {
        info!("Starting CamelContext");

        // Reset cancellation state so a restart after stop() gets a fresh token.
        self.cancel_token = CancellationToken::new();

        // Start lifecycle services first
        for (i, service) in self.services.iter_mut().enumerate() {
            info!("Starting service: {}", service.name());
            if let Err(e) = service.start().await {
                // Rollback: stop already started services in reverse order
                warn!(
                    "Service {} failed to start, rolling back {} services",
                    service.name(),
                    i
                );
                for j in (0..i).rev() {
                    if let Err(rollback_err) = self.services[j].stop().await {
                        warn!(
                            "Failed to stop service {} during rollback: {}",
                            self.services[j].name(),
                            rollback_err
                        );
                    }
                }
                return Err(e);
            }
        }

        // ADR-0033: fail-closed startup validation. Drain the registered
        // ConfigCheck list and run every check synchronously. If any check
        // returns Err, refuse to start the runtime — no route consumer is
        // started, no reconciliation runs. Drains the registry so a second
        // call to start() (currently not supported) would not re-run checks.
        let checks = std::mem::take(&mut self.startup_checks);
        if let Err(e) = run_startup_validation(checks) {
            warn!("Startup validation failed: {e}");
            return Err(e);
        }

        // H8: boot reconciliation — fail routes stuck in transient state
        // (Starting/Stopping) from a previous run before auto_startup runs.
        self.runtime
            .reconcile_transient_states()
            .await
            .map_err(|e| CamelError::RouteError(format!("boot reconciliation failed: {e}")))?;

        // Then start routes via runtime command bus (aggregate-first),
        // preserving route controller startup ordering metadata.
        let route_ids = self.route_controller.auto_startup_route_ids().await?;
        for route_id in route_ids {
            self.runtime
                .execute(camel_api::RuntimeCommand::StartRoute {
                    route_id: route_id.clone(),
                    command_id: Self::next_context_command_id("start", &route_id),
                    causation_id: None,
                })
                .await?;
        }

        info!("CamelContext started");
        Ok(())
    }

    /// Graceful shutdown with default 30-second timeout.
    pub async fn stop(&mut self) -> Result<(), CamelError> {
        self.stop_timeout(self.shutdown_timeout).await
    }

    /// Graceful shutdown with custom timeout.
    ///
    /// Note: The timeout parameter is currently not propagated to the
    /// RouteController's per-route shutdown timeout. The RouteController
    /// uses a hardcoded 5-second default (`DEFAULT_SHUTDOWN_TIMEOUT`).
    /// Full propagation is planned for a future version.
    pub async fn stop_timeout(&mut self, _timeout: std::time::Duration) -> Result<(), CamelError> {
        info!("Stopping CamelContext");

        // Signal cancellation (for any legacy code that might use it)
        self.cancel_token.cancel();
        if let Some(join) = self.supervision_join.take() {
            join.abort();
        }

        // Stop all routes via runtime command bus (aggregate-first),
        // preserving route controller shutdown ordering metadata.
        let route_ids = self.route_controller.shutdown_route_ids().await?;
        for route_id in route_ids {
            if let Err(err) = self
                .runtime
                .execute(camel_api::RuntimeCommand::StopRoute {
                    route_id: route_id.clone(),
                    command_id: Self::next_context_command_id("stop", &route_id),
                    causation_id: None,
                })
                .await
            {
                warn!(route_id = %route_id, error = %err, "Runtime stop command failed during context shutdown");
            }
        }

        // The controller actor stays alive — it owns route registrations
        // needed for a subsequent start(). Destructive teardown (actor kill,
        // health cancel) happens only in abort().

        // Then stop lifecycle services in reverse insertion order (LIFO)
        // Continue stopping all services even if some fail
        let mut first_error = None;
        for service in self.services.iter_mut().rev() {
            info!("Stopping service: {}", service.name());
            if let Err(e) = service.stop().await {
                warn!("Service {} failed to stop: {}", service.name(), e);
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
        }

        info!("CamelContext stopped");

        if let Some(e) = first_error {
            Err(e)
        } else {
            Ok(())
        }
    }

    /// Get the graceful shutdown timeout used by [`stop()`](Self::stop).
    pub fn shutdown_timeout(&self) -> std::time::Duration {
        self.shutdown_timeout
    }

    /// Set the graceful shutdown timeout used by [`stop()`](Self::stop).
    pub fn set_shutdown_timeout(&mut self, timeout: std::time::Duration) {
        self.shutdown_timeout = timeout;
    }

    /// Test-only: take the actor join handle out of the context.
    /// Used to verify the actor exits gracefully after stop().
    #[cfg(test)]
    pub(crate) fn take_actor_join(&mut self) -> Option<tokio::task::JoinHandle<()>> {
        self.actor_join.take()
    }

    /// Immediate abort — kills all tasks without draining.
    pub async fn abort(&mut self) {
        self.cancel_token.cancel();
        if let Some(join) = self.supervision_join.take() {
            join.abort();
        }
        let route_ids = self
            .route_controller
            .shutdown_route_ids()
            .await
            .unwrap_or_default();
        for route_id in route_ids {
            let _ = self
                .runtime
                .execute(camel_api::RuntimeCommand::StopRoute {
                    route_id: route_id.clone(),
                    command_id: Self::next_context_command_id("abort-stop", &route_id),
                    causation_id: None,
                })
                .await;
        }

        for service in self.services.iter_mut().rev() {
            let name = service.name().to_string();
            match timeout(std::time::Duration::from_secs(5), service.stop()).await {
                Ok(Ok(())) => info!("Aborted service: {}", name),
                Ok(Err(e)) => warn!("Service {} failed to stop during abort: {}", name, e),
                Err(_) => warn!("Service {} timed out during abort (5s)", name),
            }
        }

        // Destructive teardown: kill the controller actor and cancel health
        // probes. This is what makes abort() non-restartable vs stop().
        let _ = self.route_controller.shutdown().await;
        self.health_registry.cancel_token().cancel();
        if let Some(mut join) = self.actor_join.take() {
            match tokio::time::timeout(std::time::Duration::from_secs(5), &mut join).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => warn!("Controller actor task error during abort: {e}"),
                Err(_) => {
                    warn!("Controller actor did not stop within 5s during abort; force-aborting");
                    join.abort();
                    let _ = join.await;
                }
            }
        }
    }

    /// Check health status of all registered services and lifecycle services.
    pub async fn health_check(&self) -> HealthReport {
        use camel_api::HealthSource;
        self.health_report().await
    }

    pub fn health_registry(&self) -> Arc<HealthCheckRegistry> {
        Arc::clone(&self.health_registry)
    }

    /// Store a component config. Overwrites any previously stored config of the same type.
    pub fn set_component_config<T: 'static + Send + Sync>(&mut self, config: T) {
        self.component_configs
            .insert(TypeId::of::<T>(), Box::new(config));
    }

    /// Retrieve a stored component config by type. Returns None if not stored.
    pub fn get_component_config<T: 'static + Send + Sync>(&self) -> Option<&T> {
        self.component_configs
            .get(&TypeId::of::<T>())
            .and_then(|b| b.downcast_ref::<T>())
    }

    // --- Component Metadata ---

    /// Get a component's harvested metadata by URI scheme.
    pub fn component_metadata(&self, scheme: &str) -> Option<ComponentMetadata> {
        self.registry.lock().ok()?.get_metadata(scheme)
    }

    /// Get metadata for every registered component.
    pub fn all_component_metadata(&self) -> Vec<ComponentMetadata> {
        self.registry
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock") // allow-unwrap
            .all_metadata()
    }

    /// Get a metadata catalog handle implementing
    /// [`ComponentMetadataCatalog`](camel_api::component_metadata::ComponentMetadataCatalog).
    ///
    /// The returned handle shares the same `Arc<Mutex<Registry>>` as the
    /// context, so registrations made through one are visible through the
    /// other.
    pub fn metadata_catalog(
        &self,
    ) -> crate::component_metadata_catalog::RuntimeComponentMetadataCatalog {
        crate::component_metadata_catalog::RuntimeComponentMetadataCatalog::new(Arc::clone(
            &self.registry,
        ))
    }

    // --- Route Template Registry (data-only) ---

    /// Register a route template specification.
    ///
    /// Returns `Err(CamelError)` if a template with the same ID is already registered.
    pub fn add_route_template(&self, spec: RouteTemplateSpec) -> Result<(), CamelError> {
        self.template_registry.register(spec)
    }

    /// Retrieve a route template specification by its ID.
    pub fn get_route_template(&self, id: &str) -> Option<RouteTemplateSpec> {
        self.template_registry.get(id)
    }

    /// Return all registered template IDs.
    pub fn template_ids(&self) -> Vec<String> {
        self.template_registry.template_ids()
    }

    /// Record a newly instantiated template instance.
    pub fn record_template_instance(&self, record: TemplateInstanceRecord) {
        self.template_registry.record_instance(record)
    }

    /// Return all instance records for a given template ID.
    pub fn template_instances(&self, template_id: &str) -> Vec<TemplateInstanceRecord> {
        self.template_registry.instances(template_id)
    }

    // --- Idempotent Repository Registry ---

    /// Register an idempotent repository.
    ///
    /// Returns `Err(RegistryError::AlreadyRegistered)` if a repository with
    /// the same name is already registered.
    pub fn register_idempotent_repository(
        &mut self,
        name: impl Into<String>,
        repo: Arc<dyn camel_api::IdempotentRepository>,
    ) -> Result<(), RegistryError> {
        self.idempotent_repositories.register(name, repo)
    }

    /// Retrieve an idempotent repository by name.
    pub fn idempotent_repository(
        &self,
        name: &str,
    ) -> Option<Arc<dyn camel_api::IdempotentRepository>> {
        self.idempotent_repositories.get(name)
    }

    // --- Claim Check Repository Registry ---

    /// Register a claim check repository.
    ///
    /// Returns `Err(RegistryError::AlreadyRegistered)` if a repository with
    /// the same name is already registered.
    pub fn register_claim_check_repository(
        &mut self,
        name: impl Into<String>,
        repo: Arc<dyn camel_api::ClaimCheckRepository>,
    ) -> Result<(), RegistryError> {
        self.claim_check_repositories.register(name, repo)
    }

    /// Retrieve a claim check repository by name.
    pub fn claim_check_repository(
        &self,
        name: &str,
    ) -> Option<Arc<dyn camel_api::ClaimCheckRepository>> {
        self.claim_check_repositories.get(name)
    }
}

impl ComponentRegistrar for CamelContext {
    fn register_component_dyn(&mut self, component: Arc<dyn Component>) {
        let scheme = component.scheme().to_string();
        self.registry
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock") // allow-unwrap
            .register(component);
        trace!(scheme, "Registered component");
    }
}

impl ComponentContext for CamelContext {
    fn resolve_component(&self, scheme: &str) -> Option<Arc<dyn Component>> {
        self.registry.lock().ok()?.get(scheme)
    }

    fn resolve_language(&self, name: &str) -> Option<Arc<dyn Language>> {
        self.languages.lock().ok()?.get(name).cloned()
    }

    fn metrics(&self) -> Arc<dyn MetricsCollector> {
        Arc::clone(&self.metrics)
    }

    fn health(&self) -> Arc<dyn camel_component_api::HealthCheckRegistry> {
        // The concrete HealthCheckRegistry struct implements the trait via
        // the impl added in health_registry.rs.
        Arc::clone(&self.health_registry) as Arc<dyn camel_component_api::HealthCheckRegistry>
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
}

#[async_trait::async_trait]
impl camel_api::HealthSource for CamelContext {
    async fn liveness(&self) -> camel_api::HealthStatus {
        let has_failed = self
            .services
            .iter()
            .any(|s| s.status() == camel_api::ServiceStatus::Failed);
        if has_failed {
            camel_api::HealthStatus::Unhealthy
        } else {
            camel_api::HealthStatus::Healthy
        }
    }

    async fn readiness(&self) -> camel_api::HealthStatus {
        let has_failed = self
            .services
            .iter()
            .any(|s| s.status() == camel_api::ServiceStatus::Failed);
        if has_failed {
            return camel_api::HealthStatus::Unhealthy;
        }
        let has_stopped = self
            .services
            .iter()
            .any(|s| s.status() == camel_api::ServiceStatus::Stopped);
        if has_stopped {
            return camel_api::HealthStatus::Degraded;
        }
        self.health_registry.check_all().await.status
    }

    async fn health_report(&self) -> camel_api::HealthReport {
        let mut report = self.health_registry.check_all().await;
        let mut worst = report.status;
        for service in &self.services {
            let svc_status = service.status();
            let health = match svc_status {
                camel_api::ServiceStatus::Started => camel_api::HealthStatus::Healthy,
                camel_api::ServiceStatus::Stopped => camel_api::HealthStatus::Degraded,
                camel_api::ServiceStatus::Failed => camel_api::HealthStatus::Unhealthy,
            };
            if matches!(worst, camel_api::HealthStatus::Healthy)
                && matches!(
                    health,
                    camel_api::HealthStatus::Degraded | camel_api::HealthStatus::Unhealthy
                )
            {
                worst = health;
            }
            if matches!(worst, camel_api::HealthStatus::Degraded)
                && matches!(health, camel_api::HealthStatus::Unhealthy)
            {
                worst = health;
            }
            report.services.push(camel_api::ServiceHealth {
                name: service.name().to_string(),
                status: svc_status,
                message: None,
            });
        }
        report.status = worst;
        report
    }

    async fn startup(&self) -> camel_api::HealthStatus {
        camel_api::HealthStatus::Healthy
    }
}

#[cfg(test)]
#[path = "context_tests.rs"]
mod context_tests;
