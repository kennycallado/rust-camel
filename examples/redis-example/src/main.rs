//! Redis example for rust-camel.
//!
//! Demonstrates:
//!   - Route 1 (String Producer): timer → SET key/value → log
//!   - Route 2 (Queue Consumer): from(redis://...?command=BRPOP) → log
//!   - Route 3 (Pub/Sub Consumer): from(redis://...?command=SUBSCRIBE) → log
//!   - Route 4 (Pub/Sub Producer): timer → PUBLISH → log
//!
//! A Redis instance is started automatically via testcontainers (requires Docker).
//! No external infrastructure needed — just run:
//!
//!   cargo run -p redis-example
//!
//! Press Ctrl+C to stop.

use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_redis::RedisComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::redis::Redis;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    println!("Starting Redis via testcontainers (requires Docker)...");
    let container = Redis::default()
        .start()
        .await
        .expect("Failed to start Redis container — is Docker running?");
    let port = container
        .get_host_port_ipv4(6379)
        .await
        .expect("Failed to get Redis port");
    let conn_str = format!("127.0.0.1:{port}");
    println!("Redis available at {conn_str}");

    // Seed demo-queue with items for the BRPOP consumer
    let client = redis::Client::open(format!("redis://{conn_str}")).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let () = redis::cmd("RPUSH")
        .arg("demo-queue")
        .arg("item-0")
        .arg("item-1")
        .arg("item-2")
        .exec_async(&mut conn)
        .await
        .unwrap();
    println!("Seeded 3 items into demo-queue\n");

    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(RedisComponent::new());
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // Route 1: Timer → SET string (every 3s, 3 times)
    let string_producer = RouteBuilder::from("timer:tick?period=3000&repeatCount=3")
        .route_id("redis-string-producer")
        .set_header("CamelRedis.Key", Value::String("greeting".into()))
        .set_header(
            "CamelRedis.Value",
            Value::String("hello from rust-camel!".into()),
        )
        .to(format!("redis://{conn_str}?command=SET"))
        .to("log:info?showHeaders=true")
        .build()?;

    // Route 2: BRPOP consumer on demo-queue
    let queue_consumer = RouteBuilder::from(&format!(
        "redis://{conn_str}?command=BRPOP&key=demo-queue&timeout=2"
    ))
    .route_id("redis-queue-consumer")
    .to("log:info?showAll=true")
    .build()?;

    // Route 3: Pub/Sub subscriber
    let pubsub_consumer = RouteBuilder::from(&format!(
        "redis://{conn_str}?command=SUBSCRIBE&channels=demo-channel"
    ))
    .route_id("redis-pubsub-consumer")
    .to("log:info?showAll=true")
    .build()?;

    // Route 4: Timer → PUBLISH (every 3s, 2 times)
    let pubsub_producer = RouteBuilder::from("timer:pub?period=3000&repeatCount=2")
        .route_id("redis-pubsub-producer")
        .set_header("CamelRedis.Channel", Value::String("demo-channel".into()))
        .set_header(
            "CamelRedis.Value",
            Value::String("pubsub message from rust-camel!".into()),
        )
        .to(format!("redis://{conn_str}?command=PUBLISH"))
        .to("log:info?showHeaders=true")
        .build()?;

    ctx.add_route_definition(string_producer).await?;
    ctx.add_route_definition(queue_consumer).await?;
    ctx.add_route_definition(pubsub_consumer).await?;
    ctx.add_route_definition(pubsub_producer).await?;

    println!("Starting Redis example... Press Ctrl+C to stop.\n");

    ctx.start().await?;

    tokio::signal::ctrl_c().await.ok();
    println!("\nShutting down...");
    ctx.stop().await?;
    println!("Redis example stopped.");
    Ok(())
}
