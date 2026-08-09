//! Example: EIP Sort — order a body array by a simple-language expression.
//!
//! The route injects a small JSON array on every timer tick, then the
//! `sort` step orders it numerically. See `routes.yaml` for the route
//! declaration.

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

    let routes_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("routes.yaml");
    let routes = load_from_file(&routes_path)?;

    for route in routes {
        ctx.add_route_definition(route).await?;
    }

    ctx.start().await?;

    println!("Sort example running.");
    println!("Each timer tick sets the body to a JSON array; the sort step orders it.");
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down sort example...");
    ctx.stop().await?;
    println!("Sort example stopped cleanly.");

    Ok(())
}
