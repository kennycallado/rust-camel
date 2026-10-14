//! rc-hcica: `/readyz` must reflect only listeners this process owns. A
//! producer endpoint (`to("http://remote/...")`) registers the same health
//! check as a consumer, so the check stays inert until a consumer is created.

use std::sync::{Arc, Mutex};

use camel_api::{AsyncHealthCheck, HealthStatus};
use camel_component_api::test_support::NoopRuntimeObservability;
use camel_component_api::{Component, ComponentContext, NoOpComponentContext};

use crate::{HttpComponent, HttpsComponent};

#[derive(Default)]
struct RecordingContext {
    checks: Mutex<Vec<Arc<dyn AsyncHealthCheck>>>,
}

impl ComponentContext for RecordingContext {
    fn resolve_component(&self, _scheme: &str) -> Option<Arc<dyn Component>> {
        None
    }

    fn resolve_language(&self, name: &str) -> Option<Arc<dyn camel_language_api::Language>> {
        NoOpComponentContext.resolve_language(name)
    }

    fn metrics(&self) -> Arc<dyn camel_api::MetricsCollector> {
        Arc::new(camel_api::NoOpMetrics)
    }

    fn platform_service(&self) -> Arc<dyn camel_api::PlatformService> {
        Arc::new(camel_api::NoopPlatformService::default())
    }

    fn register_route_health_check(&self, _route_id: &str, _check: Arc<dyn AsyncHealthCheck>) {}

    fn unregister_route_health_check(&self, _route_id: &str) {}

    fn register_current_route_health_check(&self, check: Arc<dyn AsyncHealthCheck>) {
        self.checks.lock().expect("recording lock").push(check); // allow-unwrap
    }
}

/// A loopback port nothing listens on.
async fn closed_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind"); // allow-unwrap
    listener.local_addr().expect("addr").port() // allow-unwrap
}

async fn assert_gate_follows_consumer(
    component: &dyn Component,
    scheme: &str,
    consumer_uri_extra: &str,
) {
    let port = closed_port().await;
    let uri = format!("{scheme}://127.0.0.1:{port}/x{consumer_uri_extra}");

    // Producer side: remote down, nobody ever creates a consumer.
    let producer_ctx = RecordingContext::default();
    let _producer_endpoint = component
        .create_endpoint(&uri, &producer_ctx)
        .expect("endpoint builds"); // allow-unwrap
    let producer_check = producer_ctx.checks.lock().expect("lock").remove(0); // allow-unwrap
    assert_eq!(
        producer_check.check().await.status,
        HealthStatus::Healthy,
        "a producer-only endpoint must not probe its remote target"
    );

    // Consumer side: same URI shape, but the listener is down -> unhealthy.
    let consumer_ctx = RecordingContext::default();
    let consumer_endpoint = component
        .create_endpoint(&uri, &consumer_ctx)
        .expect("endpoint builds"); // allow-unwrap
    let consumer_check = consumer_ctx.checks.lock().expect("lock").remove(0); // allow-unwrap
    assert_eq!(
        consumer_check.check().await.status,
        HealthStatus::Healthy,
        "inert until the consumer exists"
    );
    let _consumer = consumer_endpoint
        .create_consumer(Arc::new(NoopRuntimeObservability))
        .expect("consumer builds"); // allow-unwrap
    assert_eq!(
        consumer_check.check().await.status,
        HealthStatus::Unhealthy,
        "a consumer whose own listener is down must still fail readiness"
    );
}

#[tokio::test]
async fn http_listener_check_is_inert_for_producers_only() {
    assert_gate_follows_consumer(&HttpComponent::new(), "http", "").await;
}

#[tokio::test]
async fn https_listener_check_is_inert_for_producers_only() {
    // https:// consumers require a TLS cert/key pair; create_consumer only
    // checks they are configured (files are read at start()).
    assert_gate_follows_consumer(
        &HttpsComponent::new(),
        "https",
        "?tlsCert=/x.pem&tlsKey=/y.pem",
    )
    .await;
}
