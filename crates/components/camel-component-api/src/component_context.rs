use std::sync::Arc;

use camel_api::{AsyncHealthCheck, MetricsCollector, PlatformService};
use camel_language_api::Language;

use crate::Component;

/// Runtime context passed to components during endpoint creation.
pub trait ComponentContext: Send + Sync {
    /// Resolve a component by scheme.
    fn resolve_component(&self, scheme: &str) -> Option<Arc<dyn Component>>;

    /// Resolve a language by name.
    fn resolve_language(&self, name: &str) -> Option<Arc<dyn Language>>;

    /// Access the active metrics collector.
    fn metrics(&self) -> Arc<dyn MetricsCollector>;

    /// Access the active health-check registry.
    ///
    /// Used by component code paths that need to pin a route Unhealthy
    /// (category (g) per ADR-0012). Default: NoOp — tests/examples inherit
    /// the no-op. Concrete runtimes (CamelContext) override to return the
    /// real registry.
    fn health(&self) -> Arc<dyn crate::HealthCheckRegistry> {
        Arc::new(crate::NoOpHealthCheckRegistry)
    }

    /// Access the active platform service.
    fn platform_service(&self) -> Arc<dyn PlatformService>;

    fn register_route_health_check(&self, route_id: &str, check: Arc<dyn AsyncHealthCheck>);

    fn unregister_route_health_check(&self, route_id: &str);

    fn route_id(&self) -> Option<&str> {
        None
    }

    fn register_current_route_health_check(&self, check: Arc<dyn AsyncHealthCheck>) {
        if let Some(id) = self.route_id() {
            self.register_route_health_check(id, check);
        }
    }
}

/// Default no-op component context for tests/examples.
pub struct NoOpComponentContext;

impl ComponentContext for NoOpComponentContext {
    fn resolve_component(&self, _scheme: &str) -> Option<Arc<dyn Component>> {
        None
    }

    fn resolve_language(&self, _name: &str) -> Option<Arc<dyn Language>> {
        None
    }

    fn metrics(&self) -> Arc<dyn MetricsCollector> {
        Arc::new(camel_api::NoOpMetrics)
    }

    fn platform_service(&self) -> Arc<dyn PlatformService> {
        Arc::new(camel_api::NoopPlatformService::default())
    }

    fn register_route_health_check(&self, _route_id: &str, _check: Arc<dyn AsyncHealthCheck>) {}

    fn unregister_route_health_check(&self, _route_id: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_context_health_default_is_noop() {
        let ctx = NoOpComponentContext;
        let h = ctx.health();
        // Must not panic.
        h.force_unhealthy_for_route("any", "any", "any");
    }
}
