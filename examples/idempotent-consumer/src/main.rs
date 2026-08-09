//! Example: EIP Idempotent Consumer — reject duplicate exchanges by
//! message ID using a memory-backed idempotent repository.
//!
//! The route sets a fixed `messageId` header on every timer tick. The
//! idempotent consumer registers the key on first delivery; subsequent
//! deliveries of the same key are detected as duplicates and skipped
//! (outcome-aware segment, ADR-0025).
//!
//! See `routes.yaml` for the route declaration.

use std::path::Path;
use std::sync::Arc;

use camel_api::CamelError;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_core::idempotent::MemoryIdempotentRepository;
use camel_dsl::load_from_file;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // Register a custom-named memory idempotent repository.
    // ("memory" is registered by default; we register "dedupe" to show the API.)
    ctx.register_idempotent_repository(
        "dedupe",
        Arc::new(MemoryIdempotentRepository::new("dedupe")),
    )
    .unwrap(); // allow-unwrap

    let routes_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("routes.yaml");
    let routes = load_from_file(&routes_path)?;

    for route in routes {
        ctx.add_route_definition(route).await?;
    }

    ctx.start().await?;

    println!("Idempotent consumer example running.");
    println!("Only the first timer tick is processed; the rest are duplicates.");
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down idempotent consumer example...");
    ctx.stop().await?;
    println!("Idempotent consumer example stopped cleanly.");

    Ok(())
}
