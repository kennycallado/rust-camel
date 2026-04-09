//! Kafka example for rust-camel.
//!
//! Demonstrates:
//!   - Route 1 (Producer): timer → set_body → to(kafka:TOPIC)
//!   - Route 2 (Consumer): from(kafka:TOPIC) → log
//!
//! A Kafka broker is started automatically via testcontainers (requires Docker).
//! No external infrastructure needed — just run:
//!
//!   cargo run -p kafka-example
//!
//! Press Ctrl+C to stop.

use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_kafka::KafkaComponent;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::kafka::apache;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    const TOPIC: &str = "orders";

    // Start a Kafka broker in Docker (KRaft mode, no Zookeeper).
    // The container is kept alive for the duration of main() via _container.
    println!("Starting Kafka broker via testcontainers (requires Docker)...");
    let _container = apache::Kafka::default()
        .start()
        .await
        .expect("Failed to start Kafka container — is Docker running?");
    let port = _container
        .get_host_port_ipv4(apache::KAFKA_PORT)
        .await
        .expect("Failed to get Kafka port");
    let brokers = format!("127.0.0.1:{port}");
    println!("Kafka broker available at {brokers}");

    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(KafkaComponent::new());

    // Route 1: Timer → Kafka producer (every 3 seconds)
    let producer_route = RouteBuilder::from("timer:tick?period=3000")
        .route_id("kafka-producer")
        .set_body(Value::String(
            r#"{"event":"heartbeat","source":"rust-camel"}"#.to_string(),
        ))
        .to(format!(
            "kafka:{topic}?brokers={brokers}&acks=all",
            topic = TOPIC,
            brokers = brokers
        ))
        .build()?;

    // Route 2: Kafka consumer → Log
    let consumer_route = RouteBuilder::from(&format!(
        "kafka:{topic}?brokers={brokers}&groupId=example-group&autoOffsetReset=earliest",
        topic = TOPIC,
        brokers = brokers
    ))
    .route_id("kafka-consumer")
    .to("log:info?showHeaders=true")
    .build()?;

    ctx.add_route_definition(producer_route).await?;
    ctx.add_route_definition(consumer_route).await?;

    println!("Starting Kafka example... Press Ctrl+C to stop.");

    ctx.start().await?;

    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;
    Ok(())
}
