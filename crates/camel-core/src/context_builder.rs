use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use camel_api::{
    CamelError, FunctionInvoker, Lifecycle, MetricsCollector, NoOpMetrics, NoopPlatformService,
    PlatformService, SupervisionConfig,
};
use camel_language_api::Language;

use super::context::{CamelContext, FromParts};
use crate::health_registry::HealthCheckRegistry;
use crate::lifecycle::adapters::RuntimeExecutionAdapter;
use crate::lifecycle::adapters::controller_actor::{
    RouteControllerHandle, spawn_controller_actor, spawn_supervision_task,
};
use crate::lifecycle::adapters::route_controller::{
    DefaultRouteController, SharedLanguageRegistry,
};
use crate::lifecycle::application::runtime_bus::RuntimeBus;
use crate::lifecycle::ports::RuntimeExecutionPort;
use crate::shared::components::domain::Registry;
use crate::template::TemplateRegistry;

type ExecutionFactory =
    Arc<dyn Fn(RouteControllerHandle) -> Arc<dyn RuntimeExecutionPort> + Send + Sync>;

pub struct CamelContextBuilder {
    registry: Option<Arc<std::sync::Mutex<Registry>>>,
    languages: Option<SharedLanguageRegistry>,
    metrics: Option<Arc<dyn MetricsCollector>>,
    // Platform ports
    platform_service: Option<Arc<dyn PlatformService>>,
    supervision_config: Option<SupervisionConfig>,
    runtime_store: Option<crate::lifecycle::adapters::InMemoryRuntimeStore>,
    shutdown_timeout: std::time::Duration,
    beans: Option<Arc<std::sync::Mutex<camel_bean::BeanRegistry>>>,
    function_invoker: Option<Arc<dyn FunctionInvoker>>,
    lifecycle_services: Vec<Box<dyn Lifecycle>>,
    execution_factory: Option<ExecutionFactory>,
    health_registry: Option<Arc<HealthCheckRegistry>>,
    template_registry: Option<Arc<TemplateRegistry>>,
}

