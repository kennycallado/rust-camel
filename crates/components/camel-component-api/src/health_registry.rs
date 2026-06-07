//! Narrow `HealthCheckRegistry` trait exposed to components via
//! `RuntimeObservability::health()`.
//!
//! This is intentionally NOT the full registry surface — components only need
//! `force_unhealthy_for_route` (category (g) per ADR-0012). Register/unregister
//! stays on `ComponentContext` (camel-component-api) and the concrete
//! `HealthCheckRegistry` struct (camel-core) for runtime-internal callers.

use std::sync::Arc;

/// Narrow view of the health registry exposed to components via
/// [`crate::RuntimeObservability::health`].
///
/// Category (g) endpoint/producer creation failures call
/// `force_unhealthy_for_route(route_id, "endpoint-creation", reason)` to pin
/// the pod NotReady (HTTP 503). Supervision restart (ADR-0007) clears the pin
/// via `register_for_route` once the endpoint is recreated.
///
/// There is no `force_degraded_for_route` method and we explicitly do NOT add
/// one — a half-functional route is worse than a removed-from-rotation one.
pub trait HealthCheckRegistry: Send + Sync {
    /// Pin the given route to Unhealthy. Replaces any existing checks for
    /// the route with a single ForcedUnhealthy entry.
    ///
    /// `name`: a stable identifier (e.g. "endpoint-creation", "pool-init").
    /// `reason`: human-readable detail that will appear in the health report.
    fn force_unhealthy_for_route(&self, route_id: &str, name: &str, reason: &str);
}

/// No-op implementation for tests and examples. All methods are silent no-ops.
#[derive(Debug, Default, Clone)]
pub struct NoOpHealthCheckRegistry;

impl HealthCheckRegistry for NoOpHealthCheckRegistry {
    fn force_unhealthy_for_route(&self, _route_id: &str, _name: &str, _reason: &str) {
        // no-op — test/example only
    }
}

/// Convenience constructor.
pub fn noop_health_check_registry() -> Arc<dyn HealthCheckRegistry> {
    Arc::new(NoOpHealthCheckRegistry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_force_unhealthy_does_not_panic() {
        let reg = NoOpHealthCheckRegistry;
        reg.force_unhealthy_for_route("any-route", "any-name", "any reason");
        // Test passes if no panic.
    }

    #[test]
    fn noop_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NoOpHealthCheckRegistry>();
    }

    #[test]
    fn noop_health_check_registry_returns_arc() {
        let reg: Arc<dyn HealthCheckRegistry> = noop_health_check_registry();
        // Construct and confirm Arc coercion works (would fail to compile if signature wrong).
        // Then exercise the no-op:
        reg.force_unhealthy_for_route("route-1", "init", "test reason");
        // No panic = pass.
    }
}
