//! Integration tests for the Kafka component.
//!
//! Uses testcontainers to spin up an Apache Kafka broker (KRaft mode, no Zookeeper).
//!
//! **Requires Docker to be running.** Tests will fail if Docker is unavailable.

use camel_api::{Body, Exchange, Message};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_kafka::{KafkaComponent, KafkaEndpointConfig, KafkaProducer};
use camel_component_mock::MockComponent;
use camel_component_timer::TimerComponent;
use camel_core::CamelContext;
use testcontainers::runners::AsyncRunner;
use testcontainers::ContainerAsync;
use testcontainers_modules::kafka::apache;
use std::sync::OnceLock;
use tower::Service as _;

/// Serializes tests in this file so we don't start multiple Kafka brokers in parallel.
static TEST_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

fn test_lock() -> &'static tokio::sync::Mutex<()> {
    TEST_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Initialise tracing once per process (ignores the error if already set).
fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let _ = fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("rdkafka=debug,camel=debug,warn")),
        )
        .with_test_writer()
        .try_init();
}

/// Starts an Apache Kafka broker and returns `(container, brokers)`.
///
/// `container.start().await` waits for "Kafka Server started", but we still do
/// a protocol-level warm-up (real produce) before considering the broker ready.
async fn start_kafka() -> (ContainerAsync<apache::Kafka>, String) {
    init_tracing();

    let container: ContainerAsync<apache::Kafka> = apache::Kafka::default().start().await.unwrap();
    let port = container
        .get_host_port_ipv4(apache::KAFKA_PORT)
        .await
        .unwrap();
    let brokers = format!("127.0.0.1:{port}");
    eprintln!("Kafka bootstrap: {brokers}");

    // Protocol-level warm-up with a slightly larger per-attempt timeout.
    // This is test-only and reduces flakiness under CI/host load.
    let warmup_deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    loop {
        let mut cfg = KafkaEndpointConfig::from_uri(&format!(
            "kafka:__warmup__?brokers={}&requestTimeoutMs=10000",
            brokers
        ))
        .unwrap();
        cfg.resolve_defaults();
        let mut producer = KafkaProducer::new(cfg).unwrap();
        let msg = Message::new(Body::Text("warmup".to_string()));
        let result = producer.call(Exchange::new(msg)).await;
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

// ===========================================================================
// Producer test — timer → kafka, assert no errors
// ===========================================================================

#[tokio::test]
async fn test_kafka_producer_sends_without_error() {
    let _guard = test_lock().lock().await;
    let (_container, brokers) = start_kafka().await;

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(KafkaComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(camel_api::Value::String("hello-kafka".to_string()))
        .to(format!("kafka:test-produce?brokers={brokers}&acks=all"))
        .to("mock:result")
        .route_id("kafka-producer-test")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    ctx.stop().await.unwrap();

    // No errors expected
    if let Some(error_ep) = mock.get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Producer route had errors: {:?}", errors[0].error);
        }
    }

    // Exchange reached mock:result → produce succeeded
    let endpoint = mock.get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;
}

// ===========================================================================
// Consumer test — produce a message, consumer receives it via mock
// ===========================================================================

#[tokio::test]
async fn test_kafka_consumer_receives_message() {
    let _guard = test_lock().lock().await;
    let (_container, brokers) = start_kafka().await;

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(KafkaComponent::new());
    ctx.register_component(mock.clone());

    // Consumer route: kafka:test-consume → mock:consumed
    let consumer_route = RouteBuilder::from(&format!(
        "kafka:test-consume?brokers={brokers}&groupId=test-group&autoOffsetReset=earliest"
    ))
    .to("mock:consumed")
    .route_id("kafka-consumer-test")
    .build()
    .unwrap();

    // Producer route: timer (once) → set_body → kafka:test-consume
    // delay=5000 gives the consumer time to join the group and get partition assignment
    // before the first message is sent (the timer's 'delay' param controls the initial wait).
    let producer_route = RouteBuilder::from("timer:produce?period=3000&delay=5000&repeatCount=1")
        .set_body(camel_api::Value::String("round-trip-payload".to_string()))
        .to(format!("kafka:test-consume?brokers={brokers}&acks=all"))
        .route_id("kafka-producer-for-consumer-test")
        .build()
        .unwrap();

    ctx.add_route_definition(consumer_route).unwrap();
    ctx.add_route_definition(producer_route).unwrap();
    ctx.start().await.unwrap();

    // Give consumer time to assign partitions (5s delay before producer fires),
    // then consumer receives. 15s total budget is comfortable.
    tokio::time::sleep(std::time::Duration::from_secs(15)).await;
    ctx.stop().await.unwrap();

    let endpoint = mock.get_endpoint("consumed").unwrap();
    let exchanges = endpoint.get_received_exchanges().await;
    assert!(
        !exchanges.is_empty(),
        "Consumer should have received at least one message"
    );
}

// ===========================================================================
// Headers test — received exchange has Kafka metadata headers
// ===========================================================================

#[tokio::test]
async fn test_kafka_consumer_sets_headers() {
    let _guard = test_lock().lock().await;
    let (_container, brokers) = start_kafka().await;

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(KafkaComponent::new());
    ctx.register_component(mock.clone());

    let consumer_route = RouteBuilder::from(&format!(
        "kafka:test-headers?brokers={brokers}&groupId=hdr-group&autoOffsetReset=earliest"
    ))
    .to("mock:headers")
    .route_id("kafka-headers-consumer")
    .build()
    .unwrap();

    let producer_route =
        RouteBuilder::from("timer:hdr-produce?period=3000&delay=5000&repeatCount=1")
            .set_body(camel_api::Value::String("header-check".to_string()))
            .to(format!("kafka:test-headers?brokers={brokers}&acks=all"))
            .route_id("kafka-headers-producer")
            .build()
            .unwrap();

    ctx.add_route_definition(consumer_route).unwrap();
    ctx.add_route_definition(producer_route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_secs(15)).await;
    ctx.stop().await.unwrap();

    let endpoint = mock.get_endpoint("headers").unwrap();
    let exchanges = endpoint.get_received_exchanges().await;
    assert!(
        !exchanges.is_empty(),
        "Consumer should have received at least one message"
    );

    let ex = &exchanges[0];
    assert!(
        ex.input.header("CamelKafkaTopic").is_some(),
        "CamelKafkaTopic header must be present"
    );
    assert!(
        ex.input.header("CamelKafkaPartition").is_some(),
        "CamelKafkaPartition header must be present"
    );
    assert!(
        ex.input.header("CamelKafkaOffset").is_some(),
        "CamelKafkaOffset header must be present"
    );
    assert!(
        ex.input.header("CamelKafkaGroupId").is_some(),
        "CamelKafkaGroupId header must be present"
    );
    // Note: kafka.manual.commit JSON property removed; use exchange.get_extension::<KafkaManualCommit>("kafka.manual_commit")
    // when allowManualCommit=true is configured.
}
