//! Integration tests for Redis component.
//!
//! Uses testcontainers to spin up Redis instances for testing.
//!
//! **Requires Docker to be running.** Tests will fail if Docker is unavailable.
//!
//! **Requires `integration-tests` feature to compile and run.**

#![cfg(feature = "integration-tests")]

use camel_api::Value;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_mock::MockComponent;
use camel_component_redis::RedisComponent;
use camel_component_timer::TimerComponent;
use camel_core::CamelContext;
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
async fn test_redis_string_commands() {
    let container = setup_redis_container().await;
    let conn_str = get_connection_string(&container).await;

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(RedisComponent::new());
    ctx.register_component(mock.clone());
    ctx.set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("CamelRedis.Key", Value::String("testkey".into()))
        .set_header("CamelRedis.Value", Value::String("testvalue".into()))
        .to(format!("redis://{}?command=SET", conn_str))
        .to("mock:result")
        .route_id("redis-string-test")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    ctx.stop().await.unwrap();

    if let Some(error_ep) = mock.get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint = mock.get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;
}

// ===========================================================================
// List commands tests
// ===========================================================================

#[tokio::test]
async fn test_redis_list_commands() {
    let container = setup_redis_container().await;
    let conn_str = get_connection_string(&container).await;

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(RedisComponent::new());
    ctx.register_component(mock.clone());
    ctx.set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("CamelRedis.Key", Value::String("mylist".into()))
        .set_header("CamelRedis.Value", Value::String("item1".into()))
        .to(format!("redis://{}?command=LPUSH", conn_str))
        .to("mock:result")
        .route_id("redis-list-test")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    ctx.stop().await.unwrap();

    if let Some(error_ep) = mock.get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint = mock.get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;
}

// ===========================================================================
// Hash commands tests
// ===========================================================================

#[tokio::test]
async fn test_redis_hash_commands() {
    let container = setup_redis_container().await;
    let conn_str = get_connection_string(&container).await;

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(RedisComponent::new());
    ctx.register_component(mock.clone());
    ctx.set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("CamelRedis.Key", Value::String("myhash".into()))
        .set_header("CamelRedis.Field", Value::String("field1".into()))
        .set_header("CamelRedis.Value", Value::String("value1".into()))
        .to(format!("redis://{}?command=HSET", conn_str))
        .to("mock:result")
        .route_id("redis-hash-test")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    ctx.stop().await.unwrap();

    if let Some(error_ep) = mock.get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint = mock.get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;
}

// ===========================================================================
// Set commands tests
// ===========================================================================

#[tokio::test]
async fn test_redis_set_commands() {
    let container = setup_redis_container().await;
    let conn_str = get_connection_string(&container).await;

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(RedisComponent::new());
    ctx.register_component(mock.clone());
    ctx.set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("CamelRedis.Key", Value::String("myset".into()))
        .set_header("CamelRedis.Value", Value::String("member1".into()))
        .to(format!("redis://{}?command=SADD", conn_str))
        .to("mock:result")
        .route_id("redis-set-test")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    ctx.stop().await.unwrap();

    if let Some(error_ep) = mock.get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint = mock.get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;
}

// ===========================================================================
// Pub/Sub producer test
// ===========================================================================

#[tokio::test]
async fn test_redis_pubsub_producer() {
    let container = setup_redis_container().await;
    let conn_str = get_connection_string(&container).await;

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(RedisComponent::new());
    ctx.register_component(mock.clone());
    ctx.set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("CamelRedis.Channel", Value::String("mychannel".into()))
        .set_header("CamelRedis.Value", Value::String("hello world".into()))
        .to(format!("redis://{}?command=PUBLISH", conn_str))
        .to("mock:result")
        .route_id("redis-pubsub-producer-test")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    ctx.stop().await.unwrap();

    if let Some(error_ep) = mock.get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint = mock.get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;
}

// ===========================================================================
// Consumer queue mode test
// ===========================================================================

#[tokio::test]
async fn test_redis_consumer_queue_mode() {
    let container = setup_redis_container().await;
    let conn_str = get_connection_string(&container).await;

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(RedisComponent::new());
    ctx.register_component(mock.clone());
    ctx.set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

    let consumer_route = RouteBuilder::from(&format!(
        "redis://{}?command=BRPOP&key=myqueue&timeout=1",
        conn_str
    ))
    .to("mock:consumed")
    .route_id("redis-queue-consumer")
    .build()
    .unwrap();

    ctx.add_route_definition(consumer_route).unwrap();

    let producer_route = RouteBuilder::from("timer:push?period=100&repeatCount=1")
        .set_header("CamelRedis.Key", Value::String("myqueue".into()))
        .set_header("CamelRedis.Value", Value::String("queue-item".into()))
        .to(format!("redis://{}?command=RPUSH", conn_str))
        .route_id("redis-queue-producer")
        .build()
        .unwrap();

    ctx.add_route_definition(producer_route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    ctx.stop().await.unwrap();

    if let Some(error_ep) = mock.get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint = mock.get_endpoint("consumed").unwrap();
    let exchanges = endpoint.get_received_exchanges().await;
    assert!(
        !exchanges.is_empty(),
        "Consumer should have received the item"
    );
}

// ===========================================================================
// Consumer pub/sub mode test
// ===========================================================================

#[tokio::test]
async fn test_redis_consumer_pubsub_mode() {
    let container = setup_redis_container().await;
    let conn_str = get_connection_string(&container).await;

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(RedisComponent::new());
    ctx.register_component(mock.clone());
    ctx.set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

    let consumer_route = RouteBuilder::from(&format!(
        "redis://{}?command=SUBSCRIBE&channels=testchannel",
        conn_str
    ))
    .to("mock:received")
    .route_id("redis-pubsub-consumer")
    .build()
    .unwrap();

    ctx.add_route_definition(consumer_route).unwrap();

    let producer_route = RouteBuilder::from("timer:pub?period=2000&repeatCount=1")
        .set_header("CamelRedis.Channel", Value::String("testchannel".into()))
        .set_header("CamelRedis.Value", Value::String("pubsub-message".into()))
        .to(format!("redis://{}?command=PUBLISH", conn_str))
        .route_id("redis-pubsub-publisher")
        .build()
        .unwrap();

    ctx.add_route_definition(producer_route).unwrap();
    ctx.start().await.unwrap();

    let endpoint = mock.get_endpoint("received").unwrap();
    let mut received = false;
    let timeout = std::time::Duration::from_secs(5);
    let start = std::time::Instant::now();

    while start.elapsed() < timeout {
        let exchanges = endpoint.get_received_exchanges().await;
        if !exchanges.is_empty() {
            received = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    ctx.stop().await.unwrap();

    if let Some(error_ep) = mock.get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    assert!(
        received,
        "Subscriber should have received the message within 5s"
    );
}
