//! JMS + Container example for rust-camel.
//!
//! Demonstrates using `camel-component-container` to manage the broker lifecycle
//! instead of testcontainers — the ActiveMQ Classic broker is started via a
//! Camel route and cleaned up on shutdown.
//!
//! Routes:
//!   - Route 1 (Queue Producer): timer → set_body → to(jms:queue:orders)
//!   - Route 2 (Queue Consumer): from(jms:queue:orders) → log
//!   - Route 3 (Topic Producer): timer → set_body → to(jms:topic:events)
//!   - Route 4 (Topic Consumer): from(jms:topic:events) → log
//!
//! The broker is started on port 61616 (fixed). If that port is already in use,
//! change the `ports` parameter in the `container:run` URI below.
//!
//! Requires a running Docker daemon. Just run:
//!
//!   cargo run -p jms-container-example
//!
//! The example runs for ~20 seconds then shuts down cleanly.

use std::net::TcpStream;
use std::time::{Duration, Instant};

use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_container::{ContainerComponent, cleanup_tracked_containers};
use camel_component_jms::{BrokerType, JmsComponent, JmsConfig};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

const BROKER_URL: &str = "tcp://127.0.0.1:61616";
const ACTIVEMQ_IMAGE: &str = "apache/activemq-classic:latest";

/// Poll TCP until the broker port is reachable or timeout expires.
async fn wait_for_broker(addr: &str, timeout: Duration) -> Result<(), CamelError> {
    let start = Instant::now();
    // addr is "host:port" — strip the scheme for TcpStream
    let tcp_addr = addr.trim_start_matches("tcp://");
    loop {
        if TcpStream::connect(tcp_addr).is_ok() {
            return Ok(());
        }
        if start.elapsed() >= timeout {
            return Err(CamelError::ProcessorError(format!(
                "Broker at {addr} did not become reachable within {}s",
                timeout.as_secs()
            )));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    // --- Step 1: Start the ActiveMQ broker via camel-component-container ---
    //
    // We use a temporary CamelContext just to execute the `container:run` action
    // imperatively (repeatCount=1 timer). The container is tracked globally by
    // `camel-component-container` and cleaned up via `cleanup_tracked_containers()`
    // on shutdown.
    println!("Starting ActiveMQ Classic broker via camel-component-container...");
    println!("  Image : {ACTIVEMQ_IMAGE}");
    println!("  Port  : 61616 (fixed — change if already in use)");

    let mut init_ctx = CamelContext::new();
    init_ctx.register_component(TimerComponent::new());
    init_ctx.register_component(ContainerComponent::new());

    // Route: fire once → run the container
    let start_route = RouteBuilder::from("timer:init?repeatCount=1&delay=0")
        .route_id("broker-start")
        .to(format!(
            "container:run?image={ACTIVEMQ_IMAGE}&name=activemq-jms-example&ports=61616:61616"
        ))
        .build()?;

    init_ctx.add_route_definition(start_route).await?;
    init_ctx.start().await?;
    // Give the route time to fire (timer fires on first tick after delay=0)
    tokio::time::sleep(Duration::from_secs(2)).await;
    init_ctx.stop().await?;

    // --- Step 2: Wait until the broker port is reachable ---
    println!("Waiting for broker to be ready...");
    wait_for_broker(BROKER_URL, Duration::from_secs(30)).await?;
    println!("Broker ready at {BROKER_URL}");

    // --- Step 3: Build the main context with JMS routes ---
    let jms_config = JmsConfig {
        broker_url: BROKER_URL.to_string(),
        broker_type: BrokerType::ActiveMq,
        ..Default::default()
    };

    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(JmsComponent::new(jms_config));

    // Route 1: Queue producer — sends an order event every 3 seconds
    let queue_producer = RouteBuilder::from("timer:queue-tick?period=3000")
        .route_id("queue-producer")
        .set_body(Value::String(
            r#"{"event":"order","source":"rust-camel","type":"queue"}"#.to_string(),
        ))
        .to("jms:queue:orders")
        .build()?;

    // Route 2: Queue consumer — receives from the orders queue
    let queue_consumer = RouteBuilder::from("jms:queue:orders")
        .route_id("queue-consumer")
        .to("log:info?showHeaders=true")
        .build()?;

    // Route 3: Topic producer — broadcasts a telemetry event every 4 seconds
    let topic_producer = RouteBuilder::from("timer:topic-tick?period=4000")
        .route_id("topic-producer")
        .set_body(Value::String(
            r#"{"event":"telemetry","source":"rust-camel","type":"topic"}"#.to_string(),
        ))
        .to("jms:topic:events")
        .build()?;

    // Route 4: Topic consumer — subscribes to the events topic
    let topic_consumer = RouteBuilder::from("jms:topic:events")
        .route_id("topic-consumer")
        .to("log:info?showHeaders=true")
        .build()?;

    ctx.add_route_definition(queue_producer).await?;
    ctx.add_route_definition(queue_consumer).await?;
    ctx.add_route_definition(topic_producer).await?;
    ctx.add_route_definition(topic_consumer).await?;

    println!("Starting JMS routes (queue + topic)... Will run for ~20 seconds then shut down.");
    ctx.start().await?;

    tokio::time::sleep(Duration::from_secs(20)).await;

    ctx.stop().await?;

    // Stop and remove the ActiveMQ container
    println!("Cleaning up broker container...");
    cleanup_tracked_containers().await;

    println!("Done.");
    Ok(())
}
