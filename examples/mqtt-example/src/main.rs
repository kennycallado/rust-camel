//! MQTT example for rust-camel.
//!
//! Demonstrates:
//!   - Route 1 (Producer): timer → set_body("hello-mqtt") → to(mqtt://test/sensors/temp)
//!   - Route 2 (Consumer): from(mqtt://test/sensors/#) → log
//!
//! A Mosquitto broker is started automatically via testcontainers GenericImage (requires Docker).
//! No external infrastructure needed — just run:
//!
//!   cargo run -p mqtt-example
//!
//! Press Ctrl+C to stop.

use std::collections::HashMap;

use camel_api::CamelError;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_mqtt::MqttComponent;
use camel_component_mqtt::config::{MqttBrokerConfig, MqttConfig};
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use std::time::Duration;
use testcontainers::{
    GenericImage, ImageExt,
    core::{ContainerPort, WaitFor},
    runners::AsyncRunner,
};

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    println!("Starting Mosquitto via testcontainers (requires Docker)...");

    // Mosquitto 2.x needs an explicit listener + allow_anonymous.
    let conf = "listener 1883 0.0.0.0\nallow_anonymous true\n";
    let cmd = format!(
        "printf '{conf}' > /mosquitto/config/mosquitto.conf && exec mosquitto -c /mosquitto/config/mosquitto.conf -v"
    );
    let container = GenericImage::new("eclipse-mosquitto", "2.0.20")
        .with_exposed_port(ContainerPort::Tcp(1883))
        .with_wait_for(WaitFor::message_on_stdout("mosquitto version"))
        .with_cmd(["sh", "-c", &cmd])
        .with_startup_timeout(Duration::from_secs(60))
        .start()
        .await
        .expect("Failed to start Mosquitto container — is Docker running?"); // allow-unwrap

    let port = container
        .get_host_port_ipv4(1883)
        .await
        .expect("Failed to get Mosquitto port"); // allow-unwrap

    let broker_url = format!("mqtt://127.0.0.1:{port}");
    println!("Mosquitto available at {broker_url}");

    // Build MqttConfig with the Mosquitto broker as "test".
    let mut cfg = MqttConfig::default();
    let mut brokers = HashMap::new();
    brokers.insert(
        "test".to_string(),
        MqttBrokerConfig {
            url: broker_url,
            username: None,
            password: None,
            tls_ca_cert: None,
        },
    );
    cfg.brokers = brokers;

    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap
    ctx.register_component(
        MqttComponent::with_config(cfg).expect("valid MQTT config"), // allow-unwrap
    );
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // Route 1 (Producer): timer → publish "hello-mqtt" on sensors/temp every 3s, twice.
    let producer = RouteBuilder::from("timer:tick?period=3000&repeatCount=2")
        .route_id("mqtt-producer")
        .set_body("hello-mqtt")
        .to("mqtt://test/sensors/temp")
        .build()?;

    // Route 2 (Consumer): subscribe to sensors/# → log.
    let consumer = RouteBuilder::from("mqtt://test/sensors/#")
        .route_id("mqtt-consumer")
        .to("log:info?showHeaders=true")
        .build()?;

    ctx.add_route_definition(producer).await?;
    ctx.add_route_definition(consumer).await?;

    println!("Starting MQTT example... Press Ctrl+C to stop.\n");

    ctx.start().await?;

    tokio::signal::ctrl_c().await.ok();
    println!("\nShutting down...");
    ctx.stop().await?;
    println!("MQTT example stopped.");
    Ok(())
}
