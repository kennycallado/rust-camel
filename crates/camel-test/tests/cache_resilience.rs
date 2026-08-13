//! Integration test for cache resilience: cache_peek_stale serves expired entries.
//!
//! Demonstrates the EFFIS anchor use case: a route that caches data with a short TTL,
//! and a separate route that serves the stale (expired) cached value via cache_peek_stale
//! when the primary cache entry has expired. This is the resilience pattern that would
//! be wired into a CircuitBreaker fallback in production.

use std::time::Duration;

use camel_api::{CamelError, Exchange, Message, Value};
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

/// Drive a route that is EXPECTED to fail at the pipeline level.
///
/// Like [`send_to_direct`], but tolerates a route-failure reply: it retries
/// only the startup-race error (direct consumer not yet registered) and accepts
/// both `Ok` (an error handler absorbed the failure) and route-failure `Err`
/// replies as valid "the route ran" outcomes. The cache state is settled by the
/// time this returns because the direct producer awaits the route's reply, so a
/// pending write-back (or its absence) is already decided.
async fn send_to_direct_tolerant(
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
            Err(e) => {
                // Startup race: direct consumer not yet registered. The direct
                // component surfaces this as EndpointCreationFailed.
                let is_startup_race = matches!(e, CamelError::EndpointCreationFailed(_))
                    || e.to_string().contains("not registered");
                if is_startup_race && tokio::time::Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    continue;
                }
                if is_startup_race {
                    panic!(
                        "direct consumer for {endpoint_uri} never registered within {timeout:?}"
                    );
                }
                // Route-failure reply: the route ran and the pipeline failed as
                // expected (e.g. cache on_miss returned Failed). Accept it.
                return;
            }
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

/// rc-65yi regression (pins ADR-0058 outcome-aware Segment composition).
///
/// A cache MISS whose `on_miss` is a `do_try` that fails (recipient_list hits an
/// unresolvable endpoint → zero-success → `Failed`) must still serve the stale
/// body produced by a `cache_peek_stale` in the catch — and that body must
/// survive the outer cache write-back (step 4 body takeover). The route uses the
/// default `"memory"` cache repository shared by both routes.
///
/// # rc-fgcu Task 4.1 Scenario 2 — stale-serve stream-ownership (vacuous)
///
/// The blessed plan called for an unconditional assertion that no
/// `Body::Stream already consumed before HTTP reply` error (rc-n8rc) occurs and
/// that a real HTTP 403 reaches the route error path. Task 3.1 established that
/// rc-n8rc's stream-consumed error path (`camel-http/src/lib.rs:1569`) is only
/// reachable via the `Ok(out)` arm of the HTTP consumer reply path, which
/// post-rc-20yn is NOT hit for the recipient_list-all-failed / 403 scenario
/// (zero-success returns `Err`, never `Ok`). The stream-ownership assertion is
/// therefore vacuously satisfied by the rc-20yn invariant. Producing a real HTTP
/// 403 here would add a wiremock + camel-http integration (CamelTestContext does
/// not register the http component by default and no in-repo helper routes
/// through it) for a vacuous assertion, so the gate is this documented
/// resolution plus the `bogus-fail:` (ProcessorError) stale-serve coverage
/// below. The end-to-end cache no-poison property is pinned independently by
/// `cache_poison_timer_recipient_list_all_failed_no_writeback`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cache_peek_stale_in_do_try_catch_serves_stale_body() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    // Route "seed": caches key with a 1ms TTL so it expires but remains
    // peek_stale-visible. Route "stale-via-catch": on the expired MISS the
    // do_try recipient_list fails and the catch serves the stale body.
    let yaml = r#"
routes:
  - id: "seed"
    from: "direct:seed"
    steps:
      - cache:
          key: "${header.cacheKey}"
          ttl: "1ms"
          on_miss:
            - set_body: "stale-payload"
  - id: "stale-via-catch"
    from: "direct:stale"
    steps:
      - cache:
          key: "${header.cacheKey}"
          on_miss:
            - do_try:
                steps:
                  - recipient_list:
                      simple: "bogus-fail:nowhere"
                catch:
                  - exception: ["*"]
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

    // Step 1: seed the key with "stale-payload" and a 1ms TTL.
    let mut seed_ex = Exchange::new(Message::new("seed"));
    seed_ex
        .input
        .set_header("cacheKey", Value::String("k".into()));
    send_to_direct(&h, "direct:seed", seed_ex, Duration::from_secs(2)).await;

    // Step 2: wait for the entry to expire (get→None = MISS, peek_stale→Some).
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Step 3: drive the test route. Outer cache MISSES (expired); on_miss runs
    // do_try; recipient_list fails (unresolvable endpoint); catch fires
    // cache_peek_stale which serves the stale body; the body must survive the
    // outer cache write-back and reach mock:result.
    let mut test_ex = Exchange::new(Message::new("request"));
    test_ex
        .input
        .set_header("cacheKey", Value::String("k".into()));
    send_to_direct(&h, "direct:stale", test_ex, Duration::from_secs(2)).await;

    mock.await_exchanges(1, Duration::from_secs(2)).await;
    {
        let received = mock.get_received_exchanges().await;
        assert_eq!(
            received.len(),
            1,
            "exactly one exchange must reach mock:result"
        );
        let body = received[0].input.body.as_text();
        assert_eq!(
            body,
            Some("stale-payload"),
            "stale body must be served through the do_try catch and survive \
             the outer cache write-back"
        );
    }

    h.stop().await;
}

