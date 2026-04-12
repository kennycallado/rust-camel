use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{
    CamelError, HealthReport, HealthStatus, Lifecycle, MetricsCollector, NoOpMetrics,
    RuntimeCommandBus, RuntimeQueryBus, ServiceHealth, ServiceStatus, SupervisionConfig,
};
use camel_component_api::{Component, ComponentContext, ComponentRegistrar};
use camel_language_api::Language;

use crate::lifecycle::adapters::RuntimeExecutionAdapter;
use crate::lifecycle::adapters::controller_actor::{
    RouteControllerHandle, spawn_controller_actor, spawn_supervision_task,
};
use crate::lifecycle::adapters::route_controller::{
    DefaultRouteController, SharedLanguageRegistry,
};
use crate::lifecycle::application::route_definition::RouteDefinition;
use crate::lifecycle::application::runtime_bus::RuntimeBus;
use crate::lifecycle::domain::LanguageRegistryError;
use crate::shared::components::domain::Registry;
use crate::shared::observability::domain::TracerConfig;

static CONTEXT_COMMAND_SEQ: AtomicU64 = AtomicU64::new(0);

pub struct CamelContextBuilder {
    registry: Option<Arc<std::sync::Mutex<Registry>>>,
    languages: Option<SharedLanguageRegistry>,
    metrics: Option<Arc<dyn MetricsCollector>>,
    supervision_config: Option<SupervisionConfig>,
    runtime_store: Option<crate::lifecycle::adapters::InMemoryRuntimeStore>,
    shutdown_timeout: std::time::Duration,
}

/// The CamelContext is the runtime engine that manages components, routes, and their lifecycle.
///
/// # Lifecycle
///
/// A `CamelContext` is single-use: call [`start()`](Self::start) once to launch routes,
/// then [`stop()`](Self::stop) or [`abort()`](Self::abort) to shut down. Restarting a
/// stopped context is not supported — create a new instance instead.
pub struct CamelContext {
    registry: Arc<std::sync::Mutex<Registry>>,
    route_controller: RouteControllerHandle,
    _actor_join: tokio::task::JoinHandle<()>,
    supervision_join: Option<tokio::task::JoinHandle<()>>,
    runtime: Arc<RuntimeBus>,
    cancel_token: CancellationToken,
    metrics: Arc<dyn MetricsCollector>,
    languages: SharedLanguageRegistry,
    shutdown_timeout: std::time::Duration,
    services: Vec<Box<dyn Lifecycle>>,
    component_configs: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

/// Opaque handle for runtime side-effect execution operations.
///
/// This intentionally does not expose direct lifecycle mutation APIs to callers.
#[derive(Clone)]
pub struct RuntimeExecutionHandle {
    controller: RouteControllerHandle,
    runtime: Arc<RuntimeBus>,
}

impl RuntimeExecutionHandle {
    pub(crate) async fn add_route_definition(
        &self,
        definition: RouteDefinition,
    ) -> Result<(), CamelError> {
        use crate::lifecycle::ports::RouteRegistrationPort;
        self.runtime.register_route(definition).await
    }

    pub(crate) async fn compile_route_definition(
        &self,
        definition: RouteDefinition,
    ) -> Result<camel_api::BoxProcessor, CamelError> {
        self.controller.compile_route_definition(definition).await
    }

    pub(crate) async fn swap_route_pipeline(
        &self,
        route_id: &str,
        pipeline: camel_api::BoxProcessor,
    ) -> Result<(), CamelError> {
        self.controller.swap_pipeline(route_id, pipeline).await
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

    #[cfg(test)]
    pub(crate) async fn force_start_route_for_test(
        &self,
        route_id: &str,
    ) -> Result<(), CamelError> {
        self.controller.start_route(route_id).await
    }

    #[cfg(test)]
    pub(crate) async fn controller_route_count_for_test(&self) -> usize {
        self.controller.route_count().await.unwrap_or(0)
    }
}

impl CamelContext {
    fn built_in_languages() -> SharedLanguageRegistry {
        let mut languages: HashMap<String, Arc<dyn Language>> = HashMap::new();
        languages.insert(
            "simple".to_string(),
            Arc::new(camel_language_simple::SimpleLanguage),
        );
        #[cfg(feature = "lang-js")]
        {
            let js_lang = camel_language_js::JsLanguage::new();
            languages.insert("js".to_string(), Arc::new(js_lang.clone()));
            languages.insert("javascript".to_string(), Arc::new(js_lang));
        }
        #[cfg(feature = "lang-rhai")]
        {
            let rhai_lang = camel_language_rhai::RhaiLanguage::new();
            languages.insert("rhai".to_string(), Arc::new(rhai_lang));
        }
        #[cfg(feature = "lang-jsonpath")]
        {
            languages.insert(
                "jsonpath".to_string(),
                Arc::new(camel_language_jsonpath::JsonPathLanguage),
            );
        }
        #[cfg(feature = "lang-xpath")]
        {
            languages.insert(
                "xpath".to_string(),
                Arc::new(camel_language_xpath::XPathLanguage),
            );
        }
        Arc::new(std::sync::Mutex::new(languages))
    }

    fn build_runtime(
        controller: RouteControllerHandle,
        store: crate::lifecycle::adapters::InMemoryRuntimeStore,
    ) -> Arc<RuntimeBus> {
        let execution = Arc::new(RuntimeExecutionAdapter::new(controller));
        Arc::new(
            RuntimeBus::new(
                Arc::new(store.clone()),
                Arc::new(store.clone()),
                Arc::new(store.clone()),
                Arc::new(store.clone()),
            )
            .with_uow(Arc::new(store))
            .with_execution(execution),
        )
    }

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
    pub fn with_lifecycle<L: Lifecycle + 'static>(mut self, service: L) -> Self {
        // Auto-register MetricsCollector if available
        if let Some(collector) = service.as_metrics_collector() {
            self.metrics = collector;
        }

