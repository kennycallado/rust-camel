//! Low-level component tests for the Redis consumer.
//!
//! These tests exercise `RedisConsumer` directly (not via the Camel routing
//! engine) using testcontainers to spin up a real Redis instance.
//!
//! **Requires Docker to be running.** Tests will fail if Docker is unavailable.

use camel_component::{Consumer, ConsumerContext};
use camel_component_redis::{RedisConsumer, RedisEndpointConfig};
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::redis::Redis;
use tokio_util::sync::CancellationToken;

async fn setup_redis_container() -> ContainerAsync<Redis> {
    Redis::default().start().await.unwrap()
}

async fn get_redis_addr(container: &ContainerAsync<Redis>) -> (String, u16) {
    let port = container.get_host_port_ipv4(6379).await.unwrap();
    eprintln!("Redis connection: 127.0.0.1:{}", port);
    ("127.0.0.1".to_string(), port)
}

// ===========================================================================
// Pub/Sub consumer test
// ===========================================================================

/// Test: RedisConsumer in SUBSCRIBE mode receives a published message.
#[tokio::test]
async fn test_pubsub_consumer_receives_messages() {
    use camel_component::ExchangeEnvelope;
    use std::time::Duration;
    use tokio::sync::mpsc;

    let container = setup_redis_container().await;
    let (host, port) = get_redis_addr(&container).await;

    // Create consumer config for SUBSCRIBE on "test-channel"
    let uri = format!("redis://{}:{}?command=SUBSCRIBE&channels=test-channel", host, port);
    let config = RedisEndpointConfig::from_uri(&uri).unwrap();
    let mut consumer = RedisConsumer::new(config);

    let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let cancel_token = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel_token.clone());

    consumer.start(ctx).await.unwrap();

    // Give consumer time to subscribe before publishing
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Publish a message using a separate Redis connection
    let client = redis::Client::open(format!("redis://{}:{}", host, port)).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    redis::cmd("PUBLISH")
        .arg("test-channel")
        .arg("hello-pubsub")
        .query_async::<()>(&mut conn)
        .await
        .unwrap();

    // Wait for the consumer to deliver the exchange
    let envelope = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("Should receive message within 5s")
        .expect("Channel should not be closed");

    assert_eq!(
        envelope.exchange.input.body.as_text(),
        Some("hello-pubsub"),
        "Body should contain the published message"
    );

    cancel_token.cancel();
    consumer.stop().await.unwrap();
}

// ===========================================================================
// Queue consumer test
// ===========================================================================

/// Test: RedisConsumer in BLPOP mode receives an item pushed to the list.
#[tokio::test]
async fn test_queue_consumer_processes_items() {
    use camel_component::ExchangeEnvelope;
    use std::time::Duration;
    use tokio::sync::mpsc;

    let container = setup_redis_container().await;
    let (host, port) = get_redis_addr(&container).await;

    // Create consumer config for BLPOP on "test-queue"
    let uri = format!("redis://{}:{}?command=BLPOP&key=test-queue&timeout=2", host, port);
    let config = RedisEndpointConfig::from_uri(&uri).unwrap();
    let mut consumer = RedisConsumer::new(config);

    let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let cancel_token = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel_token.clone());

    consumer.start(ctx).await.unwrap();

    // Give consumer time to start before pushing
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Push an item to the queue using a separate Redis connection
    let client = redis::Client::open(format!("redis://{}:{}", host, port)).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    redis::cmd("LPUSH")
        .arg("test-queue")
        .arg("item1")
        .query_async::<()>(&mut conn)
        .await
        .unwrap();

    // Wait for the consumer to deliver the exchange
    let envelope = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("Should receive item within 5s")
        .expect("Channel should not be closed");

    assert_eq!(
        envelope.exchange.input.body.as_text(),
        Some("item1"),
        "Body should contain the pushed item"
    );

    cancel_token.cancel();
    consumer.stop().await.unwrap();
}
