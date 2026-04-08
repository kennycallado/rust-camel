//! JMS example for rust-camel.
//!
//! Demonstrates:
//!   - `activemq:` scheme — locks broker type to ActiveMQ Classic automatically.
//!   - Shorthand destination: `activemq:orders` is equivalent to `activemq:queue:orders`.
//!   - `broker=<name>` URI query param selects a configured broker at the endpoint level.
//!   - Route 1 (Producer): timer → set_body → to(activemq:queue:orders)
//!   - Route 2 (Consumer): from(activemq:orders) → log  (shorthand)
//!
//! An ActiveMQ Classic broker is started automatically via testcontainers
//! (requires Docker). No external infrastructure needed — just run:
//!
//!   cargo run -p jms-example
//!
//! The example runs for ~15 seconds then shuts down cleanly.
//!
//! ## URI scheme variants
//!
//! ```text
//! activemq:orders                   shorthand → queue (ActiveMQ Classic)
//! activemq:queue:orders             explicit type
//! activemq:topic:events             topic destination
//! artemis:orders                    shorthand → queue (ActiveMQ Artemis)
//! jms:queue:orders                  generic scheme — broker_type from config
//!
//! # Inline overrides via query params:
//! activemq:queue:orders?broker=secondary
//! jms:queue:orders?username=admin&password=secret
//! ```

use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_jms::{BrokerType, JmsBridgePool, JmsComponent, JmsPoolConfig};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use std::sync::Arc;
use testcontainers::{
    GenericImage,
    core::{ContainerPort, WaitFor},
    runners::AsyncRunner,
};

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    // Start an ActiveMQ Classic broker in Docker.
    // The container is kept alive for the duration of main() via _container.
    println!("Starting ActiveMQ Classic broker via testcontainers (requires Docker)...");
    let _container = GenericImage::new("apache/activemq-classic", "latest")
        .with_exposed_port(ContainerPort::Tcp(61616))
        .with_wait_for(WaitFor::message_on_stdout("Listening for connections at:"))
        .start()
        .await
        .expect("Failed to start ActiveMQ container — is Docker running?");
    let port = _container
        .get_host_port_ipv4(61616)
        .await
        .expect("Failed to get ActiveMQ port");
    let broker_url = format!("tcp://127.0.0.1:{port}");
    println!("ActiveMQ broker available at {broker_url}");

    // Register all three JMS schemes against a shared bridge pool.
    let pool_config = JmsPoolConfig::single_broker(&broker_url, BrokerType::ActiveMq);
    let pool = Arc::new(JmsBridgePool::from_config(pool_config).unwrap());

    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(JmsComponent::with_scheme("jms", Arc::clone(&pool)));
    ctx.register_component(JmsComponent::with_scheme("activemq", Arc::clone(&pool)));
    ctx.register_component(JmsComponent::with_scheme("artemis", Arc::clone(&pool)));

    // Route 1: Timer → JMS producer (every 3 seconds)
    // Uses explicit destination type: activemq:queue:orders
    let producer_route = RouteBuilder::from("timer:tick?period=3000")
        .route_id("jms-producer")
        .set_body(Value::String(
            r#"{"event":"order","source":"rust-camel"}"#.to_string(),
        ))
        .to("activemq:queue:orders")
        .build()?;

    // Route 2: JMS consumer → Log
    // Uses shorthand: activemq:orders is equivalent to activemq:queue:orders
    let consumer_route = RouteBuilder::from("activemq:orders")
        .route_id("jms-consumer")
        .to("log:info?showHeaders=true")
        .build()?;

    ctx.add_route_definition(producer_route).await?;
    ctx.add_route_definition(consumer_route).await?;

    println!("Starting JMS example... Will run for ~15 seconds then shut down.");

    ctx.start().await?;

    // Run for ~15 seconds, then shut down cleanly.
    tokio::time::sleep(std::time::Duration::from_secs(15)).await;
    ctx.stop().await?;
    println!("JMS example finished.");
    Ok(())
}
