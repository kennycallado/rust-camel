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
use camel_api::{Body, Exchange, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::NetworkRetryPolicy;
use camel_component_redis::{RedisComponent, RedisConfig};
use camel_test::CamelTestContext;
use futures::{FutureExt, StreamExt};
use redis::AsyncCommands;
use serde_json::json;
use std::panic::AssertUnwindSafe;
use support::send_to_direct;
use support::wait::wait_until;

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
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("CamelRedis.Key", Value::String("testkey".into()))
        .set_header("CamelRedis.Value", Value::String("testvalue".into()))
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
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("CamelRedis.Key", Value::String("mylist".into()))
        .set_header("CamelRedis.Value", Value::String("item1".into()))
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
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("CamelRedis.Key", Value::String("myhash".into()))
        .set_header("CamelRedis.Field", Value::String("field1".into()))
        .set_header("CamelRedis.Value", Value::String("value1".into()))
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
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("CamelRedis.Key", Value::String("myset".into()))
        .set_header("CamelRedis.Value", Value::String("member1".into()))
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
async fn geo_exchange(
    h: &CamelTestContext,
    direct_uri: &str,
    headers: &[(&str, Value)],
) -> serde_json::Value {
    let mut ex = Exchange::default();
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

    // One direct route per GEO command: every option (members, unit, radius
    // or box, WithDist/WithCoord) rides on the exchange as CamelRedis.*
    // headers, so the routes only pin the redis command.
    for (id, command) in [
        ("geo-add", "GEOADD"),
        ("geo-pos", "GEOPOS"),
        ("geo-dist", "GEODIST"),
        ("geo-hash", "GEOHASH"),
        ("geo-search", "GEOSEARCH"),
    ] {
        let route = RouteBuilder::from(&format!("direct:{id}"))
            .to(format!("redis://{conn_str}?command={command}"))
            .route_id(&format!("redis-{id}"))
            .build()
            .unwrap();
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    // ── GEOADD ──
    let added = geo_exchange(
        &h,
        "direct:geo-add",
        &[
            ("CamelRedis.Key", json!("geo:points")),
            ("CamelRedis.Longitude", json!(13.361389)),
            ("CamelRedis.Latitude", json!(38.115556)),
            ("CamelRedis.Member", json!("Palermo")),
        ],
    )
    .await;
    assert_eq!(added, json!(1), "adding Palermo must report one new member");

    let added = geo_exchange(
        &h,
        "direct:geo-add",
        &[
            ("CamelRedis.Key", json!("geo:points")),
            ("CamelRedis.Longitude", json!(15.087269)),
            ("CamelRedis.Latitude", json!(37.502669)),
            ("CamelRedis.Member", json!("Catania")),
        ],
    )
    .await;
    assert_eq!(added, json!(1), "adding Catania must report one new member");

    let added = geo_exchange(
        &h,
        "direct:geo-add",
        &[
            ("CamelRedis.Key", json!("geo:points")),
            // Update: a barely moved coordinate keeps the 1e-4 assertions
            // below valid for both the original and the stored position.
            ("CamelRedis.Longitude", json!(13.361390)),
            ("CamelRedis.Latitude", json!(38.115557)),
            ("CamelRedis.Member", json!("Palermo")),
        ],
    )
    .await;
    assert_eq!(
        added,
        json!(0),
        "updating Palermo must report no new member"
    );

    let added = geo_exchange(
        &h,
        "direct:geo-add",
        &[
            ("CamelRedis.Key", json!("geo:points")),
            ("CamelRedis.Longitude", json!(0.0)),
            ("CamelRedis.Latitude", json!(0.0)),
            ("CamelRedis.Member", json!("Farpoint")),
        ],
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
        .lock()
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
    // extract_publish_message) and CamelRedis.Channel selects the channel;
    // CamelRedis.Value is only consulted by key/value commands like SET.
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
        .lock()
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
        .set_header("CamelRedis.Value", Value::String("queue-item".into()))
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
        .lock()
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
        .lock()
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
        .lock()
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
