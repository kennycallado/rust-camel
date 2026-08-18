//! End-to-end integration tests for the cache admin steps (OpenSpec cache-admin 1.6).
//!
//! Pins the full DSL → BuilderStep → compiler arm → service wiring for the two
//! admin steps that landed in tasks 1.1-1.5:
//! - `cache_clear` empties the whole repository (CacheClearService).
//! - `cache_stats` replaces the exchange body with a JSON snapshot (CacheStatsService).
//!
//! Both steps are already implemented; these tests pin them through the
//! camel-test harness so a regression in the wiring fails loudly.

use std::sync::Arc;
use std::time::Duration;

use camel_api::body::Body;
use camel_api::{Exchange, Message, Value};
use camel_core::cache::RedbCacheRepository;
use camel_test::CamelTestContext;

mod cache_test_support;
use cache_test_support::{send_to_direct, send_to_direct_result};

/// Build an exchange with a `cacheKey` header set to `key`.
fn cache_exchange(key: &str) -> Exchange {
    let mut ex = Exchange::new(Message::new("original"));
    ex.input.set_header("cacheKey", Value::String(key.into()));
    ex
}

// ─────────────────────────────────────────────────────────────────────────────
// cache_clear empties the repository: on_miss re-runs after a clear.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cache_clear_then_lookup_misses() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    // on_miss sets the body from the `seedBody` header so each probe can
    // distinguish a cache HIT (cached body) from a MISS (fresh body). The
    // cache_clear route sits on a separate direct endpoint.
    let yaml = r#"
routes:
  - id: "cache-clear-populate"
    from: "direct:populate"
    steps:
      - cache:
          repository: memory
          key: "${header.cacheKey}"
          on_miss:
            - set_body:
                simple: "${header.seedBody}"
      - to: "mock:populate"
  - id: "cache-clear-admin"
    from: "direct:clear"
    steps:
      - cache_clear:
          repository: memory
      - to: "mock:cleared"
