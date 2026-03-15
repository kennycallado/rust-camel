use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{
    CamelError, HealthReport, HealthStatus, Lifecycle, MetricsCollector, NoOpMetrics,
    RouteController, RouteStatus, ServiceHealth, ServiceStatus, SupervisionConfig,
};
use camel_component::Component;
use camel_language_api::Language;
use camel_language_api::LanguageError;

use crate::config::TracerConfig;
use crate::registry::Registry;
use crate::route::RouteDefinition;
use crate::route_controller::{
    DefaultRouteController, RouteControllerInternal, SharedLanguageRegistry,
};
use crate::supervising_route_controller::SupervisingRouteController;

/// The CamelContext is the runtime engine that manages components, routes, and their lifecycle.
///
/// # Lifecycle
///
/// A `CamelContext` is single-use: call [`start()`](Self::start) once to launch routes,
/// then [`stop()`](Self::stop) or [`abort()`](Self::abort) to shut down. Restarting a
/// stopped context is not supported — create a new instance instead.
pub struct CamelContext {
    registry: Arc<std::sync::Mutex<Registry>>,
    route_controller: Arc<Mutex<dyn RouteControllerInternal>>,
    cancel_token: CancellationToken,
    metrics: Arc<dyn MetricsCollector>,
    languages: SharedLanguageRegistry,
    shutdown_timeout: std::time::Duration,
    services: Vec<Box<dyn Lifecycle>>,
    component_configs: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl CamelContext {
    fn built_in_languages() -> SharedLanguageRegistry {
        let mut languages: HashMap<String, Arc<dyn Language>> = HashMap::new();
        languages.insert(
            "simple".to_string(),
            Arc::new(camel_language_simple::SimpleLanguage),
        );
        Arc::new(std::sync::Mutex::new(languages))
    }

    /// Create a new, empty CamelContext.
    pub fn new() -> Self {
        Self::with_metrics(Arc::new(NoOpMetrics))
    }

    /// Create a new CamelContext with a custom metrics collector.
    pub fn with_metrics(metrics: Arc<dyn MetricsCollector>) -> Self {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let languages = Self::built_in_languages();
        let controller: Arc<Mutex<dyn RouteControllerInternal>> = Arc::new(Mutex::new(
            DefaultRouteController::with_languages(Arc::clone(&registry), Arc::clone(&languages)),
        ));

        // Set self-ref so DefaultRouteController can create ProducerContext
        // Use try_lock since we just created it and nobody else has access yet
        controller
            .try_lock()
            .expect("BUG: CamelContext lock contention — try_lock should always succeed here since &mut self prevents concurrent access")
            .set_self_ref(Arc::clone(&controller) as Arc<Mutex<dyn RouteController>>);

        Self {
            registry,
            route_controller: controller,
            cancel_token: CancellationToken::new(),
            metrics,
            languages,
            shutdown_timeout: std::time::Duration::from_secs(30),
            services: Vec::new(),
            component_configs: HashMap::new(),
        }
    }

    /// Create a new CamelContext with route supervision enabled.
    ///
    /// The supervision config controls automatic restart behavior for crashed routes.
    pub fn with_supervision(config: SupervisionConfig) -> Self {
        Self::with_supervision_and_metrics(config, Arc::new(NoOpMetrics))
    }

    /// Create a new CamelContext with route supervision and custom metrics.
    ///
    /// The supervision config controls automatic restart behavior for crashed routes.
    pub fn with_supervision_and_metrics(
        config: SupervisionConfig,
        metrics: Arc<dyn MetricsCollector>,
    ) -> Self {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let languages = Self::built_in_languages();
        let controller: Arc<Mutex<dyn RouteControllerInternal>> = Arc::new(Mutex::new(
            SupervisingRouteController::with_languages(
                Arc::clone(&registry),
                config,
                Arc::clone(&languages),
            )
            .with_metrics(Arc::clone(&metrics)),
        ));

        // Set self-ref so SupervisingRouteController can create ProducerContext
        // Use try_lock since we just created it and nobody else has access yet
        controller
            .try_lock()
            .expect("BUG: CamelContext lock contention — try_lock should always succeed here since &mut self prevents concurrent access")
            .set_self_ref(Arc::clone(&controller) as Arc<Mutex<dyn RouteController>>);

        Self {
            registry,
            route_controller: controller,
            cancel_token: CancellationToken::new(),
            metrics,
            languages,
            shutdown_timeout: std::time::Duration::from_secs(30),
            services: Vec::new(),
            component_configs: HashMap::new(),
        }
    }

    /// Set a global error handler applied to all routes without a per-route handler.
    pub fn set_error_handler(&mut self, config: ErrorHandlerConfig) {
        self.route_controller
            .try_lock()
            .expect("BUG: CamelContext lock contention — try_lock should always succeed here since &mut self prevents concurrent access")
            .set_error_handler(config);
    }

    /// Enable or disable tracing globally.
    pub fn set_tracing(&mut self, enabled: bool) {
        self.route_controller
            .try_lock()
            .expect("BUG: CamelContext lock contention — try_lock should always succeed here since &mut self prevents concurrent access")
            .set_tracer_config(&TracerConfig {
                enabled,
                ..Default::default()
            });
    }

    /// Configure tracing with full config.
    pub fn set_tracer_config(&mut self, config: TracerConfig) {
        // Inject metrics collector if not already set
        let config = if config.metrics_collector.is_none() {
            TracerConfig {
                metrics_collector: Some(Arc::clone(&self.metrics)),
                ..config
            }
        } else {
            config
        };

        self.route_controller
            .try_lock()
            .expect("BUG: CamelContext lock contention — try_lock should always succeed here since &mut self prevents concurrent access")
            .set_tracer_config(&config);
    }

    /// Builder-style: enable tracing with default config.
    pub fn with_tracing(mut self) -> Self {
        self.set_tracing(true);
        self
    }

    /// Builder-style: configure tracing with custom config.
    /// Note: tracing subscriber initialization (stdout/file output) is handled
    /// separately via init_tracing_subscriber (called in camel-config bridge).
    pub fn with_tracer_config(mut self, config: TracerConfig) -> Self {
        self.set_tracer_config(config);
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
            .register(component);
    }

    /// Register a language with this context, keyed by name.
    ///
    /// Returns `Err(LanguageError::AlreadyRegistered)` if a language with the
    /// same name is already registered. Use [`resolve_language`](Self::resolve_language)
    /// to check before registering, or choose a distinct name.
    pub fn register_language(
        &mut self,
        name: impl Into<String>,
        lang: Box<dyn Language>,
    ) -> Result<(), LanguageError> {
        let name = name.into();
        let mut languages = self
            .languages
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock");
        if languages.contains_key(&name) {
            return Err(LanguageError::AlreadyRegistered(name));
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
    pub fn add_route_definition(&mut self, definition: RouteDefinition) -> Result<(), CamelError> {
        info!(from = definition.from_uri(), route_id = %definition.route_id(), "Adding route definition");

        self.route_controller
            .try_lock()
            .expect("BUG: CamelContext lock contention — try_lock should always succeed here since &mut self prevents concurrent access")
            .add_route(definition)
    }

    /// Access the component registry.
    pub fn registry(&self) -> std::sync::MutexGuard<'_, Registry> {
        self.registry
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock")
    }

    /// Access the route controller.
    pub fn route_controller(&self) -> &Arc<Mutex<dyn RouteControllerInternal>> {
        &self.route_controller
    }

    /// Get the metrics collector.
    pub fn metrics(&self) -> Arc<dyn MetricsCollector> {
        Arc::clone(&self.metrics)
    }

    /// Get the status of a route by ID.
    pub fn route_status(&self, route_id: &str) -> Option<RouteStatus> {
        self.route_controller
            .try_lock()
            .ok()?
            .route_status(route_id)
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

        // Then start routes
        self.route_controller
            .lock()
            .await
            .start_all_routes()
            .await?;

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

        // Stop all routes via the controller
        self.route_controller.lock().await.stop_all_routes().await?;

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
        let _ = self.route_controller.lock().await.stop_all_routes().await;
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

impl Default for CamelContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::{BuilderStep, LanguageExpressionDef, RouteDefinition};
    use camel_api::CamelError;
    use camel_component::Endpoint;

    /// Mock component for testing
    struct MockComponent;

    impl Component for MockComponent {
        fn scheme(&self) -> &str {
            "mock"
        }

        fn create_endpoint(&self, _uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
            Err(CamelError::ComponentNotFound("mock".to_string()))
        }
    }

    #[test]
    fn test_context_handles_mutex_poisoning_gracefully() {
        let mut ctx = CamelContext::new();

        // Register a component successfully
        ctx.register_component(MockComponent);

        // Access registry should work even after potential panic in another thread
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = ctx.registry();
        }));

        assert!(
            result.is_ok(),
            "Registry access should handle mutex poisoning"
        );
    }

