//! Shared helpers for the cache EIP integration tests.
//!
//! The cache test files (`cache_eip_smoke`, `cache_resilience`,
//! `cache_admin_test`) run without the `integration-tests` feature, so they
//! cannot use the gated `support::send_to_direct`. This module holds the
//! non-gated canonical copy.

use std::time::Duration;

use camel_api::{CamelError, Exchange};
use camel_test::CamelTestContext;
use tower::ServiceExt;

pub fn test_rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
    std::sync::Arc::new(camel_component_api::NoOpComponentContext)
}

/// Send an exchange to a direct endpoint, retrying with a fresh producer
/// until the consumer is registered (covers startup race).
pub async fn send_to_direct(
    h: &CamelTestContext,
    endpoint_uri: &str,
    exchange: Exchange,
    timeout: Duration,
) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let producer = {
            let ctx = h.ctx().lock().await;
            let producer_ctx = ctx.producer_context();
            let registry = ctx.registry();
            let component = registry
                .get("direct")
                .expect("direct component not registered");
            let endpoint = component
                .create_endpoint(endpoint_uri, &*ctx)
                .expect("failed to create direct endpoint");
            endpoint
                .create_producer(test_rt(), &producer_ctx)
                .expect("failed to create direct producer")
        };
        match producer.oneshot(exchange.clone()).await {
            Ok(_) => return,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => panic!("failed to send exchange within {timeout:?}: {e}"),
        }
    }
}

/// Send an exchange to a direct endpoint and return the route result.
///
/// Like [`send_to_direct`], but retries only the startup-race error (direct
/// consumer not yet registered) and returns any other `Err` — i.e. a route
/// pipeline failure — to the caller. Used to assert fail-closed errors on a
/// route.
#[allow(dead_code)] // not every test binary including this module uses it
pub async fn send_to_direct_result(
    h: &CamelTestContext,
    endpoint_uri: &str,
    exchange: Exchange,
    timeout: Duration,
) -> Result<(), CamelError> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let producer = {
            let ctx = h.ctx().lock().await;
            let producer_ctx = ctx.producer_context();
            let registry = ctx.registry();
            let component = registry
                .get("direct")
                .expect("direct component not registered");
            let endpoint = component
                .create_endpoint(endpoint_uri, &*ctx)
                .expect("failed to create direct endpoint");
            endpoint
                .create_producer(test_rt(), &producer_ctx)
                .expect("failed to create direct producer")
        };
        match producer.oneshot(exchange.clone()).await {
            Ok(_) => return Ok(()),
            Err(e) => {
                let is_startup_race = matches!(e, CamelError::EndpointCreationFailed(_))
                    || e.to_string().contains("not registered");
                if is_startup_race && tokio::time::Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    continue;
                }
                return Err(e);
            }
        }
    }
}
