//! Integration test for cache resilience: cache_peek_stale serves expired entries.
//!
//! Demonstrates the EFFIS anchor use case: a route that caches data with a short TTL,
//! and a separate route that serves the stale (expired) cached value via cache_peek_stale
//! when the primary cache entry has expired. This is the resilience pattern that would
//! be wired into a CircuitBreaker fallback in production.

use std::time::Duration;

use camel_api::{Exchange, Message, Value};
use camel_test::CamelTestContext;
use tower::ServiceExt;

fn test_rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
    std::sync::Arc::new(camel_component_api::NoOpComponentContext)
}

/// Send an exchange to a direct endpoint, retrying with a fresh producer
/// until the consumer is registered (covers startup race).
async fn send_to_direct(
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cache_peek_stale_serves_expired_entry_in_route() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    // Route 1: cache with very short TTL, on_miss sets the body.
    // Route 2: cache_peek_stale serves the expired entry.
    let yaml = r#"
routes:
  - id: "populate"
    from: "direct:populate"
    steps:
      - cache:
          key: "${header.cacheKey}"
          ttl: "1ms"
          on_miss:
            - set_body: "tile_data_v1"
  - id: "serve-stale"
    from: "direct:serve"
    steps:
      - cache_peek_stale:
          key: "${header.cacheKey}"
      - to: "mock:result"
"#;

    for route in camel_dsl::parse_yaml(yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let mock = h
        .mock()
        .get_endpoint("result")
        .expect("mock endpoint created during route compilation");

    // ── Step 1: Populate the cache ──
    // on_miss runs (sets body to "tile_data_v1"), entry is cached with 1ms TTL.
    let mut ex1 = Exchange::new(Message::new("original"));
    ex1.input
        .set_header("cacheKey", Value::String("tile1".into()));
    send_to_direct(&h, "direct:populate", ex1, Duration::from_secs(2)).await;

    // ── Step 2: Wait for TTL to expire ──
    tokio::time::sleep(Duration::from_millis(50)).await;

    // ── Step 3: Serve stale via cache_peek_stale ──
    // cache_peek_stale finds the expired entry, body becomes "tile_data_v1".
    let mut ex2 = Exchange::new(Message::new("original"));
    ex2.input
        .set_header("cacheKey", Value::String("tile1".into()));
    send_to_direct(&h, "direct:serve", ex2, Duration::from_secs(2)).await;

    mock.await_exchanges(1, Duration::from_secs(2)).await;
    {
        let received = mock.get_received_exchanges().await;
        let body = received[0].input.body.as_text();
        assert_eq!(
            body,
            Some("tile_data_v1"),
            "cache_peek_stale must serve the expired entry body"
        );
    }

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cache_peek_stale_returns_stopped_on_absent_key() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    // Route: cache_peek_stale for a key that was never cached.
    // Since cache_peek_stale returns Stopped when no entry exists,
    // the exchange never reaches mock:result.
    let yaml = r#"
routes:
  - id: "serve-absent"
    from: "direct:serve"
    steps:
      - cache_peek_stale:
          key: "${header.cacheKey}"
      - to: "mock:result"
"#;

    for route in camel_dsl::parse_yaml(yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let mock = h
        .mock()
        .get_endpoint("result")
        .expect("mock endpoint created during route compilation");

    // Send exchange for a key that was never cached
    let mut ex = Exchange::new(Message::new("original"));
    ex.input
        .set_header("cacheKey", Value::String("never-cached".into()));
    send_to_direct(&h, "direct:serve", ex, Duration::from_secs(2)).await;

    // Give a short window — the exchange should NOT reach the mock
    // because cache_peek_stale returns Stopped on absence.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let received = mock.get_received_exchanges().await;
    assert_eq!(
        received.len(),
        0,
        "no exchange should reach mock:result when cache_peek_stale finds no entry"
    );

    h.stop().await;
}
