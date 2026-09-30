use std::sync::Arc;

use camel_api::{AsyncHealthCheck, InFlightGauge, MetricsCollector, PlatformService};
use camel_language_api::Language;
use tokio_util::sync::CancellationToken;

use crate::Component;

/// Runtime context passed to components during endpoint creation.
pub trait ComponentContext: Send + Sync {
    /// Resolve a component by scheme.
    fn resolve_component(&self, scheme: &str) -> Option<Arc<dyn Component>>;

    /// Resolve a language by name.
    fn resolve_language(&self, name: &str) -> Option<Arc<dyn Language>>;

    /// Access the active metrics collector.
    fn metrics(&self) -> Arc<dyn MetricsCollector>;

    /// Context-global gauge of accepted-not-completed exchanges
    /// (drainclaim). Production contexts return the gauge installed on
    /// every `ConsumerContext` at consumer start and read by
    /// `CamelContext::total_in_flight()` for the drain verdict. Default
    /// `None` keeps test contexts uncounted.
    fn in_flight_counter(&self) -> Option<Arc<InFlightGauge>> {
        None
    }

    /// Snapshot of the `[observability.metrics].components` lever —
    /// gates only the uniform component-operations family served through
    /// `RuntimeObservability::component_metrics()`; error-family
    /// emission is never lever-gated. Default false (opt-in);
    /// `CamelContext` overrides this with its `MetricsLeversConfig`
    /// snapshot.
    fn component_metrics_enabled(&self) -> bool {
        false
    }

    /// Access the active health-check registry.
    ///
    /// Used by component code paths that need to pin a route Unhealthy
    /// (category (g) per ADR-0012). Default: NoOp — tests/examples inherit
    /// the no-op. Concrete runtimes (CamelContext) override to return the
    /// real registry.
    fn health(&self) -> Arc<dyn crate::HealthCheckRegistry> {
        Arc::new(crate::NoOpHealthCheckRegistry)
    }

    /// Clone of the Runtime-owned shutdown token when this context is
    /// bound to one. Producer-side and processor-side code (which has no
    /// `ConsumerContext`) uses it to observe Runtime shutdown.
    /// `CamelContext` overrides this with its `shutdown_token()`. Default
    /// `None` keeps unbound contexts (tests, examples) out of the
    /// shutdown lineage, so callers mint a local root token instead.
    /// Callers resolve this per call: CamelContext replaces its shutdown
    /// token on every start, so a token captured once goes stale across
    /// stop/start.
    ///
    /// # Binding-time boundary
    ///
    /// Token lineage binds at component REGISTRATION time. Production
    /// registration sites construct slot-bound contexts
    /// (`RegistryComponentContext::with_shutdown_slot`, re-written by
    /// every `CamelContext::start`) and hand them to long-lived components
    /// (for example `WasmComponent`'s captured `Arc<dyn ComponentContext>`).
    /// Endpoint-creation-time adapter contexts
    /// (`ControllerComponentContext`, `MasterDelegateContext`) must NOT
    /// snapshot tokens; they keep the `None` default so no stale-across
    /// stop/start lineage can leak, and callers mint a local root instead.
    fn shutdown_token(&self) -> Option<CancellationToken> {
        None
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

    #[test]
    fn shutdown_token_default_is_none() {
        let ctx = NoOpComponentContext;
        assert!(
            ctx.shutdown_token().is_none(),
            "unbound contexts must return None so callers mint a local root"
        );
    }
}
