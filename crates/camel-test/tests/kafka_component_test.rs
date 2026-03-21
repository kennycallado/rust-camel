//! Low-level component tests for Kafka producer and consumer.
//!
//! These tests exercise `KafkaProducer` and `KafkaConsumer` directly (not via
//! the Camel routing engine) using testcontainers to spin up a real broker.
//!
//! Tests are serialized with a lock so only one Kafka container runs at a time.
//!
//! **Requires Docker to be running.** Tests will fail if Docker is unavailable.

use camel_api::{Body, Exchange, Message};
use camel_component::{Consumer, ConsumerContext};
use camel_component_kafka::{KafkaConsumer, KafkaEndpointConfig, KafkaProducer};
use std::sync::OnceLock;
use testcontainers::runners::AsyncRunner;
use testcontainers::ContainerAsync;
use testcontainers_modules::kafka::apache;
use tower::Service as _;

// ---------------------------------------------------------------------------
// Kafka container lifecycle helpers
// ---------------------------------------------------------------------------

/// Serializes tests in this file so we don't start multiple Kafka brokers in parallel.
static TEST_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

fn test_lock() -> &'static tokio::sync::Mutex<()> {
    TEST_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Starts an Apache Kafka broker and returns `(container, brokers)`.
///
/// After `container.start().await` returns, testcontainers has already
/// waited for "Kafka Server started" in stdout via `exec_after_start`.
/// We then do an active Kafka-protocol warm-up: send a test message with
/// a short `message.timeout.ms` and retry until delivery succeeds (or
/// the deadline is reached). This ensures the broker is fully ready to
/// handle Kafka protocol requests before any test runs.
async fn start_kafka() -> (ContainerAsync<apache::Kafka>, String) {
    let container: ContainerAsync<apache::Kafka> = apache::Kafka::default().start().await.unwrap();
    let port = container
        .get_host_port_ipv4(apache::KAFKA_PORT)
        .await
        .unwrap();
    let brokers = format!("127.0.0.1:{port}");
    eprintln!("Kafka bootstrap: {brokers}");

    // Active Kafka-protocol warm-up: retry produce until the broker
    // actually accepts Kafka protocol requests (not just TCP).
    // Uses a test-only timeout that is less sensitive to host/CI load.
    let warmup_deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    loop {
        let mut cfg = KafkaEndpointConfig::from_uri(&format!(
            "kafka:__warmup__?brokers={}&requestTimeoutMs=10000",
            brokers
        ))
        .unwrap();
        cfg.resolve_defaults();
        let mut producer = KafkaProducer::new(cfg).unwrap();
        let msg = camel_api::Message::new(camel_api::Body::Text("warmup".to_string()));
        let result = producer.call(camel_api::Exchange::new(msg)).await;
        if result.is_ok() {
            eprintln!("Kafka warm-up: broker ready at {brokers}");
            break;
        }
        if std::time::Instant::now() >= warmup_deadline {
            panic!("Kafka broker at {brokers} did not become ready within 90s");
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    (container, brokers)
}

/// Ensure a topic exists before starting a consumer that waits for assignment.
///
/// Some Kafka environments do not assign partitions for a subscribed consumer
/// until the topic exists. Creating the topic with a one-off produce keeps the
/// tests deterministic.
async fn ensure_topic_exists(brokers: &str, topic: &str) {
    let mut cfg = KafkaEndpointConfig::from_uri(&format!(
        "kafka:{topic}?brokers={brokers}&requestTimeoutMs=10000"
    ))
    .unwrap();
    cfg.resolve_defaults();

    let mut producer = KafkaProducer::new(cfg).unwrap();
    let msg = Message::new(Body::Text("topic-init".to_string()));
    producer.call(Exchange::new(msg)).await.unwrap();
}

// ===========================================================================
// Producer tests
// ===========================================================================

/// Test: KafkaProducer sends a message and receives CamelKafkaRecordMetadata header back.
#[tokio::test(flavor = "multi_thread")]
async fn test_producer_sends_and_acks() {
    let _guard = test_lock().lock().await;
    let (_container, brokers) = start_kafka().await;
    let mut config =
        KafkaEndpointConfig::from_uri(&format!("kafka:test-integration?brokers={}", brokers))
            .unwrap();
    config.resolve_defaults();

    let mut producer = KafkaProducer::new(config).unwrap();
    let msg = Message::new(Body::Text("integration test".to_string()));
    let exchange = Exchange::new(msg);

    let result = producer.call(exchange).await;
    assert!(result.is_ok(), "Producer should succeed: {:?}", result.err());

    let out = result.unwrap();
    let metadata = out.input.header("CamelKafkaRecordMetadata");
    assert!(
        metadata.is_some(),
        "CamelKafkaRecordMetadata header should be present"
    );
}

/// Test: KafkaProducer honours topic override from CamelKafkaTopic header.
#[tokio::test(flavor = "multi_thread")]
async fn test_producer_resolves_topic_from_header() {
    let _guard = test_lock().lock().await;
    let (_container, brokers) = start_kafka().await;
    let mut config =
        KafkaEndpointConfig::from_uri(&format!("kafka:default-topic?brokers={}", brokers))
            .unwrap();
    config.resolve_defaults();

    let mut producer = KafkaProducer::new(config).unwrap();
    let mut msg = Message::new(Body::Text("test".to_string()));
    msg.set_header(
        "CamelKafkaTopic",
        serde_json::Value::String("override-topic".to_string()),
    );
    let exchange = Exchange::new(msg);

    let result = producer.call(exchange).await;
    assert!(result.is_ok(), "Producer should succeed: {:?}", result.err());

    let out = result.unwrap();
    let meta = out.input.header("CamelKafkaRecordMetadata").unwrap();
    assert_eq!(
        meta["topic"].as_str().unwrap(),
        "override-topic",
        "Topic should be the header-overridden value"
    );
}

/// Test: KafkaProducer returns an error when the broker is unreachable.
#[tokio::test(flavor = "multi_thread")]
async fn test_producer_delivery_error_on_unavailable_broker() {
    let mut config = KafkaEndpointConfig::from_uri(
        "kafka:test?brokers=192.0.2.1:9092&requestTimeoutMs=3000",
    )
    .unwrap();
    config.resolve_defaults();
    let mut producer = KafkaProducer::new(config).unwrap();
    let msg = Message::new(Body::Text("test".to_string()));
    let exchange = Exchange::new(msg);

    let result = producer.call(exchange).await;
    assert!(result.is_err(), "Should fail for unreachable broker");
}

// ===========================================================================
// Consumer tests
// ===========================================================================

/// Test: KafkaConsumer receives a message produced after partition assignment.
#[tokio::test(flavor = "multi_thread")]
async fn test_consumer_receives_messages() {
    use camel_component::ExchangeEnvelope;
    use std::time::Duration;
    use tokio::sync::mpsc;

    let _guard = test_lock().lock().await;
    let (_container, brokers) = start_kafka().await;
    ensure_topic_exists(&brokers, "test-consumer-recv").await;

    let mut config = KafkaEndpointConfig::from_uri(&format!(
        "kafka:test-consumer-recv?brokers={}&groupId=test-recv&autoOffsetReset=earliest",
        brokers
    ))
    .unwrap();
    config.resolve_defaults();

    let mut consumer = KafkaConsumer::new(config.clone());
    let ready = consumer.ready_signal();
    let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let ctx = ConsumerContext::new(tx, Default::default());

    // Start consumer first, then wait for partition assignment before producing
    consumer.start(ctx).await.unwrap();

    // Wait until partitions are assigned (condition-based, no arbitrary sleep)
    tokio::time::timeout(Duration::from_secs(30), ready.notified())
        .await
        .expect("Consumer should receive partition assignment within 30s");

    let mut prod_config =
        KafkaEndpointConfig::from_uri(&format!("kafka:test-consumer-recv?brokers={}", brokers))
            .unwrap();
    prod_config.resolve_defaults();
    let mut producer = KafkaProducer::new(prod_config).unwrap();
    let msg = camel_api::Message::new(camel_api::Body::Text("hello-kafka".to_string()));
    producer.call(camel_api::Exchange::new(msg)).await.unwrap();

    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let envelope = rx.recv().await.expect("Channel not closed");
            if envelope.exchange.input.body.as_text() == Some("hello-kafka") {
                break;
            }
        }
    })
    .await
    .expect("Should receive hello-kafka within 15s");

    consumer.stop().await.unwrap();
}

