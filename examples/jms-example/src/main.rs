//! JMS example for rust-camel.
//!
//! Demonstrates:
//!   - Route 1 (Producer): timer → set_body → to(jms:queue:orders)
//!   - Route 2 (Consumer): from(jms:queue:orders) → log
//!
//! An ActiveMQ Classic broker is started automatically via testcontainers
//! (requires Docker). No external infrastructure needed — just run:
//!
//!   cargo run -p jms-example
//!
//! The example runs for ~15 seconds then shuts down cleanly.

use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_jms::{BrokerType, JmsComponent, JmsConfig};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
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
        .with_wait_for(WaitFor::message_on_stdout(
            "Listening for connections at:",
        ))
        .start()
        .await
        .expect("Failed to start ActiveMQ container — is Docker running?");
    let port = _container
        .get_host_port_ipv4(61616)
        .await
        .expect("Failed to get ActiveMQ port");
    let broker_url = format!("tcp://127.0.0.1:{port}");
    println!("ActiveMQ broker available at {broker_url}");

    let jms_config = JmsConfig {
        broker_url: broker_url.clone(),
        broker_type: BrokerType::ActiveMq,
        ..Default::default()
    };

    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(JmsComponent::new(jms_config));

    // Route 1: Timer → JMS producer (every 3 seconds)
    let producer_route = RouteBuilder::from("timer:tick?period=3000")
        .route_id("jms-producer")
        .set_body(Value::String(
            r#"{"event":"order","source":"rust-camel"}"#.to_string(),
        ))
        .to("jms:queue:orders")
        .build()?;

    // Route 2: JMS consumer → Log
    let consumer_route = RouteBuilder::from("jms:queue:orders")
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
