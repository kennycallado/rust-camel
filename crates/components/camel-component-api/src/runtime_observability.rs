//! `RuntimeObservability` — narrow trait exposing `metrics()` and `health()`
//! to Endpoint consumers/producers. Defined per ADR-0012 §Signal-replacement-API
//! and the Phase A closure spec
//! (`docs/superpowers/specs/2026-06-05-adr-0012-closure-design.md`).
//!
//! **Why a separate trait:** `ComponentContext` carries `resolve_component`,
//! `resolve_language`, `platform_service` — far more authority than an
//! emitter needs. Passing `Arc<dyn ComponentContext>` would be a
//! service-locator anti-pattern inviting future misuse. This trait exposes
//! exactly the two observability surfaces an ADR-0012 emitter calls.

use std::sync::Arc;

use camel_api::MetricsCollector;

use crate::{ComponentContext, HealthCheckRegistry};

/// Narrow observability surface available to Endpoint consumers and producers.
///
/// Implemented blanket for every `T: ComponentContext`, so
/// `CamelContext` (the spec's "DefaultRuntime") and any test ctx
/// automatically satisfy this trait. Endpoints receive
/// `Arc<dyn RuntimeObservability>` at `create_consumer` / `create_producer`
/// time (per Phase A closure spec).
///
/// `Send + Sync` is mandatory — Endpoints spawn across tokio tasks.
pub trait RuntimeObservability: Send + Sync {
    /// Active metrics collector. Used for `increment_errors(route_id, label)`
    /// per ADR-0012 categories (b′) and (e).
    fn metrics(&self) -> Arc<dyn MetricsCollector>;

    /// Active health-check registry. Used for
    /// `force_unhealthy_for_route(route_id, name, reason)` per ADR-0012
    /// category (g).
    fn health(&self) -> Arc<dyn HealthCheckRegistry>;
}

// Blanket impl: every ComponentContext is automatically a RuntimeObservability.
// This covers `CamelContext` (production), `NoOpComponentContext` (tests),
// and any future impl.
//
// No `?Sized` bound: ComponentContext itself requires Sized (the trait has
// methods returning `Arc<dyn …>` which need a concrete owner type). If a
// future dynamically-sized ctx is needed, add a separate explicit impl.
impl<T: ComponentContext> RuntimeObservability for T {
    fn metrics(&self) -> Arc<dyn MetricsCollector> {
        <Self as ComponentContext>::metrics(self)
    }

    fn health(&self) -> Arc<dyn HealthCheckRegistry> {
        <Self as ComponentContext>::health(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_observability_is_send_sync() {
        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        assert_send_sync::<dyn RuntimeObservability>();
    }
}