"#;

    for route in camel_dsl::parse_yaml(yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let populate = h.mock().get_endpoint("populate").expect("populate mock");
    let cleared = h.mock().get_endpoint("cleared").expect("cleared mock");

    // ── 1. Warm the cache: MISS → on_miss sets body "v1". ──
    let mut ex1 = cache_exchange("k");
    ex1.input.set_header("seedBody", Value::String("v1".into()));
    send_to_direct(&h, "direct:populate", ex1, Duration::from_secs(2)).await;
    populate.await_exchanges(1, Duration::from_secs(2)).await;
    {
        let received = populate.get_received_exchanges().await;
        assert_eq!(received[0].input.body.as_text(), Some("v1"));
    }

    // ── 2. HIT probe: same key, different seed → cached "v1" (on_miss skipped). ──
    let mut ex2 = cache_exchange("k");
    ex2.input.set_header("seedBody", Value::String("v2".into()));
    send_to_direct(&h, "direct:populate", ex2, Duration::from_secs(2)).await;
    populate.await_exchanges(2, Duration::from_secs(2)).await;
    {
        let received = populate.get_received_exchanges().await;
        assert_eq!(
            received[1].input.body.as_text(),
            Some("v1"),
            "hit must return cached v1, not seedBody v2"
        );
    }

    // ── 3. Clear the repository. ──
    send_to_direct(
        &h,
        "direct:clear",
        Exchange::new(Message::new("clear")),
        Duration::from_secs(2),
    )
    .await;
    cleared.await_exchanges(1, Duration::from_secs(2)).await;
    {
        let received = cleared.get_received_exchanges().await;
        assert_eq!(
            received[0].input.body.as_text(),
            Some("clear"),
            "clear step must pass the clearing exchange's body through unchanged"
        );
    }

    // ── 4. Post-clear probe: entry gone → on_miss runs again → body "v3". ──
    let mut ex4 = cache_exchange("k");
    ex4.input.set_header("seedBody", Value::String("v3".into()));
    send_to_direct(&h, "direct:populate", ex4, Duration::from_secs(2)).await;
    populate.await_exchanges(3, Duration::from_secs(2)).await;
    {
        let received = populate.get_received_exchanges().await;
        assert_eq!(
            received[2].input.body.as_text(),
            Some("v3"),
            "post-clear lookup must re-run on_miss (body v3)"
        );
    }

    h.stop().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// cache_stats emits a JSON snapshot body after known operations.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cache_stats_returns_json_snapshot() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let yaml = r#"
routes:
  - id: "cache-stats-populate"
    from: "direct:populate"
    steps:
      - cache:
          repository: memory
          key: "${header.cacheKey}"
          on_miss:
            - set_body: "seed"
      - to: "mock:populate"
  - id: "cache-stats-invalidate"
    from: "direct:invalidate"
    steps:
      - cache_invalidate:
          repository: memory
          key: "${header.cacheKey}"
      - to: "mock:invalidated"
  - id: "cache-stats-admin"
    from: "direct:stats"
    steps:
      - cache_stats:
          repository: memory
      - to: "mock:stats"
"#;

    for route in camel_dsl::parse_yaml(yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let populate = h.mock().get_endpoint("populate").expect("populate mock");
    let invalidated = h
        .mock()
        .get_endpoint("invalidated")
        .expect("invalidated mock");
    let stats = h.mock().get_endpoint("stats").expect("stats mock");

    // ── 3 cache lookups on key "k": miss, hit, hit. ──
    for _ in 0..3 {
        send_to_direct(
            &h,
            "direct:populate",
            cache_exchange("k"),
            Duration::from_secs(2),
        )
        .await;
    }
    populate.await_exchanges(3, Duration::from_secs(2)).await;

    // ── 1 invalidation of the seeded key. ──
    send_to_direct(
        &h,
        "direct:invalidate",
        cache_exchange("k"),
        Duration::from_secs(2),
    )
    .await;
    invalidated.await_exchanges(1, Duration::from_secs(2)).await;

    // ── Read the stats snapshot. ──
    send_to_direct(
        &h,
        "direct:stats",
        Exchange::new(Message::new("stats")),
        Duration::from_secs(2),
    )
    .await;
    stats.await_exchanges(1, Duration::from_secs(2)).await;

    let received = stats.get_received_exchanges().await;
    match &received[0].input.body {
        Body::Json(v) => {
            assert_eq!(v["repository"].as_str(), Some("memory"));
            assert_eq!(v["hits"].as_u64(), Some(2));
            assert_eq!(v["misses"].as_u64(), Some(1));
            assert_eq!(v["invalidations"].as_u64(), Some(1));
            assert_eq!(v.get("bytes"), Some(&serde_json::Value::Null));
        }
        other => panic!("expected Body::Json, got {:?}", other),
    }

    h.stop().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// cache_invalidate key_prefix purges one namespace on a redb repository and
// reports CamelCacheInvalidatedCount == 2, leaving other namespaces intact.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cache_invalidate_prefix_purges_namespace_redb() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    // Register a persistent redb repository at a temp file path, bound to the
    // context's shutdown token so its sweep task tears down with the harness.
    let dir = tempfile::tempdir().expect("tempdir");
    {
        let mut ctx = h.ctx().lock().await;
        let repo = RedbCacheRepository::new(
            "persistent",
            dir.path().join("cache.redb"),
            Duration::from_secs(7 * 24 * 3600),
            Some(1_000_000),
            Duration::from_secs(3600),
            ctx.shutdown_token(),
        )
        .await
        .expect("open redb repo");
        ctx.register_cache_repository("persistent", Arc::new(repo))
            .expect("register persistent cache repository");
    }

    let yaml = r#"
routes:
  - id: "redb-populate"
    from: "direct:populate"
    steps:
      - cache:
          repository: persistent
          key: "${header.cacheKey}"
          on_miss:
            - set_body:
                simple: "${header.seedBody}"
      - to: "mock:populate"
  - id: "redb-invalidate"
    from: "direct:invalidate"
    steps:
      - cache_invalidate:
          repository: persistent
          key_prefix: "ns:"
      - to: "mock:invalidated"
"#;

    for route in camel_dsl::parse_yaml(yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let populate = h.mock().get_endpoint("populate").expect("populate mock");
    let invalidated = h
        .mock()
        .get_endpoint("invalidated")
        .expect("invalidated mock");

    // ── Warm three keys across two namespaces. ──
    for (key, seed) in [("ns:one", "one"), ("ns:two", "two"), ("other:x", "x")] {
        let mut ex = cache_exchange(key);
        ex.input.set_header("seedBody", Value::String(seed.into()));
        send_to_direct(&h, "direct:populate", ex, Duration::from_secs(2)).await;
    }
    populate.await_exchanges(3, Duration::from_secs(2)).await;

    // ── Invalidate the "ns:" prefix. ──
    send_to_direct(
        &h,
        "direct:invalidate",
        Exchange::new(Message::new("invalidate")),
        Duration::from_secs(2),
    )
    .await;
    invalidated.await_exchanges(1, Duration::from_secs(2)).await;
    {
        let received = invalidated.get_received_exchanges().await;
        assert_eq!(
            received[0].property("CamelCacheInvalidatedCount"),
            Some(&Value::from(2u64)),
            "prefix purge must report 2 invalidated entries"
        );
    }

    // ── other:x still hits (on_miss skipped); ns:one re-runs on_miss. ──
    let mut ex_other = cache_exchange("other:x");
    ex_other
        .input
        .set_header("seedBody", Value::String("X2".into()));
    send_to_direct(&h, "direct:populate", ex_other, Duration::from_secs(2)).await;

    let mut ex_ns = cache_exchange("ns:one");
    ex_ns
        .input
        .set_header("seedBody", Value::String("one2".into()));
    send_to_direct(&h, "direct:populate", ex_ns, Duration::from_secs(2)).await;

    populate.await_exchanges(5, Duration::from_secs(2)).await;
    {
        let received = populate.get_received_exchanges().await;
        assert_eq!(
            received[3].input.body.as_text(),
            Some("x"),
            "other:x must still hit after prefix purge"
        );
        assert_eq!(
            received[4].input.body.as_text(),
            Some("one2"),
            "ns:one must re-run on_miss after prefix purge"
        );
    }

    h.stop().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// cache_invalidate key_prefix on the memory backend fails closed with an error
// naming the backend (the default `invalidate_prefix` returns Err).
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cache_invalidate_prefix_memory_fails_closed() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let yaml = r#"
routes:
  - id: "memory-invalidate-prefix"
    from: "direct:invalidate"
    steps:
      - cache_invalidate:
          repository: memory
          key_prefix: "ns:"
      - to: "mock:never"
"#;

    for route in camel_dsl::parse_yaml(yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let err = send_to_direct_result(
        &h,
        "direct:invalidate",
        Exchange::new(Message::new("boom")),
        Duration::from_secs(2),
    )
    .await
    .expect_err("memory prefix purge must fail closed");

    let msg = err.to_string();
    assert!(
        msg.contains("memory"),
        "error must name the backend, got: {msg}"
    );
    assert!(
        msg.contains("invalidate_prefix"),
        "error must mention invalidate_prefix, got: {msg}"
    );

    h.stop().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// coalesce_misses: three concurrent misses on the same key fetch once — the
// on_miss gate fires exactly once and all three responses carry the body.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn coalesce_misses_single_fetch_under_concurrency() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    // The `delay` keeps the leader inside on_miss long enough for the two
    // other exchanges to register as waiters, making the singleflight overlap
    // deterministic.
    let yaml = r#"
routes:
  - id: "coalesce"
    from: "direct:start"
    steps:
      - cache:
          key: "k"
          coalesce_misses: true
          on_miss:
            - delay: 500
            - set_body: "fetched"
            - to: "mock:gate"
      - to: "mock:result"
"#;

    for route in camel_dsl::parse_yaml(yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let gate = h.mock().get_endpoint("gate").expect("gate mock");
    let result = h.mock().get_endpoint("result").expect("result mock");

    // Three concurrent misses on the same key, wrapped in a timeout so a
    // stranded waiter fails the test fast instead of hanging.
    tokio::time::timeout(Duration::from_secs(5), async {
        let a = send_to_direct(
            &h,
            "direct:start",
            Exchange::new(Message::new("a")),
            Duration::from_secs(5),
        );
        let b = send_to_direct(
            &h,
            "direct:start",
            Exchange::new(Message::new("b")),
            Duration::from_secs(5),
        );
        let c = send_to_direct(
            &h,
            "direct:start",
            Exchange::new(Message::new("c")),
            Duration::from_secs(5),
        );
        tokio::join!(a, b, c);
    })
    .await
    .expect("concurrent misses must not strand a waiter");

    result.await_exchanges(3, Duration::from_secs(2)).await;
    gate.await_exchanges(1, Duration::from_secs(2)).await;

    // The gate fired exactly once — on_miss ran once under coalesce.
    assert_eq!(
        gate.received_count().await,
        1,
        "on_miss must run exactly once under coalesce_misses"
    );

    // All three responses carry the fetched body.
    let received = result.get_received_exchanges().await;
    assert_eq!(
        received.len(),
        3,
        "all three exchanges must reach mock:result"
    );
    for ex in &received {
        assert_eq!(
            ex.input.body.as_text(),
            Some("fetched"),
            "every waiter must receive the fetched body"
        );
    }

    h.stop().await;
}