    #[test]
    fn test_context_resolves_simple_language() {
        let ctx = CamelContext::new();
        let lang = ctx
            .resolve_language("simple")
            .expect("simple language not found");
        assert_eq!(lang.name(), "simple");
    }

    #[test]
    fn test_simple_language_via_context() {
        let ctx = CamelContext::new();
        let lang = ctx.resolve_language("simple").unwrap();
        let pred = lang.create_predicate("${header.x} == 'hello'").unwrap();
        let mut msg = camel_api::message::Message::default();
        msg.set_header("x", camel_api::Value::String("hello".into()));
        let ex = camel_api::exchange::Exchange::new(msg);
        assert!(pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_resolve_unknown_language_returns_none() {
        let ctx = CamelContext::new();
        assert!(ctx.resolve_language("nonexistent").is_none());
    }

    #[test]
    fn test_register_language_duplicate_returns_error() {
        use camel_language_api::LanguageError;
        struct DummyLang;
        impl camel_language_api::Language for DummyLang {
            fn name(&self) -> &'static str {
                "dummy"
            }
            fn create_expression(
                &self,
                _: &str,
            ) -> Result<Box<dyn camel_language_api::Expression>, LanguageError> {
                Err(LanguageError::EvalError("not implemented".into()))
            }
            fn create_predicate(
                &self,
                _: &str,
            ) -> Result<Box<dyn camel_language_api::Predicate>, LanguageError> {
                Err(LanguageError::EvalError("not implemented".into()))
            }
        }

        let mut ctx = CamelContext::new();
        ctx.register_language("dummy", Box::new(DummyLang)).unwrap();
        let result = ctx.register_language("dummy", Box::new(DummyLang));
        assert!(result.is_err(), "duplicate registration should fail");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("dummy"),
            "error should mention the language name"
        );
    }

