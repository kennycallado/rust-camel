//! Shared helpers for the cache EIP integration tests.
//!
//! The cache test files (`cache_eip_smoke`, `cache_resilience`,
//! `cache_admin_test`) run without the `integration-tests` feature, so they
//! cannot use the gated `support::send_to_direct`. This module holds the
//! non-gated canonical copy.

// Each cache test binary compiles this module independently; helpers used
// by only one binary must not fail dead-code in the others.
#![allow(dead_code)]

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
    // Anti-wedge backstop (lintwiden D4.1S): the retry loop's own deadline
    // governs startup-race exhaustion; this outer bound only turns a stalled
    // ctx-lock or producer await into a loud failure instead of a wedge.
    tokio::time::timeout(timeout + Duration::from_secs(5), async {
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
    })
    .await
    .expect("send_to_direct stalled beyond its retry deadline");
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
    // Anti-wedge backstop (lintwiden D4.1S): the retry loop's own deadline
    // governs startup-race exhaustion; this outer bound only turns a stalled
    // ctx-lock or producer await into a loud failure instead of a wedge.
    tokio::time::timeout(timeout + Duration::from_secs(5), async {
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
                    // Retry only the direct startup race via the shared
                    // structural predicate (rc-utx98); the seda gate fails
                    // fast (rc-tgaxf).
                    let is_startup_race = camel_component_seda::is_direct_startup_race(&e);
                    if is_startup_race && tokio::time::Instant::now() < deadline {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        continue;
                    }
                    return Err(e);
                }
            }
        }
    })
    .await
    .expect("send_to_direct_result stalled beyond its retry deadline")
}
