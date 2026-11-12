//! Live round-trip integration test for Redis Streams.
//!
//! Pins producer `XADD` → consumer-group `XREADGROUP` → `XACK` → `XLEN`
//! against a dedicated `redis:7.0-alpine` testcontainer.
//!
//! **Requires Docker to be running.** Tests will fail if Docker is unavailable.
//!
//! **Requires `integration-tests` feature to compile and run.**

#![cfg(feature = "integration-tests")]

mod support;

use std::time::Duration;

use camel_api::{Body, Exchange, Message};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_redis::{RedisComponent, RedisEndpointConfig, RedisProducer};
use camel_test::CamelTestContext;
use redis::aio::MultiplexedConnection;
use support::install_crypto_provider;
use support::wait::wait_until;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::redis::Redis;
use tower::Service;

/// Stream key and consumer group shared by the round-trip steps.
const STREAM: &str = "rt";
const GROUP: &str = "g";

/// Starts a fresh `redis:7.0-alpine` container for this test (a dedicated
/// broker keeps the pending list and stream state free of cross-test noise).
async fn fresh_redis_70() -> (ContainerAsync<Redis>, String) {
    let container = Redis::default()
        .with_tag("7.0-alpine")
        .start()
        .await
        .expect("Redis 7.0 container failed to start");
    let port = container
        .get_host_port_ipv4(6379)
        .await
        .expect("Redis port not available");
    let conn_str = format!("127.0.0.1:{port}");
    eprintln!("Redis 7.0 ready at: {conn_str}");
    (container, conn_str)
}

/// Reads the entry count out of the `XPENDING` summary reply
/// (`[count, min-id, max-id, consumers]`).
fn summary_pending_count(reply: redis::Value) -> u64 {
    match reply {
        redis::Value::Array(items) => match items.first() {
            Some(redis::Value::Int(n)) if *n >= 0 => *n as u64,
            other => {
                panic!("XPENDING summary count must be a non-negative integer, got {other:?}")
            }
        },
        other => panic!("XPENDING must return a summary array, got {other:?}"),
    }
}

/// One `XPENDING <stream> <group>` summary count over the raw test connection.
async fn pending_count(conn: &mut MultiplexedConnection) -> u64 {
    let reply = redis::cmd("XPENDING")
        .arg(STREAM)
        .arg(GROUP)
        .query_async::<redis::Value>(conn)
        .await
        .expect("XPENDING summary must succeed");
    summary_pending_count(reply)
}

/// Bounded acknowledgement barrier: polls the pending summary every 50 ms for
/// at most 2 s. `send_and_wait` completing only proves the pipeline accepted
/// the entry — the `XACK` is issued afterwards, asynchronously.
async fn await_pending_empty(conn: &mut MultiplexedConnection) -> u64 {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut interval = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_millis(50),
        Duration::from_millis(50),
    );
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // Fail-closed sentinel: an `XPENDING` probe that never completes must leave
    // `pending` non-zero so the caller's assertion fails instead of passing.
    let mut pending = u64::MAX;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        // Bound each probe by the remaining time to the deadline. The deadline
        // check alone is unreachable while a stalled connection parks the poll.
        match tokio::time::timeout(remaining, pending_count(conn)).await {
            Ok(count) => pending = count,
            Err(_elapsed) => break,
        }
        if pending == 0 || tokio::time::Instant::now() >= deadline {
            break;
        }
        interval.tick().await;
    }
    pending
}

#[tokio::test(flavor = "multi_thread")]
async fn live_streams_roundtrip_xadd_consumer_ack_xlen_pending_empty() {
    install_crypto_provider();
    let (_container, conn_str) = fresh_redis_70().await;

    // (a) Producer XADD writes the text body as the single `body` field.
    let mut xadd = RedisProducer::new(
        RedisEndpointConfig::from_uri(&format!("redis://{conn_str}?command=XADD&key={STREAM}"))
            .expect("XADD endpoint config"),
        support::test_rt(),
    )
    .expect("XADD producer");
    let added = xadd
        .call(Exchange::new(Message::new(Body::Text("hello".into()))))
        .await
        .expect("XADD must succeed");
    let entry_id = added
        .input
        .header("CamelRedis.StreamId")
        .and_then(|v| v.as_str())
        .expect("XADD must expose the server-assigned entry id")
        .to_string();
    assert!(!entry_id.is_empty(), "XADD entry id must not be empty");

    // (b) Consumer group read delivers the entry through the pipeline. The
    // component acknowledges only after `ConsumerContext::send_and_wait`
    // reports pipeline completion.
    let h = CamelTestContext::builder()
        .with_mock()
        .with_component(RedisComponent::new())
        .build()
        .await;
    let consumer_uri = format!(
        "redis://{conn_str}?command=XREADGROUP&key={STREAM}&group={GROUP}&consumer=c\
         &createGroup=true&groupStart=earliest"
    );
    let route = RouteBuilder::from(&consumer_uri)
        .to("mock:delivered")
        .route_id("redis-streams-roundtrip")
        .build()
        .expect("stream consumer route");
    h.add_route(route).await.expect("add consumer route");
    h.start().await;

    let endpoint = h
        .mock()
        .get_endpoint("delivered")
        .expect("mock:delivered endpoint");
    wait_until(
        "redis streams entry delivery",
        Duration::from_secs(5),
        Duration::from_millis(50),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .expect("XREADGROUP must deliver the XADD entry");

    let received = endpoint.get_received_exchanges().await;
    assert_eq!(received.len(), 1, "exactly one stream entry expected");
    match &received[0].input.body {
        Body::Text(text) => assert_eq!(
            text.as_bytes(),
            b"hello",
            "round-trip text must be byte-identical to the XADD body"
        ),
        other => panic!("stream entry body must be Text, got {other:?}"),
    }
    assert_eq!(
        received[0]
            .input
            .header("CamelRedis.Stream")
            .and_then(|v| v.as_str()),
        Some(STREAM),
        "CamelRedis.Stream must name the stream key"
    );
    assert_eq!(
        received[0]
            .input
            .header("CamelRedis.StreamId")
            .and_then(|v| v.as_str()),
        Some(entry_id.as_str()),
        "CamelRedis.StreamId must match the XADD entry id"
    );

    // (c) Producer XLEN reports the single written entry.
    let mut xlen = RedisProducer::new(
        RedisEndpointConfig::from_uri(&format!("redis://{conn_str}?command=XLEN&key={STREAM}"))
            .expect("XLEN endpoint config"),
        support::test_rt(),
    )
    .expect("XLEN producer");
    let len = xlen
        .call(Exchange::new(Message::default()))
        .await
        .expect("XLEN must succeed");
    assert_eq!(
        len.input.body,
        Body::Json(serde_json::json!(1)),
        "XLEN must report exactly one stream entry"
    );

    // (d) The asynchronous XACK must have landed: barrier first, assert after.
    let client = redis::Client::open(format!("redis://{conn_str}")).expect("valid redis url");
    // Bound connection establishment: a stalled container must fail the test,
    // not park it forever.
    let mut conn = tokio::time::timeout(
        Duration::from_secs(5),
        client.get_multiplexed_async_connection(),
    )
    .await
    .expect("redis connection must be established within 5s")
    .expect("redis connection");
    let pending = await_pending_empty(&mut conn).await;
    assert_eq!(
        pending, 0,
        "consumer must XACK the delivered entry (XPENDING {STREAM} {GROUP} must be empty)"
    );

    h.stop().await;
}
