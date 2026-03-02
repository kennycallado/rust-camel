//! Integration tests for Redis component.
//!
//! Uses testcontainers to spin up Redis instances for testing.
//!
//! **Requires Docker to be running.** Tests will fail if Docker is unavailable.

use camel_api::Value;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_core::CamelContext;
use camel_mock::MockComponent;
use camel_redis::RedisComponent;
use camel_timer::TimerComponent;
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::redis::Redis;

async fn setup_redis_container() -> ContainerAsync<Redis> {
    Redis::default().start().await.unwrap()
}

async fn get_connection_string(container: &ContainerAsync<Redis>) -> String {
    let port = container.get_host_port_ipv4(6379).await.unwrap();
    format!("redis://127.0.0.1:{}", port)
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
    
    // Test SET command: timer → Redis SET → mock
    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("CamelRedis.Key", Value::String("testkey".into()))
        .map_body(|_| camel_api::body::Body::Text("testvalue".into()))
        .to(&format!("redis://{}?command=SET", conn_str))
        .to("mock:result")
        .build()
        .unwrap();
    
    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();
    
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    ctx.stop().await.unwrap();
    
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
    
    // Test LPUSH command: timer → Redis LPUSH → mock
    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("CamelRedis.Key", Value::String("mylist".into()))
        .map_body(|_| camel_api::body::Body::Text("item1".into()))
        .to(&format!("redis://{}?command=LPUSH", conn_str))
        .to("mock:result")
        .build()
        .unwrap();
    
    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();
    
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    ctx.stop().await.unwrap();
    
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
    
    // Test HSET command: timer → Redis HSET → mock
    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("CamelRedis.Key", Value::String("myhash".into()))
        .set_header("CamelRedis.Field", Value::String("field1".into()))
        .map_body(|_| camel_api::body::Body::Text("value1".into()))
        .to(&format!("redis://{}?command=HSET", conn_str))
        .to("mock:result")
        .build()
        .unwrap();
    
    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();
    
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    ctx.stop().await.unwrap();
    
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
    
    // Test SADD command: timer → Redis SADD → mock
    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("CamelRedis.Key", Value::String("myset".into()))
        .map_body(|_| camel_api::body::Body::Text("member1".into()))
        .to(&format!("redis://{}?command=SADD", conn_str))
        .to("mock:result")
        .build()
        .unwrap();
    
    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();
    
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    ctx.stop().await.unwrap();
    
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
    
    // Test PUBLISH command: timer → Redis PUBLISH → mock
    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("CamelRedis.Channel", Value::String("mychannel".into()))
        .map_body(|_| camel_api::body::Body::Text("hello world".into()))
        .to(&format!("redis://{}?command=PUBLISH", conn_str))
        .to("mock:result")
        .build()
        .unwrap();
    
    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();
    
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    ctx.stop().await.unwrap();
    
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
    
    // Consumer route: BRPOP from myqueue (timeout=1s to avoid blocking forever)
    let consumer_route = RouteBuilder::from(&format!(
        "redis://{}?command=BRPOP&key=myqueue&timeout=1",
        conn_str
    ))
    .to("mock:consumed")
    .build()
    .unwrap();
    
    ctx.add_route_definition(consumer_route).unwrap();
    
    // Producer route: push item to queue after delay
    let producer_route = RouteBuilder::from("timer:push?period=100&repeatCount=1")
        .set_header("CamelRedis.Key", Value::String("myqueue".into()))
        .map_body(|_| camel_api::body::Body::Text("queue-item".into()))
        .to(&format!("redis://{}?command=RPUSH", conn_str))
        .build()
        .unwrap();
    
    ctx.add_route_definition(producer_route).unwrap();
    ctx.start().await.unwrap();
    
    // Wait for timer to fire and consumer to process
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    ctx.stop().await.unwrap();
    
    let endpoint = mock.get_endpoint("consumed").unwrap();
    let exchanges = endpoint.get_received_exchanges().await;
    assert!(!exchanges.is_empty(), "Consumer should have received the item");
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
    
    // Consumer route: SUBSCRIBE to testchannel
    let consumer_route = RouteBuilder::from(&format!(
        "redis://{}?command=SUBSCRIBE&channels=testchannel",
        conn_str
    ))
    .to("mock:received")
    .build()
    .unwrap();
    
    ctx.add_route_definition(consumer_route).unwrap();
    
    // Producer route: publish message after delay
    let producer_route = RouteBuilder::from("timer:pub?period=200&repeatCount=1")
        .set_header("CamelRedis.Channel", Value::String("testchannel".into()))
        .map_body(|_| camel_api::body::Body::Text("pubsub-message".into()))
        .to(&format!("redis://{}?command=PUBLISH", conn_str))
        .build()
        .unwrap();
    
    ctx.add_route_definition(producer_route).unwrap();
    ctx.start().await.unwrap();
    
    // Wait for subscriber to connect and message to be published
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    ctx.stop().await.unwrap();
    
    let endpoint = mock.get_endpoint("received").unwrap();
    let exchanges = endpoint.get_received_exchanges().await;
    assert!(!exchanges.is_empty(), "Subscriber should have received the message");
}
