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

use camel_api::{ComponentMetrics, InFlightGauge, MetricsCollector};

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

    /// Facade for the uniform component-operations family
    /// (`camel_component_operations_total{component,operation,outcome}`,
    /// dashboard-observability Task 4.1), scoped to `component` so the
    /// handle carries both the identity and the resolved switch.
    ///
    /// The per-component lever snapshot is baked in at facade
    /// construction: the component series flow only when
    /// `component_metric_levers().enabled_for(component)` is true, while
    /// failures always reach the non-disableable error family — error
    /// emission is never lever-gated.
    ///
    /// Default: lever off — impls that do not override
    /// `component_metric_levers()` (manual test runtimes AND any context
    /// wired without a levers snapshot) keep the component series
    /// suppressed and only forward errors. In production the blanket impl
    /// resolves the context's `MetricsLeversConfig` snapshot per
    /// component.
    fn component_metrics(&self, component: &str) -> ComponentMetrics {
        ComponentMetrics::new(self.metrics(), component, false)
    }

    /// Context-global gauge of accepted-not-completed exchanges
    /// (drainclaim), forwarded from [`ComponentContext::in_flight_counter`].
    /// Default `None` keeps runtimes that install no gauge uncounted.
    fn in_flight_counter(&self) -> Option<Arc<InFlightGauge>> {
        None
    }
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

    fn component_metrics(&self, component: &str) -> ComponentMetrics {
        let enabled =
            <Self as ComponentContext>::component_metric_levers(self).enabled_for(component);
        ComponentMetrics::new(
            <Self as ComponentContext>::metrics(self),
            component,
            enabled,
        )
    }

    fn in_flight_counter(&self) -> Option<Arc<InFlightGauge>> {
        <Self as ComponentContext>::in_flight_counter(self)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use camel_api::{ComponentMetricsLevers, MetricsCollector, PlatformService};
    use camel_language_api::Language;

    use super::*;
    use crate::{Component, NoOpComponentContext};

    /// `MetricsCollector` capturing both emission families.
    #[derive(Default)]
    struct RecordingCollector {
        errors: Mutex<Vec<(String, String)>>,
        ops: Mutex<Vec<(String, String, String)>>,
    }

    impl RecordingCollector {
        fn errors(&self) -> Vec<(String, String)> {
            self.errors.lock().expect("errors lock").clone()
        }

        fn ops(&self) -> Vec<(String, String, String)> {
            self.ops.lock().expect("ops lock").clone()
        }
    }

    impl MetricsCollector for RecordingCollector {
        fn record_exchange_duration(&self, _: &str, _: Duration) {}
        fn increment_errors(&self, component: &str, error_type: &str) {
            self.errors
                .lock()
                .expect("errors lock")
                .push((component.to_string(), error_type.to_string()));
        }
        fn increment_exchanges(&self, _: &str) {}
        fn set_queue_depth(&self, _: &str, _: usize) {}
        fn record_circuit_breaker_change(&self, _: &str, _: &str, _: &str) {}
        fn record_component_operation(&self, component: &str, operation: &str, outcome: &str) {
            self.ops.lock().expect("ops lock").push((
                component.to_string(),
                operation.to_string(),
                outcome.to_string(),
            ));
        }
    }

    /// Test context whose levers enable only `wasm`.
    struct ScopedLeverContext {
        inner: NoOpComponentContext,
        collector: Arc<RecordingCollector>,
    }

    impl ComponentContext for ScopedLeverContext {
        fn resolve_component(&self, scheme: &str) -> Option<Arc<dyn Component>> {
            self.inner.resolve_component(scheme)
        }

        fn resolve_language(&self, name: &str) -> Option<Arc<dyn Language>> {
            self.inner.resolve_language(name)
        }

        fn metrics(&self) -> Arc<dyn MetricsCollector> {
            Arc::clone(&self.collector) as Arc<dyn MetricsCollector>
        }

        fn component_metric_levers(&self) -> ComponentMetricsLevers {
            ComponentMetricsLevers::new(true, false, BTreeMap::from([("wasm".to_string(), true)]))
        }

        fn platform_service(&self) -> Arc<dyn PlatformService> {
            self.inner.platform_service()
        }

        fn register_route_health_check(
            &self,
            route_id: &str,
            check: Arc<dyn camel_api::AsyncHealthCheck>,
        ) {
            self.inner.register_route_health_check(route_id, check);
        }

        fn unregister_route_health_check(&self, route_id: &str) {
            self.inner.unregister_route_health_check(route_id);
        }
    }

    #[test]
    fn runtime_observability_is_send_sync() {
        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        assert_send_sync::<dyn RuntimeObservability>();
    }

    #[test]
    fn blanket_impl_uses_context_levers() {
        let collector = Arc::new(RecordingCollector::default());
        let ctx = ScopedLeverContext {
            inner: NoOpComponentContext,
            collector: Arc::clone(&collector),
        };

        RuntimeObservability::component_metrics(&ctx, "wasm").observe("invoke", false);
        assert_eq!(
            collector.ops(),
            vec![(
                "wasm".to_string(),
                "invoke".to_string(),
                "success".to_string()
            )],
            "the override must enable the wasm component series"
        );

        RuntimeObservability::component_metrics(&ctx, "kafka").observe("consume", true);
        assert_eq!(
            collector.ops().len(),
            1,
            "kafka inherits components=false, so no operation is recorded"
        );
        assert_eq!(
            collector.errors(),
            vec![("kafka".to_string(), "e:kafka:consume".to_string())],
            "errors must flow regardless of the per-component lever"
        );
    }
}
