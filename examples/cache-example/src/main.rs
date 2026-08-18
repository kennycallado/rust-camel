//! Example: EIP Cache — cache a computed body, invalidate entries, and
//! serve stale data after TTL expiry.
//!
//! Demonstrates the three cache step kinds:
//! - `cache`: lookup by key, run `on_miss` on miss, write back the result
//! - `cache_invalidate`: remove a single key from the cache
//! - `cache_peek_stale`: serve a post-expiry entry (stale-read fallback)
//! - `circuit_breaker.fallback`: serve a stale entry when the circuit opens
//!
//! The default `"memory"` cache repository (moka-backed, size-eviction)
//! is registered automatically by `CamelContext::builder().build()`.
//!
//! See `routes.yaml` for the route declarations.

use std::path::Path;

use camel_api::CamelError;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_dsl::load_from_file;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // The default "memory" cache repository is registered automatically.
    // No manual registration is needed for this example.
    // To use the persistent redb backend instead, add this to Camel.toml:
    //   [default.cache_repo]
    //   backend = "redb"
    //   path = "data/cache.redb"
    //   stale_retention = "168h"
    //   cache_size = "256MiB"

    let routes_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("routes.yaml");
    let routes = load_from_file(&routes_path)?;

    for route in routes {
        ctx.add_route_definition(route).await?;
    }

    ctx.start().await?;

    println!("Cache example running.");
    println!("Route 1: caches a computed body with 5s TTL.");
    println!("Route 2: invalidates the cached entry.");
    println!("Route 3: serves a stale entry after TTL expiry.");
    println!("Route 5: serves a stale entry when the circuit opens.");
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down cache example...");
    ctx.stop().await?;
    println!("Cache example stopped cleanly.");

    Ok(())
}