/// Test: Received exchange has Kafka metadata headers populated.
#[tokio::test(flavor = "multi_thread")]
async fn test_consumer_headers_populated() {
    use camel_component::ExchangeEnvelope;
    use std::time::Duration;
    use tokio::sync::mpsc;

    let _guard = test_lock().lock().await;
    let (_container, brokers) = start_kafka().await;
    ensure_topic_exists(&brokers, "test-headers-low").await;

    let mut config = KafkaEndpointConfig::from_uri(&format!(
        "kafka:test-headers-low?brokers={}&groupId=test-headers-low&autoOffsetReset=earliest",
        brokers
    ))
    .unwrap();
    config.resolve_defaults();

    let mut consumer = KafkaConsumer::new(config.clone());
    let ready = consumer.ready_signal();
    let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let ctx = ConsumerContext::new(tx, Default::default());
    consumer.start(ctx).await.unwrap();

    // Wait until partitions are assigned (condition-based, no arbitrary sleep)
    tokio::time::timeout(Duration::from_secs(30), ready.notified())
        .await
        .expect("Consumer should receive partition assignment within 30s");

    let mut prod_config =
        KafkaEndpointConfig::from_uri(&format!("kafka:test-headers-low?brokers={}", brokers))
            .unwrap();
    prod_config.resolve_defaults();
    let mut producer = KafkaProducer::new(prod_config).unwrap();
    let msg = camel_api::Message::new(camel_api::Body::Text("headers-test".to_string()));
    producer.call(camel_api::Exchange::new(msg)).await.unwrap();

    let envelope = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .unwrap()
        .unwrap();
    let ex = &envelope.exchange;

    assert!(
        ex.input.header("CamelKafkaTopic").is_some(),
        "CamelKafkaTopic should be present"
    );
    assert!(
        ex.input.header("CamelKafkaPartition").is_some(),
        "CamelKafkaPartition should be present"
    );
    assert!(
        ex.input.header("CamelKafkaOffset").is_some(),
        "CamelKafkaOffset should be present"
    );
    assert!(
        ex.input.header("CamelKafkaGroupId").is_some(),
        "CamelKafkaGroupId should be present"
    );

    consumer.stop().await.unwrap();
}

/// Test: KafkaConsumer.stop() returns Ok after a graceful shutdown.
#[tokio::test(flavor = "multi_thread")]
async fn test_consumer_graceful_stop() {
    use camel_component::ExchangeEnvelope;
    use std::time::Duration;
    use tokio::sync::mpsc;

    let _guard = test_lock().lock().await;
    let (_container, brokers) = start_kafka().await;
    let mut config = KafkaEndpointConfig::from_uri(&format!(
        "kafka:test-graceful-stop?brokers={}&groupId=test-stop",
        brokers
    ))
    .unwrap();
    config.resolve_defaults();

    let mut consumer = KafkaConsumer::new(config);
    let (tx, _rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let ctx = ConsumerContext::new(tx, Default::default());

    consumer.start(ctx).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = consumer.stop().await;
    assert!(result.is_ok(), "stop() should return Ok");
}
