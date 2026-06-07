//! Phase A trait suite for `RuntimeObservability` + Endpoint signature change
//! (ADR-0012 closure, per spec lines 90–98).
//!
//! Five tests:
//! 1. `runtime_observability_metrics_routes_to_collector` — increment_errors arrives.
//! 2. `runtime_observability_health_routes_to_registry` — force_unhealthy_for_route arrives.
//! 3. `endpoint_consumer_receives_runtime` — fixture verifies create_consumer gets the Arc.
//! 4. `endpoint_producer_receives_runtime` — fixture verifies create_producer gets the Arc.
//! 5. `runtime_observability_send_sync` — Send + Sync static assertion.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::{BoxProcessor, BoxProcessorExt, CamelError, Exchange, MetricsCollector};
use camel_component_api::{
    ComponentContext, Consumer, ConsumerContext, Endpoint, HealthCheckRegistry,
    NoOpComponentContext, ProducerContext, RuntimeObservability,
};

// ============================================================
// Test infra: recording collector + recording registry + ctx
// ============================================================

#[derive(Debug, Default)]
struct RecordingMetrics {
    errors: Mutex<Vec<(String, String)>>,
}

impl MetricsCollector for RecordingMetrics {
    fn record_exchange_duration(&self, _: &str, _: Duration) {}
    fn increment_errors(&self, route_id: &str, error_type: &str) {
        self.errors
            .lock()
            .expect("metrics lock")
            .push((route_id.to_string(), error_type.to_string()));
    }
    fn increment_exchanges(&self, _: &str) {}
    fn set_queue_depth(&self, _: &str, _: usize) {}
    fn record_circuit_breaker_change(&self, _: &str, _: &str, _: &str) {}
}

#[derive(Debug, Default)]
struct RecordingRegistry {
    forced: Mutex<Vec<(String, String, String)>>,
}

impl HealthCheckRegistry for RecordingRegistry {
    fn force_unhealthy_for_route(&self, route_id: &str, name: &str, reason: &str) {
        self.forced.lock().expect("registry lock").push((
            route_id.to_string(),
            name.to_string(),
            reason.to_string(),
        ));
    }
}

/// Test ctx that returns the recording collector + registry.
struct RecordingCtx {
    metrics: Arc<RecordingMetrics>,
    health: Arc<RecordingRegistry>,
}

impl RecordingCtx {
    fn new() -> (Self, Arc<RecordingMetrics>, Arc<RecordingRegistry>) {
        let metrics = Arc::new(RecordingMetrics::default());
        let health = Arc::new(RecordingRegistry::default());
        let ctx = Self {
            metrics: Arc::clone(&metrics),
            health: Arc::clone(&health),
        };
        (ctx, metrics, health)
    }
}

impl ComponentContext for RecordingCtx {
    fn resolve_component(&self, _: &str) -> Option<Arc<dyn camel_component_api::Component>> {
        None
    }
    fn resolve_language(&self, _: &str) -> Option<Arc<dyn camel_language_api::Language>> {
        None
    }
    fn metrics(&self) -> Arc<dyn MetricsCollector> {
        Arc::clone(&self.metrics) as Arc<dyn MetricsCollector>
    }
    fn platform_service(&self) -> Arc<dyn camel_api::PlatformService> {
        Arc::new(camel_api::NoopPlatformService::default())
    }
    fn register_route_health_check(&self, _: &str, _: Arc<dyn camel_api::AsyncHealthCheck>) {}
    fn unregister_route_health_check(&self, _: &str) {}
    fn health(&self) -> Arc<dyn HealthCheckRegistry> {
        Arc::clone(&self.health) as Arc<dyn HealthCheckRegistry>
    }
}

// ============================================================
// Test 1: metrics routing
// ============================================================

#[test]
fn runtime_observability_metrics_routes_to_collector() {
    let (ctx, metrics_arc, _health_arc) = RecordingCtx::new();
    let rt: Arc<dyn RuntimeObservability> = Arc::new(ctx);
    rt.metrics()
        .increment_errors("route-x", "b-prime:sql:on-consume");

    let recorded = metrics_arc.errors.lock().expect("metrics lock").clone();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].0, "route-x");
    assert_eq!(recorded[0].1, "b-prime:sql:on-consume");
}

