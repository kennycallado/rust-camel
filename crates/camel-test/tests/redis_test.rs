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

use camel_api::Value;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::NetworkRetryPolicy;
use camel_component_redis::{RedisComponent, RedisConfig};
use camel_test::CamelTestContext;
use futures::{FutureExt, StreamExt};
use redis::AsyncCommands;
use std::panic::AssertUnwindSafe;
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
