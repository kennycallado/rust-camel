//! Redis Sentinel failover example for rust-camel.
//!
//! Demonstrates a producer and a subscriber that both run through a Redis
//! Sentinel topology using the `redis-sentinel://` URI scheme. The component
//! resolves the current master through the sentinels on every connection, so a
//! failover is picked up on the next reconnect.
//!
//! Prerequisites: a running Redis Sentinel topology (a master, one or more
//! replicas, and one or more sentinels). This example does NOT provision one.
//! Point it at an existing topology with the `REDIS_SENTINEL_NODES` and
//! `REDIS_MASTER_NAME` environment variables:
//!
//!   REDIS_SENTINEL_NODES="127.0.0.1:26379" REDIS_MASTER_NAME="mymaster" \
//!     cargo run -p redis-sentinel
//!
//! Defaults: sentinel nodes `127.0.0.1:26379`, master name `mymaster`.
//! Press Ctrl+C to stop.

use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_redis::{RedisComponent, RedisSentinelComponent};
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    let sentinel_nodes =
        std::env::var("REDIS_SENTINEL_NODES").unwrap_or_else(|_| "127.0.0.1:26379".to_string());
    let master_name = std::env::var("REDIS_MASTER_NAME").unwrap_or_else(|_| "mymaster".to_string());
    let sentinel_uri = format!("redis-sentinel://{sentinel_nodes}/{master_name}/0");

    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap
    ctx.register_component(RedisComponent::new());
    // The registry resolves URIs by scheme, so redis-sentinel:// routes need
    // the sentinel-scheme component registered as well.
    ctx.register_component(RedisSentinelComponent::new());
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // Producer: timer → SET through Sentinel → log. The producer caches one
    // multiplexed connection: the master is resolved through the sentinel
    // nodes on first use and re-resolved on every reconnect, so a failover is
    // picked up then (not on each SET).
    let producer = RouteBuilder::from("timer:tick?period=3000&repeatCount=3")
        .route_id("redis-sentinel-producer")
        .set_header("CamelRedis.Key", Value::String("greeting".into()))
        .set_header(
            "CamelRedis.Value",
            Value::String("hello via redis sentinel!".into()),
        )
        .to(format!("{sentinel_uri}?command=SET"))
        .to("log:info?showHeaders=true")
        .build()?;

    // Consumer: subscribe to the `greeting` channel through Sentinel. Each
    // reconnect re-resolves the current master and re-subscribes.
    let subscriber =
        RouteBuilder::from(format!("{sentinel_uri}?command=SUBSCRIBE&channels=greeting").as_str())
            .route_id("redis-sentinel-subscriber")
            .to("log:info?showHeaders=true")
            .build()?;

    ctx.add_route_definition(producer).await?;
    ctx.add_route_definition(subscriber).await?;

    println!("Starting Redis Sentinel example... Press Ctrl+C to stop.\n");

    ctx.start().await?;

    tokio::signal::ctrl_c().await.ok();
    println!("\nShutting down...");
    ctx.stop().await?;
    println!("Redis Sentinel example stopped.");
    Ok(())
}
