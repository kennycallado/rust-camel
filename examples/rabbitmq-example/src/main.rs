//! RabbitMQ example for rust-camel.
//!
//! Demonstrates:
//!   - `rabbitmq:` scheme against the default exchange.
//!   - Route 1 (Consumer): from(rabbitmq:default?queue=demo&autoDeclare=true) → log.
//!   - Route 2 (Producer): timer → set_body(json) → to(rabbitmq:default?queue=demo).
//!
//! The broker URL comes from `RABBITMQ_URL`, defaulting to the local fixture
//! `amqp://rmq:rmq@127.0.0.1:5672/%2f`. Start that fixture with Docker:
//!
//!   docker run -d --rm --name rmq-example \
//!     -p 127.0.0.1:5672:5672 \
//!     -e RABBITMQ_DEFAULT_USER=rmq \
//!     -e RABBITMQ_DEFAULT_PASS=rmq \
//!     rabbitmq:3.13-alpine
//!
//! Then run:
//!
//!   cargo run -p rabbitmq-example
//!
//! Press Ctrl+C to stop.
//!
//! The `rmq`/`rmq` credentials belong to the ephemeral local fixture. They are
//! not a production secret; override `RABBITMQ_URL` for any other broker.

use std::collections::HashMap;

use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_rabbitmq::RabbitMqComponent;
use camel_component_rabbitmq::config::{RabbitBrokerConfig, RabbitComponentConfig};
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

/// Local Docker fixture URL. Explicit demo credentials, not a production secret.
const DEFAULT_BROKER_URL: &str = "amqp://rmq:rmq@127.0.0.1:5672/%2f";

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    let url = std::env::var("RABBITMQ_URL").unwrap_or_else(|_| DEFAULT_BROKER_URL.to_string());
    println!("Using RabbitMQ broker at {url}");

    let mut brokers = HashMap::new();
    brokers.insert(
        "main".to_string(),
        RabbitBrokerConfig {
            url,
            username: None,
            password: None,
            vhost: None,
        },
    );

    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap
    ctx.register_component(RabbitMqComponent::new(RabbitComponentConfig {
        brokers,
        reconnect: None,
    }));
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // Route 1 (Consumer): default exchange → queue `demo` → log.
    // `autoDeclare=true` declares the queue at consumer start; the producer
    // timer below publishes to the same queue every second.
    // ANCHOR: rabbitmq-consumer-route
    let consumer = RouteBuilder::from("rabbitmq:default?queue=demo&autoDeclare=true")
        .route_id("rabbitmq-consumer")
        .to("log:info?showHeaders=true")
        .build()?;
    // ANCHOR_END: rabbitmq-consumer-route

    // Route 2 (Producer): timer → publish to queue `demo` every second.
    // ANCHOR: rabbitmq-producer-route
    let producer = RouteBuilder::from("timer:tick?period=1000")
        .route_id("rabbitmq-producer")
        .set_body(Value::String(
            r#"{"event":"order","source":"rust-camel"}"#.to_string(),
        ))
        .to("rabbitmq:default?queue=demo")
        .build()?;
    // ANCHOR_END: rabbitmq-producer-route

    ctx.add_route_definition(consumer).await?;
    ctx.add_route_definition(producer).await?;

    println!("Starting RabbitMQ example... Press Ctrl+C to stop.\n");

    ctx.start().await?;

    tokio::signal::ctrl_c().await.ok();
    println!("\nShutting down...");
    ctx.stop().await?;
    println!("RabbitMQ example stopped.");
    Ok(())
}
