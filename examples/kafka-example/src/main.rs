//! Kafka example for rust-camel.
//!
//! Demonstrates:
//!   - Route 1 (Producer): timer → set_body → to(kafka:TOPIC)
//!   - Route 2 (Consumer): from(kafka:TOPIC) → log
//!
//! Prerequisites:
//!   docker run -p 9092:9092 apache/kafka:latest
//!
//! Run:
//!   KAFKA_BROKERS=localhost:9092 cargo run -p kafka-example

use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_kafka::KafkaComponent;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    const TOPIC: &str = "orders";

    let brokers = std::env::var("KAFKA_BROKERS").unwrap_or_else(|_| "localhost:9092".to_string());

    let mut ctx = CamelContext::new();
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

    ctx.add_route_definition(producer_route)?;
    ctx.add_route_definition(consumer_route)?;

    println!("Starting Kafka example...");
    println!("Prerequisites: docker run -p 9092:9092 apache/kafka:latest");
    println!("Press Ctrl+C to stop.");

    ctx.start().await?;

    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;
    Ok(())
}
