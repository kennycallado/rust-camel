//! Adapters layer of the health_registry vertical slice.
//!
//! This module hosts the external trait delegation: `RuntimeExecutionHandle`
//! and other consumers reach `HealthCheckRegistry` through the
//! `camel_component_api::HealthCheckRegistry` port. The impl here is a thin
//! forward to the inherent entity method, which Rust's method resolution
//! prefers over the trait method (no recursion).

use super::domain::HealthCheckRegistry;

impl camel_component_api::HealthCheckRegistry for HealthCheckRegistry {
    fn force_unhealthy_for_route(&self, route_id: &str, name: &str, reason: &str) {
        // Rust's method resolution prefers inherent methods over trait methods
        // when both are in scope with the same name. This calls the inherent
        // method (defined on HealthCheckRegistry in domain), NOT this trait
        // method we're defining here — so there is no recursion.
        self.force_unhealthy_for_route(route_id, name, reason.to_string());
    }
}
