//! Example: EIP Claim Check — stash a payload in a repository and
//! retrieve it later via a claim key.
//!
//! Demonstrates Set (stash body, replace with key) and Get (retrieve the
//! stashed body by key) operations against a memory claim check repository.
//!
//! See `routes.yaml` for the route declaration.

use std::path::Path;
use std::sync::Arc;

use camel_api::CamelError;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::claim_check::MemoryClaimCheckRepository;
use camel_core::context::CamelContext;
use camel_dsl::load_from_file;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // Register a custom-named memory claim check repository.
    // ("memory" is registered by default; we register "vault" to show the API.)
    ctx.register_claim_check_repository(
        "vault",
        Arc::new(MemoryClaimCheckRepository::new("vault")),
    )
    .unwrap(); // allow-unwrap

    let routes_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("routes.yaml");
    let routes = load_from_file(&routes_path)?;

    for route in routes {
        ctx.add_route_definition(route).await?;
    }

    ctx.start().await?;

    println!("Claim check example running.");
    println!("Each tick stashes the body, replaces it with a key, then retrieves it.");
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down claim check example...");
    ctx.stop().await?;
    println!("Claim check example stopped cleanly.");

    Ok(())
}