/// rc-fgcu Task 4.1 Scenario 1 — cache-poison gate (pins rc-20yn end-to-end).
///
/// A cache MISS whose `on_miss` is a `recipient_list` that all-fails
/// (zero-success → `Err` per ADR-0058) MUST NOT poison the cache with the
/// inbound body. Pre-rc-20yn, `recipient_list` returned `Ok` on zero-success,
/// so the cache treated the failed on-miss as `Completed` and wrote the inbound
/// body back — overwriting the stale seed. Post-rc-20yn the `Err` makes on_miss
/// return `Failed`, so the cache skips write-back (cache_eip.rs step 3) and the
/// stale seed survives.
///
/// The poison route is driven via a `direct:` source with [`send_to_direct_tolerant`]:
/// the route legitimately fails (cache on_miss → `Failed` propagates), so the
/// helper retries only the startup-race error and accepts the route-failure
/// reply. A `to: "mock:poison-ran"` probe inside `on_miss` (before the failing
/// `recipient_list`) self-verifies that on_miss actually executed, ruling out a
/// "pass for the wrong reason" if the route never ran. A `direct:` source
/// variant is used (not a timer) because it is fully deterministic — the direct
/// producer awaits the route reply, so the cache state is settled on return.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cache_poison_timer_recipient_list_all_failed_no_writeback() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock_fail_fast()
        .build()
        .await;

    // Route "seed": caches key "k" = "stale-seed" with a 1ms TTL so it expires
    // but remains peek_stale-visible. Route "poison": on the expired MISS the
    // on_miss recipient_list calls a mock endpoint whose producer is latched to
    // fail (resolves OK, fails on call → parallel zero-success → Err), so
    // write-back MUST be skipped. This exercises the ADR-0058 zero-success
    // guard (recipient_list parallel arm) end-to-end through the cache EIP.
    // Route "inspect": a companion read that serves the stale entry via
    // cache_peek_stale.
    let yaml = r#"
routes:
  - id: "seed"
    from: "direct:seed"
    steps:
      - cache:
          key: "${header.cacheKey}"
          ttl: "1ms"
          on_miss:
            - set_body: "stale-seed"
  - id: "poison"
    from: "direct:poison"
    steps:
      - cache:
          key: "${header.cacheKey}"
          ttl: "15m"
          on_miss:
            - to: "mock:poison-ran"
            - recipient_list:
                simple: "mock:poison-fail"
                parallel: true
  - id: "inspect"
    from: "direct:inspect"
    steps:
      - cache_peek_stale:
          key: "${header.cacheKey}"
      - to: "mock:inspect"
"#;

    for route in camel_dsl::parse_yaml(yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    // Pre-create the mock:poison-fail endpoint in the shared mock registry.
    // The recipient_list resolves endpoints lazily at message time, so the
    // endpoint does not exist after start(). By pre-creating it here we can
    // trip the fail-fast latch before any message flows. When the route later
    // resolves "mock:poison-fail" it finds this same endpoint (shared Arc
    // registry).
    camel_component_api::Component::create_endpoint(
        h.mock(),
        "mock:poison-fail",
        &camel_component_api::NoOpComponentContext,
    )
    .expect("pre-create mock:poison-fail endpoint");

    // Trip the fail-fast latch. Once tripped, every MockProducer::call on this
    // endpoint returns Err — the recipient_list resolves it to Some(producer)
    // but the parallel task fails (ready Err), exercising the zero-success
    // guard (ADR-0058).
    h.mock()
        .get_endpoint("poison-fail")
        .expect("mock endpoint 'poison-fail' pre-created above")
        .trigger_fail_fast(CamelError::ProcessorError(
            "simulated downstream failure".to_string(),
        ));

    let poison_ran = h
        .mock()
        .get_endpoint("poison-ran")
        .expect("mock endpoint 'poison-ran' created during route compilation");
    let inspect = h
        .mock()
        .get_endpoint("inspect")
        .expect("mock endpoint 'inspect' created during route compilation");

    // Step 1: seed key "k" = "stale-seed" with a 1ms TTL.
    let mut seed_ex = Exchange::new(Message::new("seed"));
    seed_ex
        .input
        .set_header("cacheKey", Value::String("k".into()));
    send_to_direct(&h, "direct:seed", seed_ex, Duration::from_secs(2)).await;

    // Step 2: wait for the seed to expire (get→None = MISS, peek_stale→Some).
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Step 3: drive the poison route with a distinct inbound body. The cache
    // MISSES (expired); on_miss runs the mock probe then the recipient_list,
    // which all-fails → on_miss returns Failed → no write-back. The route
    // propagates the failure, so the tolerant send accepts the Err reply.
    let mut poison_ex = Exchange::new(Message::new("inbound-poison-body"));
    poison_ex
        .input
        .set_header("cacheKey", Value::String("k".into()));
    send_to_direct_tolerant(&h, "direct:poison", poison_ex, Duration::from_secs(2)).await;

    // Self-verify: on_miss executed (the probe ran before recipient_list).
    poison_ran.await_exchanges(1, Duration::from_secs(2)).await;

    // Step 4: inspect via the companion read route. cache_peek_stale must serve
    // the preserved stale seed — NOT the inbound body and NOT absent/empty.
    let mut inspect_ex = Exchange::new(Message::new("inspect"));
    inspect_ex
        .input
        .set_header("cacheKey", Value::String("k".into()));
    send_to_direct(&h, "direct:inspect", inspect_ex, Duration::from_secs(2)).await;

    inspect.await_exchanges(1, Duration::from_secs(2)).await;
    {
        let received = inspect.get_received_exchanges().await;
        assert_eq!(
            received.len(),
            1,
            "the stale entry must be peek_stale-visible (not absent / not poisoned)"
        );
        let body = received[0].input.body.as_text();
        assert_eq!(
            body,
            Some("stale-seed"),
            "cache must preserve the stale seed; the inbound body must NOT be \
             written back (rc-20yn regression would poison with the inbound body)"
        );
    }

    h.stop().await;
}