        self.services.push(Box::new(service));
        self
    }

    /// Register a component with this context.
    pub fn register_component<C: Component + 'static>(&mut self, component: C) {
        info!(scheme = component.scheme(), "Registering component");
        self.registry
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock")
            .register(Arc::new(component));
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
            .expect("mutex poisoned: another thread panicked while holding this lock");
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
            .expect("mutex poisoned: another thread panicked while holding this lock");
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
        info!(
            from = definition.from_uri(),
            route_id = %definition.route_id(),
            "Adding route definition"
        );
        self.runtime.register_route(definition).await
    }

    fn next_context_command_id(op: &str, route_id: &str) -> String {
        let seq = CONTEXT_COMMAND_SEQ.fetch_add(1, Ordering::Relaxed);
        format!("context:{op}:{route_id}:{seq}")
    }

    /// Access the component registry.
    pub fn registry(&self) -> std::sync::MutexGuard<'_, Registry> {
        self.registry
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock")
    }

    /// Get runtime execution handle for file-watcher integrations.
    pub fn runtime_execution_handle(&self) -> RuntimeExecutionHandle {
        RuntimeExecutionHandle {
            controller: self.route_controller.clone(),
            runtime: Arc::clone(&self.runtime),
        }
    }

    /// Get the metrics collector.
    pub fn metrics(&self) -> Arc<dyn MetricsCollector> {
        Arc::clone(&self.metrics)
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
    /// Note: The timeout parameter is currently not used directly; the RouteController
    /// manages its own shutdown timeout. This may change in a future version.
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
    }

    /// Check health status of all registered services.
    pub fn health_check(&self) -> HealthReport {
        let services: Vec<ServiceHealth> = self
            .services
            .iter()
            .map(|s| ServiceHealth {
                name: s.name().to_string(),
                status: s.status(),
            })
            .collect();

        let status = if services.iter().all(|s| s.status == ServiceStatus::Started) {
            HealthStatus::Healthy
        } else {
            HealthStatus::Unhealthy
        };

        HealthReport {
            status,
            services,
            ..Default::default()
        }
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
}

impl ComponentRegistrar for CamelContext {
    fn register_component_dyn(&mut self, component: Arc<dyn Component>) {
        self.registry
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock")
            .register(component);
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
}

impl CamelContextBuilder {
    pub fn new() -> Self {
        Self {
            registry: None,
            languages: None,
            metrics: None,
            supervision_config: None,
            runtime_store: None,
            shutdown_timeout: std::time::Duration::from_secs(30),
        }
    }

    pub fn registry(mut self, registry: Arc<std::sync::Mutex<Registry>>) -> Self {
        self.registry = Some(registry);
        self
    }

    pub fn languages(mut self, languages: SharedLanguageRegistry) -> Self {
        self.languages = Some(languages);
        self
    }

    pub fn metrics(mut self, metrics: Arc<dyn MetricsCollector>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    pub fn supervision(mut self, config: SupervisionConfig) -> Self {
        self.supervision_config = Some(config);
        self
    }

    pub fn runtime_store(
        mut self,
        store: crate::lifecycle::adapters::InMemoryRuntimeStore,
    ) -> Self {
        self.runtime_store = Some(store);
        self
    }

    pub fn shutdown_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.shutdown_timeout = timeout;
        self
    }

    pub async fn build(self) -> Result<CamelContext, CamelError> {
        let registry = self
            .registry
            .unwrap_or_else(|| Arc::new(std::sync::Mutex::new(Registry::new())));
        let languages = self
            .languages
            .unwrap_or_else(CamelContext::built_in_languages);
        let metrics = self.metrics.unwrap_or_else(|| Arc::new(NoOpMetrics));

        let (controller, actor_join, supervision_join) =
            if let Some(config) = self.supervision_config {
                let (crash_tx, crash_rx) = tokio::sync::mpsc::channel(64);
                let mut controller_impl = DefaultRouteController::with_languages(
                    Arc::clone(&registry),
                    Arc::clone(&languages),
                );
                controller_impl.set_crash_notifier(crash_tx);
                let (controller, actor_join) = spawn_controller_actor(controller_impl);
                let supervision_join = spawn_supervision_task(
                    controller.clone(),
                    config,
                    Some(Arc::clone(&metrics)),
                    crash_rx,
                );
                (controller, actor_join, Some(supervision_join))
            } else {
                let controller_impl = DefaultRouteController::with_languages(
                    Arc::clone(&registry),
                    Arc::clone(&languages),
                );
                let (controller, actor_join) = spawn_controller_actor(controller_impl);
                (controller, actor_join, None)
            };

        let store = self.runtime_store.unwrap_or_default();
        let runtime = CamelContext::build_runtime(controller.clone(), store);
        let runtime_handle: Arc<dyn camel_api::RuntimeHandle> = runtime.clone();
        controller
            .try_set_runtime_handle(runtime_handle)
            .expect("controller actor mailbox should accept initial runtime handle");

        Ok(CamelContext {
            registry,
            route_controller: controller,
            _actor_join: actor_join,
            supervision_join,
            runtime,
            cancel_token: CancellationToken::new(),
            metrics,
            languages,
            shutdown_timeout: self.shutdown_timeout,
            services: Vec::new(),
            component_configs: HashMap::new(),
        })
    }
}

impl Default for CamelContextBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "context_tests.rs"]
mod context_tests;
