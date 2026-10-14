//! Integration tests for the Kafka component.
//!
//! Uses testcontainers to spin up an Apache Kafka broker (KRaft mode, no Zookeeper).
//!
//! **Requires Docker to be running.** Tests will fail if Docker is unavailable.
//!
//! **Requires `integration-tests` feature to compile and run.**

#![cfg(feature = "integration-tests")]

mod support;
use support::kafka::shared_kafka;

use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_kafka::KafkaComponent;
use camel_test::CamelTestContext;
use support::wait::wait_until;

// ===========================================================================
// Producer test — timer → kafka, assert no errors
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn kafka_producer_sends_without_error() {
    let (_, brokers) = shared_kafka().await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(KafkaComponent::new())
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(camel_api::Value::String("hello-kafka".to_string()))
        .to(format!("kafka:test-produce?brokers={brokers}&acks=all"))
        .to("mock:result")
        .route_id("kafka-producer-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    wait_until(
        "kafka producer route delivery",
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

    // No errors expected
    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Producer route had errors: {:?}", errors[0].error);
        }
    }

    // Exchange reached mock:result → produce succeeded
    endpoint.assert_exchange_count(1).await;
}

// ===========================================================================
// Consumer test — produce a message, consumer receives it via mock
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn kafka_consumer_receives_message() {
    let (_, brokers) = shared_kafka().await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(KafkaComponent::new())
        .build()
        .await;

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

    h.add_route(consumer_route).await.unwrap();
    h.add_route(producer_route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("consumed").unwrap();
    wait_until(
        "kafka consumer receives message",
        std::time::Duration::from_secs(20),
        std::time::Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;

    let exchanges = endpoint.get_received_exchanges().await;
    assert!(
        !exchanges.is_empty(),
        "Consumer should have received at least one message"
    );
}

// ===========================================================================
// Headers test — received exchange has Kafka metadata headers
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn kafka_consumer_sets_headers() {
    let (_, brokers) = shared_kafka().await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(KafkaComponent::new())
        .build()
        .await;

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

    h.add_route(consumer_route).await.unwrap();
    h.add_route(producer_route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("headers").unwrap();
    wait_until(
        "kafka headers consumer receives message",
        std::time::Duration::from_secs(20),
        std::time::Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;

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

#[tokio::test(flavor = "multi_thread")]
async fn kafka_consumer_cooperative_sticky_strategy() {
    let (_, brokers) = shared_kafka().await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(KafkaComponent::new())
        .build()
        .await;

    let consumer_route = RouteBuilder::from(&format!(
        "kafka:test-cooperative?brokers={brokers}&groupId=coop-group&autoOffsetReset=earliest&partitionAssignmentStrategy=cooperativeSticky"
    ))
    .to("mock:coop-consumed")
    .route_id("kafka-cooperative-consumer")
    .build()
    .unwrap();

    let producer_route =
        RouteBuilder::from("timer:coop-produce?period=3000&delay=5000&repeatCount=1")
            .set_body(camel_api::Value::String("cooperative-payload".to_string()))
            .to(format!("kafka:test-cooperative?brokers={brokers}&acks=all"))
            .route_id("kafka-cooperative-producer")
            .build()
            .unwrap();

    h.add_route(consumer_route).await.unwrap();
    h.add_route(producer_route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("coop-consumed").unwrap();
    wait_until(
        "kafka cooperative consumer receives message",
        std::time::Duration::from_secs(20),
        std::time::Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;

    let exchanges = endpoint.get_received_exchanges().await;
    assert!(
        !exchanges.is_empty(),
        "Consumer with cooperativeSticky strategy should have received at least one message"
    );
}

// ===========================================================================
// master: over a real Explicit delegate (rc-w02ns)
// ===========================================================================

/// `master:` wrapping the Kafka consumer (Explicit startup: `start()` returns
/// after spawning the poll task). The delegate must be started once, deliver,
/// and never be stopped until shutdown. Before the fix master stopped it right
/// after `start()`, churning leadership.
#[tokio::test(flavor = "multi_thread")]
async fn master_keeps_kafka_delegate_running() {
    use camel_component_api::ComponentBundle;
    use camel_component_mock::MockComponent;
    use camel_component_timer::TimerComponent;
    use camel_core::CamelContext;
    use camel_master::MasterBundle;
    use support::master_probe;

    let (_, brokers) = shared_kafka().await;

    let mut ctx = CamelContext::builder().build().await.unwrap();
    MasterBundle::from_toml(toml::Value::Table(toml::map::Map::new()))
        .unwrap()
        .register_all(&mut ctx);
    ctx.register_component(KafkaComponent::new());
    ctx.register_component(TimerComponent::new());
    let mock = MockComponent::new();
    ctx.register_component(mock.clone());
    let (mut ctx, probe) = master_probe::attach(ctx);

    let consumer = RouteBuilder::from(&format!(
        "master:kafka-explicit:kafka:test-master-explicit?brokers={brokers}\
         &groupId=master-explicit-group&autoOffsetReset=earliest"
    ))
    .to("mock:consumed")
    .route_id("master-kafka-consumer")
    .build()
    .unwrap();
    // 5 messages over ~5s keep the observation window open: churn restarts
    // the delegate within hundreds of ms, so `started` would exceed 1.
    let producer = RouteBuilder::from("timer:master-produce?period=1000&delay=3000&repeatCount=5")
        .set_body(camel_api::Value::String("master-kafka".to_string()))
        .to(format!(
            "kafka:test-master-explicit?brokers={brokers}&acks=all"
        ))
        .route_id("master-kafka-producer")
        .build()
        .unwrap();
    ctx.add_route_definition(consumer).await.unwrap();
    ctx.add_route_definition(producer).await.unwrap();
    ctx.start().await.unwrap();

    let endpoint = mock.get_endpoint("consumed").unwrap();
    wait_until(
        "master-wrapped kafka consumer receives 5 messages",
        std::time::Duration::from_secs(60),
        std::time::Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(endpoint.get_received_exchanges().await.len() >= 5) }
        },
    )
    .await
    .unwrap();

    assert_eq!(probe.count("started"), 1, "delegate must not churn");
    assert_eq!(probe.count("stopped"), 0, "no stop before shutdown");

    ctx.stop().await.unwrap();
}
