//! Redis cache and idempotent repository example for rust-camel.
//!
//! Runs two routes against Redis-backed repositories, both configured in
//! `Camel.toml` and registered by `CamelConfig::configure_context` under the
//! name `"redis"`:
//! - a `cache` step that reads and writes its entries in Redis
//! - an `idempotent_consumer` that records deduplication keys in Redis
//!
//! Entries and keys survive a process restart and are shared with every
//! process that connects to the same Redis.
//!
//! Prerequisites: a running Redis at `127.0.0.1:6379`. This example does NOT
//! provision one. Point it at another instance (and database, via the `?db=N`
//! query parameter) with the `REDIS_URL` environment variable:
//!
//!   REDIS_URL="redis://redis.internal:6379?db=2" cargo run
//!
//! Both repositories connect eagerly at construction, so startup fails fast
//! when Redis is unreachable. Press Ctrl+C to stop.

use std::path::Path;

use camel_api::CamelError;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_config::CamelConfig;
use camel_dsl::load_from_file;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    let mut config =
        CamelConfig::from_file("Camel.toml").map_err(|e| CamelError::Config(e.to_string()))?;

    // REDIS_URL overrides the url of both redis repositories. The CAMEL_* env
    // override allowlist has no URL field, so the override is applied here on
    // the loaded config, before the repositories connect.
    if let Ok(url) = std::env::var("REDIS_URL") {
        if let Some(cache_repo) = config.cache_repo.as_mut() {
            cache_repo.url = Some(url.clone());
        }
        if let Some(idempotent_repo) = config.idempotent_repo.as_mut() {
            idempotent_repo.url = Some(url);
        }
    }

    // Connects both redis repositories eagerly; fails fast when Redis is
    // unreachable.
    let mut ctx = CamelConfig::configure_context(&config).await?;
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let routes_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("routes.yaml");
    let routes = load_from_file(&routes_path)?;
    for route in routes {
        ctx.add_route_definition(route).await?;
    }

    ctx.start().await?;

    println!("Redis repositories example running.");
    println!("Route 1 caches a computed body in the redis cache repository.");
    println!("Route 2 processes only the first tick; the rest are duplicates.");
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down...");
    ctx.stop().await?;
    println!("Redis repositories example stopped.");
    Ok(())
}