impl CamelContextBuilder {
    pub fn new() -> Self {
        Self {
            registry: None,
            languages: None,
            metrics: None,
            platform_service: None,
            supervision_config: None,
            runtime_store: None,
            shutdown_timeout: std::time::Duration::from_secs(5),
            beans: None,
            function_invoker: None,
            lifecycle_services: Vec::new(),
            execution_factory: None,
            health_registry: None,
            template_registry: None,
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

    pub fn with_execution_factory(
        mut self,
        factory: impl Fn(RouteControllerHandle) -> Arc<dyn RuntimeExecutionPort> + Send + Sync + 'static,
    ) -> Self {
        self.execution_factory = Some(Arc::new(factory));
        self
    }

    pub fn metrics(mut self, metrics: Arc<dyn MetricsCollector>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Set a custom platform service.
    pub fn platform_service(mut self, platform_service: Arc<dyn PlatformService>) -> Self {
        self.platform_service = Some(platform_service);
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

    pub fn health_registry(mut self, registry: Arc<HealthCheckRegistry>) -> Self {
        self.health_registry = Some(registry);
        self
    }

    /// Inject a shared `BeanRegistry` for bean resolution across routes.
    pub fn beans(mut self, beans: Arc<std::sync::Mutex<camel_bean::BeanRegistry>>) -> Self {
        self.beans = Some(beans);
        self
    }

    /// Register a lifecycle service (e.g., FunctionRuntimeService) at builder time.
    ///
    /// This is the recommended path for services that need to be wired into the
    /// route controller before any routes are added. The function invoker (if any)
    /// is extracted and passed to the `DefaultRouteController` during `build()`.
    pub fn with_lifecycle<L: Lifecycle + 'static>(mut self, service: L) -> Self {
        if let Some(collector) = service.as_metrics_collector() {
            self.metrics = Some(collector);
        }
        if let Some(invoker) = service.as_function_invoker() {
            self.function_invoker = Some(invoker);
        }
        self.lifecycle_services.push(Box::new(service));
        self
    }

    /// Set a custom `TemplateRegistry` for route template storage.
    ///
    /// If not provided, a default empty registry is created during `build()`.
    pub fn template_registry(mut self, registry: Arc<TemplateRegistry>) -> Self {
        self.template_registry = Some(registry);
        self
    }

    fn built_in_languages() -> SharedLanguageRegistry {
        let mut languages: HashMap<String, Arc<dyn Language>> = HashMap::new();
        languages.insert(
            "simple".to_string(),
            Arc::new(camel_language_simple::SimpleLanguage::new()),
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
                Arc::new(camel_language_jsonpath::JsonPathLanguage::new()),
            );
        }
        #[cfg(feature = "lang-xpath")]
        {
            languages.insert(
                "xpath".to_string(),
                Arc::new(camel_language_xpath::XPathLanguage::new()),
            );
        }
        Arc::new(std::sync::Mutex::new(languages))
    }

    fn build_runtime(
        controller: RouteControllerHandle,
        store: crate::lifecycle::adapters::InMemoryRuntimeStore,
        execution_factory: Option<ExecutionFactory>,
        health_registry: Arc<HealthCheckRegistry>,
    ) -> Arc<RuntimeBus> {
        let execution: Arc<dyn RuntimeExecutionPort> = if let Some(factory) = execution_factory {
            factory(controller.clone())
        } else {
            Arc::new(RuntimeExecutionAdapter::new(controller))
        };
        Arc::new(
            RuntimeBus::new(
                Arc::new(store.clone()),
                Arc::new(store.clone()),
                Arc::new(store.clone()),
                Arc::new(store.clone()),
            )
            .with_uow(Arc::new(store))
            .with_execution(execution)
            .with_health_registry(health_registry),
        )
    }

    pub async fn build(self) -> Result<CamelContext, CamelError> {
        let registry = self
            .registry
            .unwrap_or_else(|| Arc::new(std::sync::Mutex::new(Registry::new())));
        let languages = self.languages.unwrap_or_else(Self::built_in_languages);
        let simple_with_resolver: Arc<dyn Language> = Arc::new(
            camel_language_simple::SimpleLanguage::with_resolver(Arc::new({
                let languages = Arc::clone(&languages);
                move |name| {
                    languages
                        .lock()
                        .ok()
                        .and_then(|registry| registry.get(name).cloned())
                }
            })),
        );
        languages
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock") // allow-unwrap
            .insert("simple".to_string(), simple_with_resolver);
        let metrics = self.metrics.unwrap_or_else(|| Arc::new(NoOpMetrics));
        let platform_service = self
            .platform_service
            .unwrap_or_else(|| Arc::new(NoopPlatformService::default()));
        let health_registry = self.health_registry.unwrap_or_else(|| {
            Arc::new(HealthCheckRegistry::new(std::time::Duration::from_secs(5)))
        });

        let (controller, actor_join, supervision_join) =
            if let Some(config) = self.supervision_config {
                let (crash_tx, crash_rx) = tokio::sync::mpsc::channel(64);
                let mut controller_impl = if let Some(ref beans) = self.beans {
                    DefaultRouteController::with_languages_and_beans(
                        Arc::clone(&registry),
                        Arc::clone(&languages),
                        Arc::clone(&platform_service),
                        Arc::clone(beans),
                    )
                } else {
                    DefaultRouteController::with_languages(
                        Arc::clone(&registry),
                        Arc::clone(&languages),
                        Arc::clone(&platform_service),
                    )
                };
                if let Some(invoker) = self.function_invoker.clone() {
                    controller_impl = controller_impl.with_function_invoker(invoker);
                }
                controller_impl.set_health_registry(Arc::clone(&health_registry));
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
                let mut controller_impl = if let Some(ref beans) = self.beans {
                    DefaultRouteController::with_languages_and_beans(
                        Arc::clone(&registry),
                        Arc::clone(&languages),
                        Arc::clone(&platform_service),
                        Arc::clone(beans),
                    )
                } else {
                    DefaultRouteController::with_languages(
                        Arc::clone(&registry),
                        Arc::clone(&languages),
                        Arc::clone(&platform_service),
                    )
                };
                if let Some(invoker) = self.function_invoker.clone() {
                    controller_impl = controller_impl.with_function_invoker(invoker);
                }
                controller_impl.set_health_registry(Arc::clone(&health_registry));
                let (controller, actor_join) = spawn_controller_actor(controller_impl);
                (controller, actor_join, None)
            };

        let store = self.runtime_store.unwrap_or_default();
        let runtime = Self::build_runtime(
            controller.clone(),
            store,
            self.execution_factory,
            Arc::clone(&health_registry),
        );
        let runtime_handle: Arc<dyn camel_api::RuntimeHandle> = runtime.clone();
        controller
            .try_set_runtime_handle(runtime_handle)
            .expect("controller actor mailbox should accept initial runtime handle"); // allow-unwrap

        let template_registry = self
            .template_registry
            .unwrap_or_else(|| Arc::new(TemplateRegistry::new()));

        Ok(CamelContext::from_parts(FromParts {
            registry,
            route_controller: controller,
            _actor_join: actor_join,
            supervision_join,
            runtime,
            cancel_token: CancellationToken::new(),
            metrics,
            platform_service,
            languages,
            shutdown_timeout: self.shutdown_timeout,
            services: self.lifecycle_services,
            health_registry,
            component_configs: HashMap::new(),
            function_invoker: self.function_invoker,
            template_registry,
        }))
    }
}

impl Default for CamelContextBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::FromParts;

    #[test]
    fn builder_default_has_sane_timeout() {
        let builder = CamelContextBuilder::new();
        assert_eq!(builder.shutdown_timeout, std::time::Duration::from_secs(5));
    }
}