// ============================================================
// Test 2: health routing
// ============================================================

#[test]
fn runtime_observability_health_routes_to_registry() {
    let (ctx, _metrics_arc, health_arc) = RecordingCtx::new();
    let rt: Arc<dyn RuntimeObservability> = Arc::new(ctx);
    rt.health()
        .force_unhealthy_for_route("route-y", "endpoint-creation", "nope");

    let recorded = health_arc.forced.lock().expect("registry lock").clone();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].0, "route-y");
    assert_eq!(recorded[0].1, "endpoint-creation");
    assert_eq!(recorded[0].2, "nope");
}

// ============================================================
// Test 3 & 4: endpoint receives runtime (consumer + producer)
// ============================================================

/// Fixture that captures the Arc<dyn RuntimeObservability> it receives.
struct CapturingEndpoint {
    captured_consumer_rt: Mutex<Option<Arc<dyn RuntimeObservability>>>,
    captured_producer_rt: Mutex<Option<Arc<dyn RuntimeObservability>>>,
}

impl CapturingEndpoint {
    fn new() -> Self {
        Self {
            captured_consumer_rt: Mutex::new(None),
            captured_producer_rt: Mutex::new(None),
        }
    }
}

impl Endpoint for CapturingEndpoint {
    fn uri(&self) -> &str {
        "capturing://test"
    }

    fn create_consumer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        *self.captured_consumer_rt.lock().expect("cap lock") = Some(Arc::clone(&rt));
        // Return a no-op consumer that immediately succeeds at start().
        struct NoopConsumer;
        #[async_trait]
        impl Consumer for NoopConsumer {
            async fn start(&mut self, _: ConsumerContext) -> Result<(), CamelError> {
                Ok(())
            }
            async fn stop(&mut self) -> Result<(), CamelError> {
                Ok(())
            }
        }
        Ok(Box::new(NoopConsumer))
    }

    fn create_producer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
        _: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        *self.captured_producer_rt.lock().expect("cap lock") = Some(Arc::clone(&rt));
        // Return a no-op processor.
        Ok(BoxProcessor::from_fn(|_exchange: Exchange| {
            Box::pin(async { Ok(Exchange::default()) })
        }))
    }
}

#[test]
fn endpoint_consumer_receives_runtime() {
    let ep = CapturingEndpoint::new();
    let rt_in: Arc<dyn RuntimeObservability> = Arc::new(NoOpComponentContext);
    let _consumer = ep
        .create_consumer(Arc::clone(&rt_in))
        .expect("create_consumer");

    let rt_out = ep
        .captured_consumer_rt
        .lock()
        .expect("cap lock")
        .take()
        .expect("no rt captured");
    // Verify pointer equality — same Arc passed in was received.
    assert!(
        Arc::ptr_eq(&rt_in, &rt_out),
        "Endpoint::create_consumer must receive the same Arc<dyn RuntimeObservability> passed by the caller"
    );
}

#[test]
fn endpoint_producer_receives_runtime() {
    let ep = CapturingEndpoint::new();
    let rt_in: Arc<dyn RuntimeObservability> = Arc::new(NoOpComponentContext);
    let ctx = ProducerContext::new();
    let _producer = ep
        .create_producer(Arc::clone(&rt_in), &ctx)
        .expect("create_producer");

    let rt_out = ep
        .captured_producer_rt
        .lock()
        .expect("cap lock")
        .take()
        .expect("no rt captured");
    assert!(
        Arc::ptr_eq(&rt_in, &rt_out),
        "Endpoint::create_producer must receive the same Arc<dyn RuntimeObservability> passed by the caller"
    );
}

// ============================================================
// Test 5: Send + Sync static assertion
// ============================================================

#[test]
fn runtime_observability_send_sync() {
    fn assert_send<T: Send + ?Sized>() {}
    fn assert_sync<T: Sync + ?Sized>() {}
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send::<dyn RuntimeObservability>();
    assert_sync::<dyn RuntimeObservability>();
    assert_send_sync::<Arc<dyn RuntimeObservability>>();

    // Also assert for the concrete blanket-impl target.
    assert_send_sync::<NoOpComponentContext>();

    // If this test compiles, all assertions passed — there's nothing to
    // "run" at test-time.
}
