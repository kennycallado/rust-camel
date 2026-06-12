//! ControllerComponentContext — a thin [`ComponentContext`] implementation used
//! inside [`DefaultRouteController`](super::route_controller::DefaultRouteController)
//! for endpoint resolution and observability during route compilation and startup.

use std::sync::Arc;

use camel_api::metrics::MetricsCollector;
use camel_api::{AsyncHealthCheck, PlatformService};
use camel_component_api::ComponentContext;
use camel_processor::aggregator::SharedLanguageRegistry;

use crate::health_registry::HealthCheckRegistry;
use crate::shared::components::domain::Registry;

/// A [`ComponentContext`] that delegates to the controller's registries.
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

    fn register_route_health_check(&self, route_id: &str, check: Arc<dyn AsyncHealthCheck>) {
        self.health_registry.register_for_route(route_id, check);
    }

    fn unregister_route_health_check(&self, route_id: &str) {
        self.health_registry.unregister_for_route(route_id);
    }

    fn route_id(&self) -> Option<&str> {
        self.route_id.as_deref()
    }
}
