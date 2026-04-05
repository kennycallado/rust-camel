//! Integration tests for Redis component.
//!
//! Uses testcontainers to spin up Redis instances for testing.
//!
//! **Requires Docker to be running.** Tests will fail if Docker is unavailable.
//!
//! **Requires `integration-tests` feature to compile and run.**

#![cfg(feature = "integration-tests")]

mod support;

use camel_api::Value;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_redis::RedisComponent;
use camel_test::CamelTestContext;
use redis::AsyncCommands;
use support::wait::wait_until;
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::redis::Redis;

async fn setup_redis_container() -> ContainerAsync<Redis> {
    Redis::default().start().await.unwrap()
}

async fn get_connection_string(container: &ContainerAsync<Redis>) -> String {
    let port = container.get_host_port_ipv4(6379).await.unwrap();
    let conn_str = format!("127.0.0.1:{}", port);
    eprintln!("Redis connection: {}", conn_str);
    conn_str
}

// ===========================================================================
// String commands tests
// ===========================================================================

#[tokio::test]
async fn redis_string_commands() {
    let container = setup_redis_container().await;
    let conn_str = get_connection_string(&container).await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(RedisComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

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

#[tokio::test]
async fn redis_list_commands() {
    let container = setup_redis_container().await;
    let conn_str = get_connection_string(&container).await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(RedisComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

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

#[tokio::test]
async fn redis_hash_commands() {
    let container = setup_redis_container().await;
    let conn_str = get_connection_string(&container).await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(RedisComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

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

#[tokio::test]
async fn redis_set_commands() {
    let container = setup_redis_container().await;
    let conn_str = get_connection_string(&container).await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(RedisComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

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

#[tokio::test]
async fn redis_pubsub_producer() {
    let container = setup_redis_container().await;
    let conn_str = get_connection_string(&container).await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(RedisComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("CamelRedis.Channel", Value::String("mychannel".into()))
        .set_header("CamelRedis.Value", Value::String("hello world".into()))
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

#[tokio::test]
async fn redis_consumer_queue_mode() {
    let container = setup_redis_container().await;
    let conn_str = get_connection_string(&container).await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(RedisComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

    let consumer_route = RouteBuilder::from(&format!(
        "redis://{}?command=BRPOP&key=myqueue&timeout=1",
        conn_str
    ))
    .to("mock:consumed")
    .route_id("redis-queue-consumer")
    .build()
    .unwrap();

    h.add_route(consumer_route).await.unwrap();

    let producer_route = RouteBuilder::from("timer:push?period=100&repeatCount=1")
        .set_header("CamelRedis.Key", Value::String("myqueue".into()))
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

#[tokio::test]
async fn redis_consumer_blpop_reads_left_side_first() {
    let container = setup_redis_container().await;
    let conn_str = get_connection_string(&container).await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(RedisComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

    let consumer_route = RouteBuilder::from(&format!(
        "redis://{}?command=BLPOP&key=myqueue&timeout=1",
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
        .rpush("myqueue", vec!["item-a", "item-b"])
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

#[tokio::test]
async fn redis_consumer_brpop_reads_right_side_first() {
    let container = setup_redis_container().await;
    let conn_str = get_connection_string(&container).await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(RedisComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

    let consumer_route = RouteBuilder::from(&format!(
        "redis://{}?command=BRPOP&key=myqueue&timeout=1",
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
        .rpush("myqueue", vec!["item-a", "item-b"])
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

#[tokio::test]
async fn redis_consumer_pubsub_mode() {
    let container = setup_redis_container().await;
    let conn_str = get_connection_string(&container).await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(RedisComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

    let consumer_route = RouteBuilder::from(&format!(
        "redis://{}?command=SUBSCRIBE&channels=testchannel",
        conn_str
    ))
    .to("mock:received")
    .route_id("redis-pubsub-consumer")
    .build()
    .unwrap();

    h.add_route(consumer_route).await.unwrap();

    let producer_route = RouteBuilder::from("timer:pub?period=2000&repeatCount=1")
        .set_header("CamelRedis.Channel", Value::String("testchannel".into()))
        .set_header("CamelRedis.Value", Value::String("pubsub-message".into()))
        .to(format!("redis://{}?command=PUBLISH", conn_str))
        .route_id("redis-pubsub-publisher")
        .build()
        .unwrap();

    h.add_route(producer_route).await.unwrap();
    h.start().await;

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
