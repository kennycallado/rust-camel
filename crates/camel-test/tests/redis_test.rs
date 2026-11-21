//! Integration tests for Redis component.
//!
//! Uses testcontainers to spin up Redis instances for testing.
//!
//! **Requires Docker to be running.** Tests will fail if Docker is unavailable.
//!
//! **Requires `integration-tests` feature to compile and run.**

#![cfg(feature = "integration-tests")]

mod support;
use support::install_crypto_provider;
use support::redis::shared_redis;

use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{Body, Exchange, Message, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::test_support::NoopRuntimeObservability;
use camel_component_api::{CamelError, NetworkRetryPolicy};
use camel_component_redis::{RedisComponent, RedisConfig, RedisEndpointConfig, RedisProducer};
use camel_test::CamelTestContext;
use futures::{FutureExt, StreamExt};
use redis::AsyncCommands;
use redis::aio::MultiplexedConnection;
use serde_json::json;
use std::collections::HashMap;
use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use support::send_to_direct;
use support::wait::wait_until;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::redis::Redis;
use tower::Service;

// ===========================================================================
// String commands tests
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn redis_string_commands() {
    install_crypto_provider();
    let conn_str = shared_redis().await.to_string();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(RedisComponent::new())
        .build()
        .await;
    h.ctx()
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("CamelRedis.Key", Value::String("testkey".into()))
        .set_body("testvalue")
        .to(format!("redis://{}?command=SET", conn_str))
        .to("mock:result")
        .route_id("redis-string-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    wait_until(
        "redis string route delivery",
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(100),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    endpoint.assert_exchange_count(1).await;
}

// ===========================================================================
// List commands tests
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn redis_list_commands() {
    install_crypto_provider();
    let conn_str = shared_redis().await.to_string();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(RedisComponent::new())
        .build()
        .await;
    h.ctx()
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("CamelRedis.Key", Value::String("mylist".into()))
        .set_body("item1")
        .to(format!("redis://{}?command=LPUSH", conn_str))
        .to("mock:result")
        .route_id("redis-list-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    wait_until(
        "redis list route delivery",
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(100),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    endpoint.assert_exchange_count(1).await;
}

// ===========================================================================
// Hash commands tests
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn redis_hash_commands() {
    install_crypto_provider();
    let conn_str = shared_redis().await.to_string();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(RedisComponent::new())
        .build()
        .await;
    h.ctx()
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("CamelRedis.Key", Value::String("myhash".into()))
        .set_header("CamelRedis.Field", Value::String("field1".into()))
        .set_body("value1")
        .to(format!("redis://{}?command=HSET", conn_str))
        .to("mock:result")
        .route_id("redis-hash-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    wait_until(
        "redis hash route delivery",
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(100),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    endpoint.assert_exchange_count(1).await;
}

// ===========================================================================
// Set commands tests
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn redis_set_commands() {
    install_crypto_provider();
    let conn_str = shared_redis().await.to_string();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(RedisComponent::new())
        .build()
        .await;
    h.ctx()
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("CamelRedis.Key", Value::String("myset".into()))
        .set_body("member1")
        .to(format!("redis://{}?command=SADD", conn_str))
        .to("mock:result")
        .route_id("redis-set-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    wait_until(
        "redis set route delivery",
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(100),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    endpoint.assert_exchange_count(1).await;
}

// ===========================================================================
// Geo commands tests
// ===========================================================================

/// Sends one exchange with `headers` through the direct route bound to a GEO
/// command and returns the JSON body the redis producer set as the reply.
/// Commands that take no value payload send an empty body.
async fn geo_exchange(
    h: &CamelTestContext,
    direct_uri: &str,
    headers: &[(&str, Value)],
) -> serde_json::Value {
    send_geo_exchange(h, direct_uri, headers, None).await
}

/// Same as [`geo_exchange`] for `GEOADD`, whose member rides on the body.
async fn geo_exchange_with_body(
    h: &CamelTestContext,
    direct_uri: &str,
    headers: &[(&str, Value)],
    body: Value,
) -> serde_json::Value {
    send_geo_exchange(h, direct_uri, headers, Some(body)).await
}

async fn send_geo_exchange(
    h: &CamelTestContext,
    direct_uri: &str,
    headers: &[(&str, Value)],
    body: Option<Value>,
) -> serde_json::Value {
    let mut ex = Exchange::default();
    if let Some(body) = body {
        ex.input.body = Body::Json(body);
    }
    for (name, value) in headers {
        ex.input.set_header(*name, value.clone());
    }
    let reply = send_to_direct(h, direct_uri, ex)
        .await
        .expect("GEO command must succeed");
    match reply.input.body {
        Body::Json(v) => v,
        other => panic!("GEO reply body must be JSON, got {other:?}"),
    }
}

/// Asserts a GEOPOS entry is a `[lon, lat]` pair within `1e-4` of the target.
fn assert_position(entry: &serde_json::Value, lon: f64, lat: f64, label: &str) {
    let pair = entry
        .as_array()
        .unwrap_or_else(|| panic!("{label}: position must be a [lon, lat] pair, got {entry}"));
    let got_lon = pair[0]
        .as_f64()
        .unwrap_or_else(|| panic!("{label}: longitude must be numeric, got {entry}"));
    let got_lat = pair[1]
        .as_f64()
        .unwrap_or_else(|| panic!("{label}: latitude must be numeric, got {entry}"));
    assert!(
        (got_lon - lon).abs() < 1e-4 && (got_lat - lat).abs() < 1e-4,
        "{label}: position ({got_lon}, {got_lat}) must be within 1e-4 of ({lon}, {lat})"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn redis_geo_commands() {
    install_crypto_provider();
    let conn_str = shared_redis().await.to_string();

    let h = CamelTestContext::builder()
        .with_direct()
        .with_component(RedisComponent::new())
        .build()
        .await;

    // One direct route per GEO command: addressing operands (member(s), unit,
    // radius or box, WithDist/WithCoord) ride on the exchange as CamelRedis.*
    // headers while GEOADD takes its member from the body, so the routes only
    // pin the redis command.
    for (id, command) in [
        ("geo-add", "GEOADD"),
        ("geo-pos", "GEOPOS"),
        ("geo-dist", "GEODIST"),
        ("geo-hash", "GEOHASH"),
        ("geo-search", "GEOSEARCH"),
    ] {
        let route = RouteBuilder::from(&format!("direct:{id}"))
            .to(format!("redis://{conn_str}?command={command}"))
            .route_id(format!("redis-{id}"))
            .build()
            .unwrap();
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    // ── GEOADD ──
    // The member is the value payload: it rides on the body.
    let added = geo_exchange_with_body(
        &h,
        "direct:geo-add",
        &[
            ("CamelRedis.Key", json!("geo:points")),
            ("CamelRedis.Longitude", json!(13.361389)),
            ("CamelRedis.Latitude", json!(38.115556)),
        ],
        json!("Palermo"),
    )
    .await;
    assert_eq!(added, json!(1), "adding Palermo must report one new member");

    let added = geo_exchange_with_body(
        &h,
        "direct:geo-add",
        &[
            ("CamelRedis.Key", json!("geo:points")),
            ("CamelRedis.Longitude", json!(15.087269)),
            ("CamelRedis.Latitude", json!(37.502669)),
        ],
        json!("Catania"),
    )
    .await;
    assert_eq!(added, json!(1), "adding Catania must report one new member");

    let added = geo_exchange_with_body(
        &h,
        "direct:geo-add",
        &[
            ("CamelRedis.Key", json!("geo:points")),
            // Update: a barely moved coordinate keeps the 1e-4 assertions
            // below valid for both the original and the stored position.
            ("CamelRedis.Longitude", json!(13.361390)),
            ("CamelRedis.Latitude", json!(38.115557)),
        ],
        json!("Palermo"),
    )
    .await;
    assert_eq!(
        added,
        json!(0),
        "updating Palermo must report no new member"
    );

    let added = geo_exchange_with_body(
        &h,
        "direct:geo-add",
        &[
            ("CamelRedis.Key", json!("geo:points")),
            ("CamelRedis.Longitude", json!(0.0)),
            ("CamelRedis.Latitude", json!(0.0)),
        ],
        json!("Farpoint"),
    )
    .await;
    assert_eq!(
        added,
        json!(1),
        "adding Farpoint must report one new member"
    );

    // ── GEOPOS ──
    let positions = geo_exchange(
        &h,
        "direct:geo-pos",
        &[
            ("CamelRedis.Key", json!("geo:points")),
            ("CamelRedis.Members", json!(["Palermo", "Catania"])),
        ],
    )
    .await;
    let positions = positions
        .as_array()
        .expect("GEOPOS body must be an array of positions");
    assert_eq!(positions.len(), 2, "one position per requested member");
    assert_position(&positions[0], 13.361389, 38.115556, "Palermo");
    assert_position(&positions[1], 15.087269, 37.502669, "Catania");

    let positions = geo_exchange(
        &h,
        "direct:geo-pos",
        &[
            ("CamelRedis.Key", json!("geo:points")),
            ("CamelRedis.Members", json!(["Palermo", "Atlantis"])),
        ],
    )
    .await;
    assert_eq!(
        positions[1],
        json!(null),
        "an absent member must map to a null position"
    );
    assert_position(&positions[0], 13.361389, 38.115556, "Palermo");

    let positions = geo_exchange(
        &h,
        "direct:geo-pos",
        &[
            ("CamelRedis.Key", json!("geo:absent")),
            ("CamelRedis.Members", json!(["Palermo", "Catania"])),
        ],
    )
    .await;
    assert_eq!(
        positions,
        json!([null, null]),
        "GEOPOS on an absent key must return null entries"
    );

    // ── GEODIST ──
    let distance = geo_exchange(
        &h,
        "direct:geo-dist",
        &[
            ("CamelRedis.Key", json!("geo:points")),
            ("CamelRedis.Member", json!("Palermo")),
            ("CamelRedis.Member2", json!("Catania")),
            ("CamelRedis.Unit", json!("km")),
        ],
    )
    .await;
    let distance = distance
        .as_f64()
        .expect("GEODIST body must be numeric or null");
    assert!(
        distance > 100.0,
        "Palermo-Catania is ~166 km, got {distance}"
    );

    let distance = geo_exchange(
        &h,
        "direct:geo-dist",
        &[
            ("CamelRedis.Key", json!("geo:points")),
            ("CamelRedis.Member", json!("Palermo")),
            ("CamelRedis.Member2", json!("Atlantis")),
            ("CamelRedis.Unit", json!("km")),
        ],
    )
    .await;
    assert!(
        distance.is_null(),
        "GEODIST with an absent member must be null, got {distance}"
    );

    let distance = geo_exchange(
        &h,
        "direct:geo-dist",
        &[
            ("CamelRedis.Key", json!("geo:absent")),
            ("CamelRedis.Member", json!("Palermo")),
            ("CamelRedis.Member2", json!("Catania")),
            ("CamelRedis.Unit", json!("km")),
        ],
    )
    .await;
    assert!(
        distance.is_null(),
        "GEODIST on an absent key must be null, got {distance}"
    );

    let distance = geo_exchange(
        &h,
        "direct:geo-dist",
        &[
            ("CamelRedis.Key", json!("geo:points")),
            ("CamelRedis.Member", json!("Palermo")),
            ("CamelRedis.Member2", json!("Catania")),
        ],
    )
    .await;
    let distance = distance
        .as_f64()
        .expect("GEODIST body must be numeric or null");
    assert!(
        distance > 100000.0,
        "GEODIST without a unit defaults to meters, got {distance}"
    );

    // ── GEOHASH ──
    let hashes = geo_exchange(
        &h,
        "direct:geo-hash",
        &[
            ("CamelRedis.Key", json!("geo:points")),
            (
                "CamelRedis.Members",
                json!(["Palermo", "Catania", "Atlantis"]),
            ),
        ],
    )
    .await;
    let hashes = hashes
        .as_array()
        .expect("GEOHASH body must be an array of geohashes");
    assert_eq!(hashes.len(), 3, "one geohash per requested member");
    assert!(
        hashes[0].as_str().is_some_and(|h| !h.is_empty()),
        "Palermo geohash must be a non-empty string, got {}",
        hashes[0]
    );
    assert!(
        hashes[1].as_str().is_some_and(|h| !h.is_empty()),
        "Catania geohash must be a non-empty string, got {}",
        hashes[1]
    );
    assert!(
        hashes[2].is_null(),
        "an absent member must map to a null geohash"
    );

    // ── GEOSEARCH ──
    let members = geo_exchange(
        &h,
        "direct:geo-search",
        &[
            ("CamelRedis.Key", json!("geo:points")),
            ("CamelRedis.Longitude", json!(15.0)),
            ("CamelRedis.Latitude", json!(37.0)),
            ("CamelRedis.Radius", json!(200)),
            ("CamelRedis.Unit", json!("km")),
        ],
    )
    .await;
    let members = members
        .as_array()
        .expect("plain GEOSEARCH body must be an array");
    assert!(
        members.iter().all(|m| m.is_string()),
        "plain GEOSEARCH rows must be plain strings, got {members:?}"
    );
    assert!(members.contains(&json!("Palermo")), "got {members:?}");
    assert!(members.contains(&json!("Catania")), "got {members:?}");

    let rows = geo_exchange(
        &h,
        "direct:geo-search",
        &[
            ("CamelRedis.Key", json!("geo:points")),
            ("CamelRedis.Longitude", json!(15.0)),
            ("CamelRedis.Latitude", json!(37.0)),
            ("CamelRedis.Radius", json!(200)),
            ("CamelRedis.Unit", json!("km")),
            ("CamelRedis.WithDist", json!(true)),
        ],
    )
    .await;
    let rows = rows
        .as_array()
        .expect("GEOSEARCH WithDist body must be an array");
    assert!(!rows.is_empty(), "the 200 km radius cannot be empty");
    for row in rows {
        let obj = row
            .as_object()
            .expect("GEOSEARCH WithDist rows must be objects");
        let member = obj
            .get("member")
            .and_then(|v| v.as_str())
            .expect("row member must be a string");
        let distance = obj
            .get("distance")
            .and_then(|v| v.as_f64())
            .unwrap_or_else(|| panic!("{member}: row distance must be numeric, got {row}"));
        assert!(
            distance > 0.0 && distance < 200.0,
            "{member}: distance must be a km figure inside the radius, got {distance}"
        );
    }

    let rows = geo_exchange(
        &h,
        "direct:geo-search",
        &[
            ("CamelRedis.Key", json!("geo:points")),
            ("CamelRedis.Longitude", json!(15.0)),
            ("CamelRedis.Latitude", json!(37.0)),
            ("CamelRedis.Radius", json!(200)),
            ("CamelRedis.Unit", json!("km")),
            ("CamelRedis.WithCoord", json!(true)),
        ],
    )
    .await;
    let rows = rows
        .as_array()
        .expect("GEOSEARCH WithCoord body must be an array");
    // GEOSEARCH row order is unspecified, so the coordinate check is keyed
    // by member instead of relying on the reply's ordering.
    let mut coords: std::collections::HashMap<&str, (f64, f64)> = std::collections::HashMap::new();
    for row in rows {
        let obj = row
            .as_object()
            .expect("GEOSEARCH WithCoord rows must be objects");
        assert!(
            !obj.contains_key("distance"),
            "WithCoord rows must not carry distances, got {row}"
        );
        let member = obj
            .get("member")
            .and_then(|v| v.as_str())
            .expect("row member must be a string");
        let lon = obj
            .get("longitude")
            .and_then(|v| v.as_f64())
            .expect("row longitude must be numeric");
        let lat = obj
            .get("latitude")
            .and_then(|v| v.as_f64())
            .expect("row latitude must be numeric");
        coords.insert(member, (lon, lat));
    }
    for (member, lon, lat) in [
        ("Palermo", 13.361389, 38.115556),
        ("Catania", 15.087269, 37.502669),
    ] {
        let &(got_lon, got_lat) = coords
            .get(member)
            .unwrap_or_else(|| panic!("GEOSEARCH WithCoord must return {member}, got {coords:?}"));
        assert!(
            (got_lon - lon).abs() < 1e-4 && (got_lat - lat).abs() < 1e-4,
            "{member}: coordinates ({got_lon}, {got_lat}) must be within 1e-4 of ({lon}, {lat})"
        );
    }

    let members = geo_exchange(
        &h,
        "direct:geo-search",
        &[
            ("CamelRedis.Key", json!("geo:points")),
            ("CamelRedis.Longitude", json!(15.0)),
            ("CamelRedis.Latitude", json!(37.0)),
            ("CamelRedis.Width", json!(400)),
            ("CamelRedis.Height", json!(400)),
            ("CamelRedis.Unit", json!("km")),
        ],
    )
    .await;
    let members = members
        .as_array()
        .expect("GEOSEARCH BYBOX body must be an array");
    assert!(
        members.contains(&json!("Palermo")) && members.contains(&json!("Catania")),
        "the 400 km box must contain Palermo and Catania, got {members:?}"
    );
    assert!(
        !members.contains(&json!("Farpoint")),
        "the 400 km box must not reach Farpoint, got {members:?}"
    );

    let members = geo_exchange(
        &h,
        "direct:geo-search",
        &[
            ("CamelRedis.Key", json!("geo:absent")),
            ("CamelRedis.Longitude", json!(15.0)),
            ("CamelRedis.Latitude", json!(37.0)),
            ("CamelRedis.Radius", json!(200)),
            ("CamelRedis.Unit", json!("km")),
        ],
    )
    .await;
    assert_eq!(
        members,
        json!([]),
        "GEOSEARCH on an absent key must return an empty array"
    );

    h.stop().await;
}

// ===========================================================================
// Pub/Sub producer test
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn redis_pubsub_producer() {
    install_crypto_provider();
    let conn_str = shared_redis().await.to_string();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(RedisComponent::new())
        .build()
        .await;
    h.ctx()
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    // Subscriber-side receipt proof (rc-3ckqr): a raw redis pubsub client
    // subscribes BEFORE the route starts, then must receive the published
    // payload. Every step is deadline-bounded.
    let subscriber_client =
        redis::Client::open(format!("redis://{conn_str}")).expect("valid subscriber redis url");
    let mut subscriber = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        subscriber_client.get_async_pubsub(),
    )
    .await
    .expect("subscriber pubsub connect within 5s")
    .expect("subscriber pubsub connection");
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        subscriber.subscribe("mychannel"),
    )
    .await
    .expect("subscriber SUBSCRIBE within 5s")
    .expect("subscribe to mychannel succeeded");

    // redis-rs consumes the SUBSCRIBE ack inside subscribe(); the first
    // decoded payload on the stream is the published message. The
    // `if let Ok` guards non-UTF-8 payload conversion.
    let (payload_tx, payload_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let mut messages = subscriber.on_message();
        while let Some(msg) = messages.next().await {
            if let Ok(text) = msg.get_payload::<String>() {
                let _ = payload_tx.send(text);
                break;
            }
        }
    });

    // PUBLISH consumes the message BODY (commands/pubsub.rs
    // extract_publish_message) and CamelRedis.Channel selects the channel.
    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("CamelRedis.Channel", Value::String("mychannel".into()))
        .set_body("hello world")
        .to(format!("redis://{}?command=PUBLISH", conn_str))
        .to("mock:result")
        .route_id("redis-pubsub-producer-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    wait_until(
        "redis pubsub producer route delivery",
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(100),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    // The raw subscriber must receive the exact published payload.
    let received = tokio::time::timeout(std::time::Duration::from_secs(5), payload_rx)
        .await
        .expect("raw subscriber receipt within 5s")
        .expect("subscriber task delivered a payload");
    assert_eq!(received, "hello world");

    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    endpoint.assert_exchange_count(1).await;
}

// ===========================================================================
// Consumer queue mode test
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn redis_consumer_queue_mode() {
    install_crypto_provider();
    let conn_str = shared_redis().await.to_string();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(RedisComponent::new())
        .build()
        .await;
    h.ctx()
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let consumer_route = RouteBuilder::from(&format!(
        "redis://{}?command=BRPOP&key=queue-test&timeout=1",
        conn_str
    ))
    .to("mock:consumed")
    .route_id("redis-queue-consumer")
    .build()
    .unwrap();

    h.add_route(consumer_route).await.unwrap();

    let producer_route = RouteBuilder::from("timer:push?period=100&repeatCount=1")
        .set_header("CamelRedis.Key", Value::String("queue-test".into()))
        .set_body("queue-item")
        .to(format!("redis://{}?command=RPUSH", conn_str))
        .route_id("redis-queue-producer")
        .build()
        .unwrap();

    h.add_route(producer_route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("consumed").unwrap();
    wait_until(
        "redis queue consumer receives item",
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(100),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let exchanges = endpoint.get_received_exchanges().await;
    assert!(
        !exchanges.is_empty(),
        "Consumer should have received the item"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn redis_consumer_blpop_reads_left_side_first() {
    install_crypto_provider();
    let conn_str = shared_redis().await.to_string();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(RedisComponent::new())
        .build()
        .await;
    h.ctx()
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let consumer_route = RouteBuilder::from(&format!(
        "redis://{}?command=BLPOP&key=blpop-test&timeout=1",
        conn_str
    ))
    .to("mock:consumed")
    .route_id("redis-blpop-consumer")
    .build()
    .unwrap();

    // Seed queue before starting consumer so BLPOP side is observable.
    let client = redis::Client::open(format!("redis://{}", conn_str)).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let _: i64 = conn
        .rpush("blpop-test", vec!["item-a", "item-b"])
        .await
        .unwrap();

    h.add_route(consumer_route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("consumed").unwrap();
    wait_until(
        "redis BLPOP receives two items",
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(50),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(endpoint.get_received_exchanges().await.len() >= 2) }
        },
    )
    .await
    .unwrap();

    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let exchanges = endpoint.get_received_exchanges().await;
    assert_eq!(
        exchanges.len(),
        2,
        "BLPOP consumer should receive exactly two items"
    );

    let first = exchanges[0]
        .input
        .body
        .as_text()
        .map(|s| s.trim_matches('"').to_string());
    let second = exchanges[1]
        .input
        .body
        .as_text()
        .map(|s| s.trim_matches('"').to_string());
    assert_eq!(
        first.as_deref(),
        Some("item-a"),
        "BLPOP should pop left-most item first"
    );
    assert_eq!(second.as_deref(), Some("item-b"));
}

#[tokio::test(flavor = "multi_thread")]
async fn redis_consumer_brpop_reads_right_side_first() {
    install_crypto_provider();
    let conn_str = shared_redis().await.to_string();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(RedisComponent::new())
        .build()
        .await;
    h.ctx()
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let consumer_route = RouteBuilder::from(&format!(
        "redis://{}?command=BRPOP&key=brpop-test&timeout=1",
        conn_str
    ))
    .to("mock:consumed")
    .route_id("redis-brpop-consumer")
    .build()
    .unwrap();

    // Seed queue before starting consumer so BRPOP/BLPOP side is observable.
    let client = redis::Client::open(format!("redis://{}", conn_str)).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let _: i64 = conn
        .rpush("brpop-test", vec!["item-a", "item-b"])
        .await
        .unwrap();

    h.add_route(consumer_route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("consumed").unwrap();
    wait_until(
        "redis BRPOP receives two items",
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(50),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(endpoint.get_received_exchanges().await.len() >= 2) }
        },
    )
    .await
    .unwrap();

    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let exchanges = endpoint.get_received_exchanges().await;
    assert_eq!(
        exchanges.len(),
        2,
        "BRPOP consumer should receive exactly two items"
    );

    let first = exchanges[0]
        .input
        .body
        .as_text()
        .map(|s| s.trim_matches('"').to_string());
    let second = exchanges[1]
        .input
        .body
        .as_text()
        .map(|s| s.trim_matches('"').to_string());
    assert_eq!(
        first.as_deref(),
        Some("item-b"),
        "BRPOP should pop right-most item first"
    );
    assert_eq!(second.as_deref(), Some("item-a"));
}

// ===========================================================================
// Consumer pub/sub mode test
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn redis_consumer_pubsub_mode() {
    install_crypto_provider();
    let conn_str = shared_redis().await.to_string();

    let h = CamelTestContext::builder()
        .with_mock()
        .with_component(RedisComponent::new())
        .build()
        .await;
    h.ctx()
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    // Unique channel per invocation (uuid is not a camel-test dep): with
    // readiness now gated on the SUBSCRIBE ack, start() returning already
    // guarantees the subscription is registered server-side, so a publish
    // after start() is deterministic — no server-side subscription barrier
    // (rc-3ckqr). Uniqueness keeps a leftover subscriber from a previous
    // run off this channel.
    let channel = format!(
        "pubsub-race-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after unix epoch")
            .as_millis()
    );

    let consumer_route = RouteBuilder::from(&format!(
        "redis://{}?command=SUBSCRIBE&channels={channel}",
        conn_str
    ))
    .to("mock:received")
    .route_id("redis-pubsub-consumer")
    .build()
    .unwrap();

    h.add_route(consumer_route).await.unwrap();
    h.start().await;

    {
        let publish_client =
            redis::Client::open(format!("redis://{conn_str}")).expect("valid publish redis url");
        // R1 (ADR-0069 s13): every wait carries a deadline — connect included.
        let mut conn = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            publish_client.get_multiplexed_async_connection(),
        )
        .await
        .expect("publish redis connect within 5s")
        .expect("publish redis connection");

        // The producer PUBLISH path keeps dedicated coverage in the
        // redis_pubsub_producer test above.
        redis::cmd("PUBLISH")
            .arg(&channel)
            .arg("pubsub-message")
            .query_async::<i64>(&mut conn)
            .await
            .expect("test publish failed");
    }

    let endpoint = h.mock().get_endpoint("received").unwrap();
    wait_until(
        "redis pubsub receives message",
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(100),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    assert!(
        !endpoint.get_received_exchanges().await.is_empty(),
        "Subscriber should have received the message within 5s"
    );
}

// ===========================================================================
// PubSub startup fail-fast test
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn pubsub_startup_fails_fast_on_unreachable_broker() {
    install_crypto_provider();

    // Bounded retry budget (~200ms total): retry exhaustion, not the outer
    // 30s deadline, must terminate the failed startup.
    let config = RedisConfig::default().with_reconnect(NetworkRetryPolicy {
        enabled: true,
        max_attempts: 2,
        initial_delay: std::time::Duration::from_millis(100),
        multiplier: 1.0,
        max_delay: std::time::Duration::from_millis(100),
        jitter_factor: 0.0,
        max_attempts_absolute: None,
    });

    let h = CamelTestContext::builder()
        .with_mock()
        .with_component(RedisComponent::with_config(config))
        .build()
        .await;

    // SUBSCRIBE is consumer-only, so the failure exercises the consumer
    // startup path: retry exhaustion kills the consumer task, the
    // await_ready sender drops, and start() resolves Err. A producer-side
    // SUBSCRIBE would be rejected for the wrong reason.
    let consumer_route = RouteBuilder::from("redis://127.0.0.1:1?command=SUBSCRIBE&channels=dead")
        .to("mock:never")
        .route_id("redis-pubsub-unreachable-broker")
        .build()
        .unwrap();

    h.add_route(consumer_route).await.unwrap();

    // The harness start() .expect()s the context start result
    // (harness.rs:271-273), so the failure surfaces as a panic escaping the
    // start() future — catch it and assert it was observed in time.
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        AssertUnwindSafe(h.start()).catch_unwind(),
    )
    .await
    .expect("unreachable-broker startup must resolve within 30s, not hang");

    let payload = outcome.expect_err("start() must fail on an unreachable broker");
    assert!(
        payload.is::<String>() || payload.is::<&str>(),
        "expected the harness expect() panic, got a non-panic unwind payload"
    );
}

// ===========================================================================
// Kill-connection regression and health recovery
// ===========================================================================
//
// `CLIENT KILL TYPE normal` severs the multiplexed socket server-side, which
// is the only failure mode the crate's invalidation path cannot observe as a
// dial error: the driver may still hold a live-looking connection. These
// tests therefore run on DEDICATED containers — the shared fixture serves
// parallel tests whose connections the kill would also sever.

/// List key used by the kill-connection tests.
const KILL_KEY: &str = "killreg";

/// Starts a dedicated `redis:7-alpine` container for one kill test.
///
/// Deliberately not `shared_redis()`: `CLIENT KILL TYPE normal` disconnects
/// every client on the broker, including the connections of tests running in
/// parallel against the shared fixture.
async fn dedicated_redis() -> (ContainerAsync<Redis>, String) {
    let container = Redis::default()
        .with_tag("7-alpine")
        .start()
        .await
        .expect("dedicated Redis container failed to start");
    let port = container
        .get_host_port_ipv4(6379)
        .await
        .expect("dedicated Redis port not available");
    let conn_str = format!("127.0.0.1:{port}");
    eprintln!("dedicated Redis ready at: {conn_str}");
    (container, conn_str)
}

/// Producer for `LPUSH <KILL_KEY>` with a fresh, lazily connected executor.
fn kill_test_producer(conn_str: &str) -> RedisProducer {
    let uri = format!("redis://{conn_str}?command=LPUSH&key={KILL_KEY}");
    RedisProducer::new(
        RedisEndpointConfig::from_uri(&uri).expect("LPUSH endpoint config"),
        Arc::new(NoopRuntimeObservability),
    )
    .expect("LPUSH producer")
}

/// LPUSH `value` through the producer (the text body is the pushed value).
async fn lpush(producer: &mut RedisProducer, value: &str) -> Result<Exchange, CamelError> {
    producer
        .call(Exchange::new(Message::new(Body::Text(value.to_string()))))
        .await
}

/// Fresh admin connection to `conn_str`, bounded so a stalled dial fails the
/// test instead of hanging it.
async fn admin_connection(conn_str: &str) -> MultiplexedConnection {
    let client = redis::Client::open(format!("redis://{conn_str}")).expect("valid redis url");
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.get_multiplexed_async_connection(),
    )
    .await
    .expect("admin redis connect within 5s")
    .expect("admin redis connection")
}

/// `LLEN <KILL_KEY>` over a fresh admin connection.
async fn kill_key_len(conn_str: &str) -> i64 {
    let mut conn = admin_connection(conn_str).await;
    redis::cmd("LLEN")
        .arg(KILL_KEY)
        .query_async::<i64>(&mut conn)
        .await
        .expect("admin LLEN must succeed")
}

/// Kills every normal client of the dedicated broker (the caller excepted)
/// and returns the number of connections killed.
async fn client_kill_normal(conn: &mut MultiplexedConnection) -> i64 {
    redis::cmd("CLIENT")
        .arg("KILL")
        .arg("TYPE")
        .arg("normal")
        .query_async::<i64>(conn)
        .await
        .expect("CLIENT KILL TYPE normal must report the killed count")
}

#[tokio::test(flavor = "multi_thread")]
async fn client_kill_then_next_command_reconnects() {
    install_crypto_provider();
    let (_container, conn_str) = dedicated_redis().await;
    let mut producer = kill_test_producer(&conn_str);

    // Arrange: one command primes the cached connection, LLEN proves it
    // reached the server.
    lpush(&mut producer, "a")
        .await
        .expect("first LPUSH must succeed");
    assert_eq!(
        kill_key_len(&conn_str).await,
        1,
        "the primed connection must have applied 'a'"
    );

    // Act: sever the producer's socket server-side.
    let mut admin = admin_connection(&conn_str).await;
    let killed = client_kill_normal(&mut admin).await;
    assert!(
        killed >= 1,
        "CLIENT KILL TYPE normal must kill at least the producer's client, killed {killed}"
    );

    // The killed socket still looks cached: LPUSH is non-idempotent, so the
    // transport failure is rethrown unchanged and 'b' is never applied.
    let err = lpush(&mut producer, "b")
        .await
        .expect_err("non-idempotent command on a killed socket must not be retried into success");
    let msg = err.to_string();
    assert!(
        msg.contains("Redis LPUSH failed"),
        "the original transport failure must be rethrown at the LPUSH wrap site, got: {msg}"
    );

    // The failure invalidated the cache, so the next command dials lazily.
    lpush(&mut producer, "c")
        .await
        .expect("post-kill LPUSH must rebuild the connection");

    assert_eq!(
        kill_key_len(&conn_str).await,
        2,
        "'a' and 'c' must be the only applied pushes; the killed in-flight 'b' must be absent"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn health_recovers_after_client_kill() {
    install_crypto_provider();
    let (_container, conn_str) = dedicated_redis().await;
    let producer = kill_test_producer(&conn_str);

    // Arrange: a successful health check primes the cached connection.
    producer
        .check_connection()
        .await
        .expect("health check must succeed before the kill");

    // Act: sever the producer's socket server-side.
    let mut admin = admin_connection(&conn_str).await;
    let killed = client_kill_normal(&mut admin).await;
    assert!(
        killed >= 1,
        "CLIENT KILL TYPE normal must kill at least the producer's client, killed {killed}"
    );

    // Assert: the first check observes the dead socket and invalidates it;
    // the second one dials and succeeds.
    let first = producer
        .check_connection()
        .await
        .expect_err("the first health check after the kill must fail");
    assert!(
        first.to_string().contains("Redis health check PING failed"),
        "the kill failure must surface at the health-check wrap site, got: {first}"
    );
    producer
        .check_connection()
        .await
        .expect("the second health check must rebuild the connection and succeed");
}

// ===========================================================================
// Script commands: eval and fcall (redisfc 2.3)
// ===========================================================================

/// Producer for one endpoint URI, with a fresh lazily-connected executor.
fn redis_producer(uri: &str) -> RedisProducer {
    RedisProducer::new(
        RedisEndpointConfig::from_uri(uri).expect("endpoint config"),
        Arc::new(NoopRuntimeObservability),
    )
    .expect("producer")
}

/// One script exchange carrying `{"keys": [...], "args": [...]}`.
fn script_exchange(keys: &[&str], args: &[&str]) -> Exchange {
    Exchange::new(Message::new(Body::Json(json!({
        "keys": keys,
        "args": args,
    }))))
}

/// Encodes a script source for the `script=` query value.
fn encode_script(source: &str) -> String {
    percent_encoding::utf8_percent_encode(source, percent_encoding::NON_ALPHANUMERIC).to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn live_eval_script_roundtrip() {
    install_crypto_provider();
    let conn_str = shared_redis().await;

    let script = "redis.call('set',KEYS[1],ARGV[1]) return 1";
    let uri = format!(
        "redis://{conn_str}?command=eval&script={}",
        encode_script(script)
    );
    let mut producer = redis_producer(&uri);

    let out = producer
        .call(script_exchange(&["live:k"], &["v"]))
        .await
        .expect("the script sets the key and answers 1");
    assert_eq!(
        out.input.body,
        Body::Json(json!(1)),
        "the script reply maps to a JSON number"
    );

    let mut get = redis_producer(&format!("redis://{conn_str}?command=GET&key=live:k"));
    let got = get
        .call(Exchange::new(Message::new(Body::Empty)))
        .await
        .expect("the follow-up GET succeeds");
    assert_eq!(
        got.input.body,
        Body::Text("v".into()),
        "the script's write is visible to a later GET"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn live_fcall_function_roundtrip() {
    install_crypto_provider();
    let conn_str = shared_redis().await;

    let dir = tempfile::tempdir().expect("create the library temp dir");
    let library = dir.path().join("echo_rt.lua");
    let source = "#!lua name=echo_rt\n\
                  redis.register_function('echo_rt', function(keys, args) return args[1] end)\n";
    std::fs::write(&library, source).expect("write the function library");

    let uri = format!(
        "redis://{conn_str}?command=fcall&function=echo_rt&scriptFile={}",
        encode_script(&library.display().to_string())
    );
    let mut producer = redis_producer(&uri);

    let out = producer
        .call(script_exchange(&[], &["hi"]))
        .await
        .expect("the first-use load registers the function and it answers");
    assert_eq!(
        out.input.body,
        Body::Text("hi".into()),
        "the function returns its first argument"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn live_fcall_missing_function_recovery() {
    install_crypto_provider();
    let conn_str = shared_redis().await;

    let dir = tempfile::tempdir().expect("create the library temp dir");
    let library = dir.path().join("echo_rec.lua");
    let source = "#!lua name=echo_rec\n\
                  redis.register_function('echo_rec', function(keys, args) return args[1] end)\n";
    std::fs::write(&library, source).expect("write the function library");

    let uri = format!(
        "redis://{conn_str}?command=fcall&function=echo_rec&scriptFile={}",
        encode_script(&library.display().to_string())
    );
    let mut producer = redis_producer(&uri);

    producer
        .call(script_exchange(&[], &["first"]))
        .await
        .expect("the first exchange loads the library and invokes the function");

    // Evict every library, as a restart would: the next invocation must see
    // the function absent in FUNCTION LIST, reload, and re-invoke once.
    let mut admin = admin_connection(conn_str).await;
    redis::cmd("FUNCTION")
        .arg("FLUSH")
        .query_async::<redis::Value>(&mut admin)
        .await
        .expect("FUNCTION FLUSH must evict the libraries");

    let out = producer
        .call(script_exchange(&[], &["again"]))
        .await
        .expect("the provenance pair recovers the missing function");
    assert_eq!(
        out.input.body,
        Body::Text("again".into()),
        "the re-invocation's reply is the exchange result"
    );
}

// ===========================================================================
// Batch commands: multi (redisfc 3.3)
// ===========================================================================

/// One batch exchange: `entries` as `CamelRedis.Batch`, `body` as the
/// index-aligned value array.
fn batch_exchange(entries: serde_json::Value, body: serde_json::Value) -> Exchange {
    let mut message = Message::new(Body::Json(body));
    message.set_header("CamelRedis.Batch", entries);
    Exchange::new(message)
}

#[tokio::test(flavor = "multi_thread")]
async fn live_multi_atomic_batch_roundtrip() {
    install_crypto_provider();
    let conn_str = shared_redis().await;

    let mut producer = redis_producer(&format!("redis://{conn_str}?command=multi"));
    let exchange = batch_exchange(
        json!([
            {"command": "set", "key": "live:b1"},
            {"command": "incr", "key": "live:b2"},
        ]),
        json!(["v1", null]),
    );

    let out = producer
        .call(exchange)
        .await
        .expect("the atomic batch applies both entries");

    let Body::Json(replies) = &out.input.body else {
        panic!("a batch reply is a JSON array, got {:?}", out.input.body);
    };
    assert_eq!(
        replies.as_array().map(Vec::len),
        Some(2),
        "the EXEC array carries one reply per entry, got {replies}"
    );

    let mut get_b1 = redis_producer(&format!("redis://{conn_str}?command=GET&key=live:b1"));
    let b1 = get_b1
        .call(Exchange::new(Message::new(Body::Empty)))
        .await
        .expect("the follow-up GET of the SET key succeeds");
    assert_eq!(
        b1.input.body,
        Body::Text("v1".into()),
        "entry 0's SET applied inside the batch"
    );

    let mut get_b2 = redis_producer(&format!("redis://{conn_str}?command=GET&key=live:b2"));
    let b2 = get_b2
        .call(Exchange::new(Message::new(Body::Empty)))
        .await
        .expect("the follow-up GET of the INCR key succeeds");
    assert_eq!(
        b2.input.body,
        Body::Text("1".into()),
        "entry 1's INCR applied inside the batch"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn live_multi_partial_entry_error_is_named() {
    install_crypto_provider();
    let conn_str = shared_redis().await;

    // Arrange: `live:b3` holds a list, so INCR fails with WRONGTYPE inside
    // the transaction (a per-entry error, not a queue-time rejection).
    let mut admin = admin_connection(conn_str).await;
    redis::cmd("LPUSH")
        .arg("live:b3")
        .arg("x")
        .query_async::<i64>(&mut admin)
        .await
        .expect("LPUSH must make live:b3 a list");

    let mut producer = redis_producer(&format!("redis://{conn_str}?command=multi"));
    let exchange = batch_exchange(
        json!([{"command": "incr", "key": "live:b3"}]),
        json!([null]),
    );

    let err = producer
        .call(exchange)
        .await
        .expect_err("INCR on a list key is a WRONGTYPE entry error");

    let message = err.to_string();
    assert!(
        message.contains("entry 0"),
        "the error must name the failing entry index, got: {message}"
    );
    assert!(
        message.contains("WRONGTYPE"),
        "the error must carry the server error text, got: {message}"
    );
}

// ===========================================================================
// Live bitmap, HyperLogLog, and WAIT commands (redisbmp)
// ===========================================================================

/// Harness with one `direct:{id}` → `redis://…?command={command}` route per
/// entry, so a test addresses each Redis command by endpoint id.
async fn live_redis_harness(conn_str: &str, routes: &[(&str, &str)]) -> CamelTestContext {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_component(RedisComponent::new())
        .build()
        .await;

    for (id, command) in routes {
        let route = RouteBuilder::from(&format!("direct:{id}"))
            .to(format!("redis://{conn_str}?command={command}"))
            .route_id(format!("redis-live-{id}"))
            .build()
            .unwrap();
        h.add_route(route).await.unwrap();
    }
    h.start().await;
    h
}

/// Sends one exchange to a `direct:` endpoint and returns the whole reply
/// exchange, so callers can read output headers (`CamelRedis.Cursor`,
/// `CamelRedis.ScanComplete`) as well as the body.
///
/// The round-trip is bounded by a 10 s deadline: a Redis command is a
/// single request/response, `send_to_direct` adds only its own short
/// not-registered retry budget, so anything slower means the harness or
/// the container is wedged and the test should fail fast instead of
/// hanging the lane.
async fn live_redis_reply_full(
    h: &CamelTestContext,
    direct_uri: &str,
    headers: &[(&str, Value)],
    body: Body,
) -> Exchange {
    let mut ex = Exchange::new(Message::new(body));
    for (name, value) in headers {
        ex.input.set_header(*name, value.clone());
    }
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        send_to_direct(h, direct_uri, ex),
    )
    .await
    .expect("redis command round-trip must return within 10s")
    .expect("redis command must succeed")
}

/// Sends one exchange to a `direct:` endpoint and returns its JSON reply body.
async fn live_redis_reply(
    h: &CamelTestContext,
    direct_uri: &str,
    headers: &[(&str, Value)],
    body: Body,
) -> serde_json::Value {
    let reply = live_redis_reply_full(h, direct_uri, headers, body).await;
    match reply.input.body {
        Body::Json(v) => v,
        other => panic!("redis reply body must be JSON, got {other:?}"),
    }
}

/// Borrows a reply exchange's JSON body, failing loudly on a non-JSON body.
fn live_redis_json_body(reply: &Exchange) -> &serde_json::Value {
    match &reply.input.body {
        Body::Json(v) => v,
        other => panic!("redis reply body must be JSON, got {other:?}"),
    }
}

/// Reads the `CamelRedis.Cursor` output header every SCAN family page carries.
fn live_redis_cursor(reply: &Exchange) -> u64 {
    let value = reply
        .input
        .header("CamelRedis.Cursor")
        .unwrap_or_else(|| panic!("every SCAN family reply carries CamelRedis.Cursor"));
    value
        .as_u64()
        .unwrap_or_else(|| panic!("CamelRedis.Cursor must be a whole number, got {value}"))
}

/// Seeds `keys` with a placeholder string value over an admin connection.
async fn seed_string_keys(conn: &mut MultiplexedConnection, keys: &[String]) {
    let mut pipe = redis::pipe();
    for key in keys {
        pipe.cmd("SET").arg(key).arg("1").ignore();
    }
    pipe.query_async::<()>(conn)
        .await
        .expect("admin SET pipeline must seed the keys");
}

/// UNLINKs `keys` over an admin connection (best-effort test cleanup: the
/// shared container keeps every other test's keys).
async fn unlink_keys(conn: &mut MultiplexedConnection, keys: &[String]) {
    let mut pipe = redis::pipe();
    for key in keys {
        pipe.cmd("UNLINK").arg(key).ignore();
    }
    pipe.query_async::<()>(conn)
        .await
        .expect("admin UNLINK pipeline must remove the seeded keys");
}

#[tokio::test(flavor = "multi_thread")]
async fn redis_live_setbit_getbit_roundtrip() {
    install_crypto_provider();
    let conn_str = shared_redis().await.to_string();
    let h = live_redis_harness(
        &conn_str,
        &[("live-setbit", "SETBIT"), ("live-getbit", "GETBIT")],
    )
    .await;

    let key = "live:setbit:roundtrip";
    let previous = live_redis_reply(
        &h,
        "direct:live-setbit",
        &[
            ("CamelRedis.Key", json!(key)),
            ("CamelRedis.BitOffset", json!(7u64)),
            ("CamelRedis.BitValue", json!(1u64)),
        ],
        Body::Empty,
    )
    .await;
    assert_eq!(
        previous,
        json!(0),
        "SETBIT of a clear bit reports the previous bit 0"
    );

    let bit = live_redis_reply(
        &h,
        "direct:live-getbit",
        &[
            ("CamelRedis.Key", json!(key)),
            ("CamelRedis.BitOffset", json!(7u64)),
        ],
        Body::Empty,
    )
    .await;
    assert_eq!(
        bit,
        json!(1),
        "GETBIT reads back the bit SETBIT wrote to the wire"
    );

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn redis_live_bitcount_bit_range() {
    install_crypto_provider();
    let conn_str = shared_redis().await.to_string();
    let h = live_redis_harness(
        &conn_str,
        &[("live-setbit", "SETBIT"), ("live-bitcount", "BITCOUNT")],
    )
    .await;

    let key = "live:bitcount:range";
    live_redis_reply(
        &h,
        "direct:live-setbit",
        &[
            ("CamelRedis.Key", json!(key)),
            ("CamelRedis.BitOffset", json!(7u64)),
            ("CamelRedis.BitValue", json!(1u64)),
        ],
        Body::Empty,
    )
    .await;

    // Only offset 7 is set, and the inclusive BIT range 7..=7 covers exactly
    // that one bit.
    let count = live_redis_reply(
        &h,
        "direct:live-bitcount",
        &[
            ("CamelRedis.Key", json!(key)),
            ("CamelRedis.Start", json!(7i64)),
            ("CamelRedis.End", json!(7i64)),
            ("CamelRedis.BitRangeUnit", json!("BIT")),
        ],
        Body::Empty,
    )
    .await;
    assert_eq!(
        count,
        json!(1),
        "BITCOUNT over bit range 7..7 counts the single set bit"
    );

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn redis_live_bitop_not_single_key() {
    install_crypto_provider();
    let conn_str = shared_redis().await.to_string();
    let h = live_redis_harness(
        &conn_str,
        &[
            ("live-setbit", "SETBIT"),
            ("live-bitop", "BITOP"),
            ("live-getbit", "GETBIT"),
        ],
    )
    .await;

    let src = "live:bitop:not:src";
    let dst = "live:bitop:not:dst";
    live_redis_reply(
        &h,
        "direct:live-setbit",
        &[
            ("CamelRedis.Key", json!(src)),
            ("CamelRedis.BitOffset", json!(7u64)),
            ("CamelRedis.BitValue", json!(1u64)),
        ],
        Body::Empty,
    )
    .await;

    let length = live_redis_reply(
        &h,
        "direct:live-bitop",
        &[
            ("CamelRedis.Destination", json!(dst)),
            ("CamelRedis.BitOperation", json!("NOT")),
            ("CamelRedis.Keys", json!([src])),
        ],
        Body::Empty,
    )
    .await;
    assert_eq!(
        length,
        json!(1),
        "BITOP NOT reports the destination length in bytes; the one-byte source negates to one byte"
    );

    // Redis numbers bits MSB-first, so SETBIT offset 7 sets the least
    // significant bit: the source byte is 0x01 and NOT 0x01 is 0xfe. Bit 0
    // is set in the destination, bit 7 is clear.
    let head = live_redis_reply(
        &h,
        "direct:live-getbit",
        &[
            ("CamelRedis.Key", json!(dst)),
            ("CamelRedis.BitOffset", json!(0u64)),
        ],
        Body::Empty,
    )
    .await;
    assert_eq!(head, json!(1), "BITOP NOT inverts bit 0 of the source");
    let tail = live_redis_reply(
        &h,
        "direct:live-getbit",
        &[
            ("CamelRedis.Key", json!(dst)),
            ("CamelRedis.BitOffset", json!(7u64)),
        ],
        Body::Empty,
    )
    .await;
    assert_eq!(tail, json!(0), "BITOP NOT clears the source's set bit 7");

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn redis_live_bitfield_overflow_sat_reply_array() {
    install_crypto_provider();
    let conn_str = shared_redis().await.to_string();
    let h = live_redis_harness(&conn_str, &[("live-bitfield", "BITFIELD")]).await;

    let key = "live:bitfield:sat";
    let reply = live_redis_reply(
        &h,
        "direct:live-bitfield",
        &[
            ("CamelRedis.Key", json!(key)),
            ("CamelRedis.BitFieldOverflow", json!("SAT")),
            (
                "CamelRedis.BitFieldOps",
                json!(["SET u8 #0 255", "INCRBY u8 #0 1", "GET u8 #0"]),
            ),
        ],
        Body::Empty,
    )
    .await;
    assert_eq!(
        reply,
        json!([0, 255, 255]),
        "SET reports the old value 0, INCRBY saturates to 255, GET reads back 255"
    );

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn redis_live_pfcount_multi_key() {
    install_crypto_provider();
    let conn_str = shared_redis().await.to_string();
    let h = live_redis_harness(
        &conn_str,
        &[("live-pfadd", "PFADD"), ("live-pfcount", "PFCOUNT")],
    )
    .await;

    let h1 = "live:pfcount:h1";
    let h2 = "live:pfcount:h2";
    for (key, element) in [(h1, "alpha"), (h2, "beta")] {
        let changed = live_redis_reply(
            &h,
            "direct:live-pfadd",
            &[("CamelRedis.Key", json!(key))],
            Body::Text(element.into()),
        )
        .await;
        assert_eq!(
            changed,
            json!(1),
            "PFADD of a fresh element changes the structure"
        );
    }

    let cardinality = live_redis_reply(
        &h,
        "direct:live-pfcount",
        &[("CamelRedis.Keys", json!([h1, h2]))],
        Body::Empty,
    )
    .await;
    let cardinality = cardinality
        .as_u64()
        .unwrap_or_else(|| panic!("PFCOUNT body must be a JSON number, got {cardinality}"));
    assert!(
        cardinality >= 2,
        "PFCOUNT over two disjoint keys counts at least two members, got {cardinality}"
    );

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn redis_live_wait_reply_body() {
    install_crypto_provider();
    let conn_str = shared_redis().await.to_string();
    let h = live_redis_harness(&conn_str, &[("live-wait", "WAIT")]).await;

    let reply = live_redis_reply(
        &h,
        "direct:live-wait",
        &[
            ("CamelRedis.NumReplicas", json!(1u64)),
            ("CamelRedis.WaitTimeout", json!(1000u64)),
        ],
        Body::Empty,
    )
    .await;
    let acked = reply
        .as_u64()
        .unwrap_or_else(|| panic!("WAIT body must be a JSON number, got {reply}"));
    assert!(
        acked <= 1,
        "a single-node container acks for at most one replica, got {acked}"
    );

    h.stop().await;
}

// ===========================================================================
// Live SCAN family (rediscan)
// ===========================================================================

/// Upper bound on pages a live SCAN walk may take: real walks stay far below
/// it, so hitting it means the walk never terminates.
const MAX_SCAN_PAGES: usize = 100_000;

#[tokio::test(flavor = "multi_thread")]
async fn redis_live_scan_multipage_match_count() {
    install_crypto_provider();
    let conn_str = shared_redis().await.to_string();
    let mut admin = admin_connection(&conn_str).await;

    let seeded: Vec<String> = (0..250).map(|i| format!("scan:it:{i}")).collect();
    seed_string_keys(&mut admin, &seeded).await;

    let h = live_redis_harness(&conn_str, &[("live-scan", "SCAN")]).await;

    let mut cursor: Option<u64> = None;
    let mut pages = 0usize;
    let mut forwarded: Vec<String> = Vec::new();
    loop {
        // allow-test-wait: each page round-trip is bounded by live_redis_reply_full's 10s deadline
        let mut headers: Vec<(&str, Value)> = vec![
            ("CamelRedis.Pattern", json!("scan:it:*")),
            ("CamelRedis.Count", json!(10u64)),
        ];
        if let Some(cursor) = cursor {
            headers.push(("CamelRedis.Cursor", json!(cursor)));
        }
        let reply = live_redis_reply_full(&h, "direct:live-scan", &headers, Body::Empty).await;
        pages += 1;
        assert!(
            pages <= MAX_SCAN_PAGES,
            "SCAN walk exceeded {MAX_SCAN_PAGES} pages"
        );

        let body = live_redis_json_body(&reply);
        let page = body
            .as_array()
            .unwrap_or_else(|| panic!("a SCAN page body is a JSON array of keys, got {body}"));
        for element in page {
            let key = element
                .as_str()
                .unwrap_or_else(|| panic!("a SCAN page element is a key string, got {element}"));
            assert!(
                key.starts_with("scan:it:"),
                "MATCH scan:it:* must filter every page element, got {key}"
            );
            forwarded.push(key.to_string());
        }

        let next = live_redis_cursor(&reply);
        if reply.input.header("CamelRedis.ScanComplete").is_some() {
            assert_eq!(
                reply.input.header("CamelRedis.ScanComplete"),
                Some(&json!(true)),
                "ScanComplete on the final page is the JSON boolean true"
            );
            assert_eq!(next, 0, "the final page reports cursor 0");
            break;
        }
        assert_ne!(next, 0, "a non-final page carries a non-zero cursor");
        cursor = Some(next);
    }

    let seen: HashSet<&str> = forwarded.iter().map(String::as_str).collect();
    for key in &seeded {
        assert!(
            seen.contains(key.as_str()),
            "full coverage: {key} must appear in some page (saw {} elements over {pages} pages)",
            forwarded.len()
        );
    }
    assert!(
        pages > 1,
        "COUNT 10 over 250 keys must take several pages, got {pages}"
    );

    h.stop().await;
    unlink_keys(&mut admin, &seeded).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn redis_live_hscan_map_shape() {
    install_crypto_provider();
    let conn_str = shared_redis().await.to_string();
    let mut admin = admin_connection(&conn_str).await;

    // 600 fields exceed `hash-max-listpack-entries`, so the hash is
    // hashtable-encoded and COUNT 1 walks it over several pages. The default
    // is 128 up to Redis 7.2 and 512 from 7.4, so 600 stays above both.
    let key = "scan:it:hash".to_string();
    let seeded: HashMap<String, String> = (0..600)
        .map(|i| (format!("f{i}"), format!("v{i}")))
        .collect();
    {
        let mut pipe = redis::pipe();
        for (field, value) in &seeded {
            pipe.cmd("HSET").arg(&key).arg(field).arg(value).ignore();
        }
        pipe.query_async::<()>(&mut admin)
            .await
            .expect("admin HSET pipeline must seed the hash");
    }

    let h = live_redis_harness(&conn_str, &[("live-hscan", "HSCAN")]).await;

    let mut cursor: Option<u64> = None;
    let mut pages = 0usize;
    let mut union: HashMap<String, String> = HashMap::new();
    loop {
        // allow-test-wait: each page round-trip is bounded by live_redis_reply_full's 10s deadline
        let mut headers: Vec<(&str, Value)> = vec![
            ("CamelRedis.Key", json!(key)),
            ("CamelRedis.Count", json!(1u64)),
        ];
        if let Some(cursor) = cursor {
            headers.push(("CamelRedis.Cursor", json!(cursor)));
        }
        let reply = live_redis_reply_full(&h, "direct:live-hscan", &headers, Body::Empty).await;
        pages += 1;
        assert!(
            pages <= MAX_SCAN_PAGES,
            "HSCAN walk exceeded {MAX_SCAN_PAGES} pages"
        );

        let body = live_redis_json_body(&reply);
        let page = body.as_object().unwrap_or_else(|| {
            panic!("an HSCAN page body is a JSON field/value object, got {body}")
        });
        for (field, value) in page {
            let value = value
                .as_str()
                .unwrap_or_else(|| panic!("an HSCAN page value is a string, got {value}"));
            let expected = seeded
                .get(field)
                .unwrap_or_else(|| panic!("HSCAN returned field {field} that was never seeded"));
            assert_eq!(
                expected, value,
                "HSCAN page entries are a subset of the seeded pairs"
            );
            union.insert(field.clone(), value.to_string());
        }

        let next = live_redis_cursor(&reply);
        if reply.input.header("CamelRedis.ScanComplete").is_some() {
            assert_eq!(
                reply.input.header("CamelRedis.ScanComplete"),
                Some(&json!(true))
            );
            assert_eq!(next, 0, "the final page reports cursor 0");
            break;
        }
        assert_ne!(next, 0, "a non-final page carries a non-zero cursor");
        cursor = Some(next);
    }

    assert_eq!(
        union, seeded,
        "the union of HSCAN pages must equal all 600 seeded field/value pairs"
    );
    assert!(
        pages > 1,
        "COUNT 1 over 600 hashtable fields must take several pages, got {pages}"
    );

    h.stop().await;
    unlink_keys(&mut admin, std::slice::from_ref(&key)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn redis_live_zscan_pair_shape() {
    install_crypto_provider();
    let conn_str = shared_redis().await.to_string();
    let mut admin = admin_connection(&conn_str).await;

    // 200 members exceed the sorted-set small-collection encodings, so the
    // zset is skiplist-encoded and COUNT 1 walks it over several pages.
    let key = "scan:it:zset".to_string();
    let seeded: HashMap<String, f64> = (0..200)
        .map(|i| (format!("m{i}"), i as f64 + 1.0))
        .collect();
    {
        let mut pipe = redis::pipe();
        for (member, score) in &seeded {
            pipe.cmd("ZADD").arg(&key).arg(*score).arg(member).ignore();
        }
        pipe.query_async::<()>(&mut admin)
            .await
            .expect("admin ZADD pipeline must seed the sorted set");
    }

    let h = live_redis_harness(&conn_str, &[("live-zscan", "ZSCAN")]).await;

    let mut cursor: Option<u64> = None;
    let mut pages = 0usize;
    let mut union: HashMap<String, f64> = HashMap::new();
    loop {
        // allow-test-wait: each page round-trip is bounded by live_redis_reply_full's 10s deadline
        let mut headers: Vec<(&str, Value)> = vec![
            ("CamelRedis.Key", json!(key)),
            ("CamelRedis.Count", json!(1u64)),
        ];
        if let Some(cursor) = cursor {
            headers.push(("CamelRedis.Cursor", json!(cursor)));
        }
        let reply = live_redis_reply_full(&h, "direct:live-zscan", &headers, Body::Empty).await;
        pages += 1;
        assert!(
            pages <= MAX_SCAN_PAGES,
            "ZSCAN walk exceeded {MAX_SCAN_PAGES} pages"
        );

        let body = live_redis_json_body(&reply);
        let page = body.as_array().unwrap_or_else(|| {
            panic!("a ZSCAN page body is a JSON array of member/score objects, got {body}")
        });
        for element in page {
            let pair = element
                .as_object()
                .unwrap_or_else(|| panic!("a ZSCAN page element is an object, got {element}"));
            assert_eq!(
                pair.len(),
                2,
                "a ZSCAN page element carries exactly member and score, got {pair:?}"
            );
            let member = pair
                .get("member")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| panic!("a ZSCAN element names its member, got {element}"));
            let score = pair
                .get("score")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or_else(|| {
                    panic!("a ZSCAN element carries a numeric score, got {element}")
                });
            let expected = seeded
                .get(member)
                .unwrap_or_else(|| panic!("ZSCAN returned member {member} that was never seeded"));
            assert_eq!(
                *expected, score,
                "ZSCAN reports member {member} with its exact seeded score"
            );
            union.insert(member.to_string(), score);
        }

        let next = live_redis_cursor(&reply);
        if reply.input.header("CamelRedis.ScanComplete").is_some() {
            assert_eq!(
                reply.input.header("CamelRedis.ScanComplete"),
                Some(&json!(true))
            );
            assert_eq!(next, 0, "the final page reports cursor 0");
            break;
        }
        assert_ne!(next, 0, "a non-final page carries a non-zero cursor");
        cursor = Some(next);
    }

    assert_eq!(
        union, seeded,
        "the union of ZSCAN pages must cover all 200 members with exact scores"
    );
    assert!(
        pages > 1,
        "COUNT 1 over 200 skiplist members must take several pages, got {pages}"
    );

    h.stop().await;
    unlink_keys(&mut admin, std::slice::from_ref(&key)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn redis_live_sscan_member_array_body() {
    install_crypto_provider();
    let conn_str = shared_redis().await.to_string();
    let mut admin = admin_connection(&conn_str).await;

    // 200 members exceed `set-max-listpack-entries` (128), so the set is
    // hashtable-encoded and COUNT 1 walks it over several pages.
    let key = "scan:it:set".to_string();
    let seeded: HashSet<String> = (0..200).map(|i| format!("m{i}")).collect();
    {
        let mut pipe = redis::pipe();
        for member in &seeded {
            pipe.cmd("SADD").arg(&key).arg(member).ignore();
        }
        pipe.query_async::<()>(&mut admin)
            .await
            .expect("admin SADD pipeline must seed the set");
    }

    let h = live_redis_harness(&conn_str, &[("live-sscan", "SSCAN")]).await;

    let mut cursor: Option<u64> = None;
    let mut pages = 0usize;
    let mut union: HashSet<String> = HashSet::new();
    loop {
        // allow-test-wait: each page round-trip is bounded by live_redis_reply_full's 10s deadline
        let mut headers: Vec<(&str, Value)> = vec![
            ("CamelRedis.Key", json!(key)),
            ("CamelRedis.Count", json!(1u64)),
        ];
        if let Some(cursor) = cursor {
            headers.push(("CamelRedis.Cursor", json!(cursor)));
        }
        let reply = live_redis_reply_full(&h, "direct:live-sscan", &headers, Body::Empty).await;
        pages += 1;
        assert!(
            pages <= MAX_SCAN_PAGES,
            "SSCAN walk exceeded {MAX_SCAN_PAGES} pages"
        );

        let body = live_redis_json_body(&reply);
        let page = body.as_array().unwrap_or_else(|| {
            panic!("an SSCAN page body is a JSON array of member strings, got {body}")
        });
        for element in page {
            let member = element.as_str().unwrap_or_else(|| {
                panic!("an SSCAN page element is a member string, got {element}")
            });
            assert!(
                seeded.contains(member),
                "SSCAN returned member {member} that was never seeded"
            );
            union.insert(member.to_string());
        }

        let next = live_redis_cursor(&reply);
        if reply.input.header("CamelRedis.ScanComplete").is_some() {
            assert_eq!(
                reply.input.header("CamelRedis.ScanComplete"),
                Some(&json!(true))
            );
            assert_eq!(next, 0, "the final page reports cursor 0");
            break;
        }
        assert_ne!(next, 0, "a non-final page carries a non-zero cursor");
        cursor = Some(next);
    }

    assert_eq!(
        union, seeded,
        "the union of SSCAN pages must cover all 200 seeded members"
    );
    assert!(
        pages > 1,
        "COUNT 1 over 200 hashtable members must take several pages, got {pages}"
    );

    h.stop().await;
    unlink_keys(&mut admin, std::slice::from_ref(&key)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn redis_live_scan_repeats_across_pages_full_coverage() {
    install_crypto_provider();
    let conn_str = shared_redis().await.to_string();
    let mut admin = admin_connection(&conn_str).await;

    let seeded: Vec<String> = (0..30).map(|i| format!("scan:it:repeat:{i}")).collect();
    seed_string_keys(&mut admin, &seeded).await;

    let h = live_redis_harness(&conn_str, &[("live-scan-repeat", "SCAN")]).await;

    // The loop condition is the ABSENCE of CamelRedis.ScanComplete, never body
    // emptiness: an empty (MATCH-filtered) page structurally continues the
    // iteration.
    let mut cursor: Option<u64> = None;
    let mut pages = 0usize;
    let mut empty_pages = 0usize;
    let mut forwarded: Vec<String> = Vec::new();
    loop {
        // allow-test-wait: each page round-trip is bounded by live_redis_reply_full's 10s deadline
        let mut headers: Vec<(&str, Value)> = vec![
            ("CamelRedis.Pattern", json!("scan:it:repeat:*")),
            ("CamelRedis.Count", json!(1u64)),
        ];
        if let Some(cursor) = cursor {
            headers.push(("CamelRedis.Cursor", json!(cursor)));
        }
        let reply =
            live_redis_reply_full(&h, "direct:live-scan-repeat", &headers, Body::Empty).await;
        pages += 1;
        assert!(
            pages <= MAX_SCAN_PAGES,
            "SCAN walk exceeded {MAX_SCAN_PAGES} pages"
        );

        let body = live_redis_json_body(&reply);
        let page = body
            .as_array()
            .unwrap_or_else(|| panic!("a SCAN page body is a JSON array of keys, got {body}"));
        if page.is_empty() {
            empty_pages += 1;
        }
        for element in page {
            let key = element
                .as_str()
                .unwrap_or_else(|| panic!("a SCAN page element is a key string, got {element}"));
            assert!(
                key.starts_with("scan:it:repeat:"),
                "MATCH scan:it:repeat:* must filter every page element, got {key}"
            );
            forwarded.push(key.to_string());
        }

        let next = live_redis_cursor(&reply);
        if reply.input.header("CamelRedis.ScanComplete").is_some() {
            assert_eq!(
                reply.input.header("CamelRedis.ScanComplete"),
                Some(&json!(true))
            );
            assert_eq!(next, 0, "the final page reports cursor 0");
            break;
        }
        assert_ne!(next, 0, "a non-final page carries a non-zero cursor");
        cursor = Some(next);
    }

    let unique: HashSet<&str> = forwarded.iter().map(String::as_str).collect();
    for key in &seeded {
        assert!(
            unique.contains(key.as_str()),
            "full coverage: {key} must appear in some page (saw {} elements over {pages} pages, {empty_pages} empty)",
            forwarded.len()
        );
    }
    // Page bodies are forwarded verbatim: no component-side dedup or
    // filtering, so the forwarded total is at least one occurrence per key
    // (it exceeds it when the server repeats a key across pages).
    assert!(
        forwarded.len() >= seeded.len(),
        "page bodies forwarded verbatim: got {} elements for {} seeded keys",
        forwarded.len(),
        seeded.len()
    );
    assert!(
        pages > 1,
        "COUNT 1 over 30 keys must take several pages, got {pages}"
    );

    h.stop().await;
    unlink_keys(&mut admin, &seeded).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn redis_live_keys_unchanged_frame() {
    install_crypto_provider();
    let conn_str = shared_redis().await.to_string();
    let mut admin = admin_connection(&conn_str).await;

    let seeded: Vec<String> = vec!["keys:it:a".to_string(), "keys:it:b".to_string()];
    seed_string_keys(&mut admin, &seeded).await;

    let h = live_redis_harness(&conn_str, &[("live-keys", "KEYS")]).await;

    let reply = live_redis_reply_full(
        &h,
        "direct:live-keys",
        &[("CamelRedis.Pattern", json!("keys:it:*"))],
        Body::Empty,
    )
    .await;

    let body = live_redis_json_body(&reply);
    let keys = body
        .as_array()
        .unwrap_or_else(|| panic!("a KEYS body is the full JSON key array, got {body}"));
    let mut returned: Vec<&str> = keys
        .iter()
        .map(|key| {
            key.as_str()
                .unwrap_or_else(|| panic!("a KEYS element is a key string, got {key}"))
        })
        .collect();
    returned.sort_unstable();
    assert_eq!(
        returned,
        vec!["keys:it:a", "keys:it:b"],
        "KEYS returns every matching key in one exchange"
    );
    assert!(
        reply.input.header("CamelRedis.Cursor").is_none(),
        "KEYS must not emit a CamelRedis.Cursor header"
    );
    assert!(
        reply.input.header("CamelRedis.ScanComplete").is_none(),
        "KEYS must not emit a CamelRedis.ScanComplete header"
    );

    h.stop().await;
    unlink_keys(&mut admin, &seeded).await;
}