    #[test]
    fn test_register_language_new_key_succeeds() {
        use camel_language_api::LanguageError;
        struct DummyLang;
        impl camel_language_api::Language for DummyLang {
            fn name(&self) -> &'static str {
                "dummy"
            }
            fn create_expression(
                &self,
                _: &str,
            ) -> Result<Box<dyn camel_language_api::Expression>, LanguageError> {
                Err(LanguageError::EvalError("not implemented".into()))
            }
            fn create_predicate(
                &self,
                _: &str,
            ) -> Result<Box<dyn camel_language_api::Predicate>, LanguageError> {
                Err(LanguageError::EvalError("not implemented".into()))
            }
        }

        let mut ctx = CamelContext::new();
        let result = ctx.register_language("dummy", Box::new(DummyLang));
        assert!(result.is_ok(), "first registration should succeed");
    }

    #[test]
    fn test_add_route_definition_uses_runtime_registered_language() {
        use camel_language_api::{Expression, LanguageError, Predicate};

        struct DummyExpression;
        impl Expression for DummyExpression {
            fn evaluate(
                &self,
                _exchange: &camel_api::Exchange,
            ) -> Result<camel_api::Value, LanguageError> {
                Ok(camel_api::Value::String("ok".into()))
            }
        }

        struct DummyPredicate;
        impl Predicate for DummyPredicate {
            fn matches(&self, _exchange: &camel_api::Exchange) -> Result<bool, LanguageError> {
                Ok(true)
            }
        }

        struct RuntimeLang;
        impl camel_language_api::Language for RuntimeLang {
            fn name(&self) -> &'static str {
                "runtime"
            }

            fn create_expression(
                &self,
                _script: &str,
            ) -> Result<Box<dyn Expression>, LanguageError> {
                Ok(Box::new(DummyExpression))
            }

            fn create_predicate(&self, _script: &str) -> Result<Box<dyn Predicate>, LanguageError> {
                Ok(Box::new(DummyPredicate))
            }
        }

        let mut ctx = CamelContext::new();
        ctx.register_language("runtime", Box::new(RuntimeLang))
            .unwrap();

        let definition = RouteDefinition::new(
            "timer:tick",
            vec![BuilderStep::DeclarativeScript {
                expression: LanguageExpressionDef {
                    language: "runtime".into(),
                    source: "${body}".into(),
                },
            }],
        )
        .with_route_id("runtime-lang-route");

        let result = ctx.add_route_definition(definition);
        assert!(
            result.is_ok(),
            "route should resolve runtime language: {result:?}"
        );
    }

    #[test]
    fn test_add_route_definition_fails_for_unregistered_runtime_language() {
        let mut ctx = CamelContext::new();
        let definition = RouteDefinition::new(
            "timer:tick",
            vec![BuilderStep::DeclarativeSetBody {
                value: crate::route::ValueSourceDef::Expression(LanguageExpressionDef {
                    language: "missing-lang".into(),
                    source: "${body}".into(),
                }),
            }],
        )
        .with_route_id("missing-runtime-lang-route");

        let result = ctx.add_route_definition(definition);
        assert!(
            result.is_err(),
            "route should fail when language is missing"
        );
        let error_text = result.unwrap_err().to_string();
        assert!(
            error_text.contains("missing-lang"),
            "error should mention missing language, got: {error_text}"
        );
    }

    #[test]
    fn test_health_check_empty_context() {
        let ctx = CamelContext::new();
        let report = ctx.health_check();

        assert_eq!(report.status, HealthStatus::Healthy);
        assert!(report.services.is_empty());
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use async_trait::async_trait;
    use camel_api::Lifecycle;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockService {
        start_count: Arc<AtomicUsize>,
        stop_count: Arc<AtomicUsize>,
    }

    impl MockService {
        fn new() -> (Self, Arc<AtomicUsize>, Arc<AtomicUsize>) {
            let start_count = Arc::new(AtomicUsize::new(0));
            let stop_count = Arc::new(AtomicUsize::new(0));
            (
                Self {
                    start_count: start_count.clone(),
                    stop_count: stop_count.clone(),
                },
                start_count,
                stop_count,
            )
        }
    }

    #[async_trait]
    impl Lifecycle for MockService {
        fn name(&self) -> &str {
            "mock"
        }

        async fn start(&mut self) -> Result<(), CamelError> {
            self.start_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn stop(&mut self) -> Result<(), CamelError> {
            self.stop_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_context_starts_lifecycle_services() {
        let (service, start_count, stop_count) = MockService::new();

        let mut ctx = CamelContext::new().with_lifecycle(service);

        assert_eq!(start_count.load(Ordering::SeqCst), 0);

        ctx.start().await.unwrap();

        assert_eq!(start_count.load(Ordering::SeqCst), 1);
        assert_eq!(stop_count.load(Ordering::SeqCst), 0);

        ctx.stop().await.unwrap();

        assert_eq!(stop_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_service_start_failure_rollback() {
        struct FailingService {
            start_count: Arc<AtomicUsize>,
            stop_count: Arc<AtomicUsize>,
            should_fail: bool,
        }

        #[async_trait]
        impl Lifecycle for FailingService {
            fn name(&self) -> &str {
                "failing"
            }

            async fn start(&mut self) -> Result<(), CamelError> {
                self.start_count.fetch_add(1, Ordering::SeqCst);
                if self.should_fail {
                    Err(CamelError::ProcessorError("intentional failure".into()))
                } else {
                    Ok(())
                }
            }

            async fn stop(&mut self) -> Result<(), CamelError> {
                self.stop_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let start1 = Arc::new(AtomicUsize::new(0));
        let stop1 = Arc::new(AtomicUsize::new(0));
        let start2 = Arc::new(AtomicUsize::new(0));
        let stop2 = Arc::new(AtomicUsize::new(0));
        let start3 = Arc::new(AtomicUsize::new(0));
        let stop3 = Arc::new(AtomicUsize::new(0));

        let service1 = FailingService {
            start_count: start1.clone(),
            stop_count: stop1.clone(),
            should_fail: false,
        };
        let service2 = FailingService {
            start_count: start2.clone(),
            stop_count: stop2.clone(),
            should_fail: true, // This one will fail
        };
        let service3 = FailingService {
            start_count: start3.clone(),
            stop_count: stop3.clone(),
            should_fail: false,
        };

        let mut ctx = CamelContext::new()
            .with_lifecycle(service1)
            .with_lifecycle(service2)
            .with_lifecycle(service3);

        // Attempt to start - should fail
        let result = ctx.start().await;
        assert!(result.is_err());

        // Verify service1 was started and then stopped (rollback)
        assert_eq!(start1.load(Ordering::SeqCst), 1);
        assert_eq!(stop1.load(Ordering::SeqCst), 1);

        // Verify service2 was attempted to start but failed
        assert_eq!(start2.load(Ordering::SeqCst), 1);
        assert_eq!(stop2.load(Ordering::SeqCst), 0);

        // Verify service3 was never started
        assert_eq!(start3.load(Ordering::SeqCst), 0);
        assert_eq!(stop3.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_services_stop_in_reverse_order() {
        use std::sync::Mutex as StdMutex;

        struct OrderTracker {
            name: String,
            order: Arc<StdMutex<Vec<String>>>,
        }

        #[async_trait]
        impl Lifecycle for OrderTracker {
            fn name(&self) -> &str {
                &self.name
            }

            async fn start(&mut self) -> Result<(), CamelError> {
                Ok(())
            }

            async fn stop(&mut self) -> Result<(), CamelError> {
                self.order.lock().unwrap().push(self.name.clone());
                Ok(())
            }
        }

        let order = Arc::new(StdMutex::new(Vec::<String>::new()));

        let s1 = OrderTracker {
            name: "first".into(),
            order: Arc::clone(&order),
        };
        let s2 = OrderTracker {
            name: "second".into(),
            order: Arc::clone(&order),
        };
        let s3 = OrderTracker {
            name: "third".into(),
            order: Arc::clone(&order),
        };

        let mut ctx = CamelContext::new()
            .with_lifecycle(s1)
            .with_lifecycle(s2)
            .with_lifecycle(s3);

        ctx.start().await.unwrap();
        ctx.stop().await.unwrap();

        let stopped = order.lock().unwrap();
        assert_eq!(
            *stopped,
            vec!["third", "second", "first"],
            "services must stop in reverse insertion order"
        );
    }
}

#[cfg(test)]
mod config_registry_tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct MyConfig {
        value: u32,
    }

    #[test]
    fn test_set_and_get_component_config() {
        let mut ctx = CamelContext::new();
        ctx.set_component_config(MyConfig { value: 42 });
        let got = ctx.get_component_config::<MyConfig>();
        assert_eq!(got, Some(&MyConfig { value: 42 }));
    }

    #[test]
    fn test_get_missing_config_returns_none() {
        let ctx = CamelContext::new();
        assert!(ctx.get_component_config::<MyConfig>().is_none());
    }

    #[test]
    fn test_set_overwrites_previous_config() {
        let mut ctx = CamelContext::new();
        ctx.set_component_config(MyConfig { value: 1 });
        ctx.set_component_config(MyConfig { value: 2 });
        assert_eq!(ctx.get_component_config::<MyConfig>().unwrap().value, 2);
    }
}
