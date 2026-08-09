//! Example: EIP Sampling — pass 1 of every N exchanges (deterministic).
//!
//! The `sampling: 3` step lets every third exchange through and drops
//! the rest by setting `CamelStop=true`. See `routes.yaml` for the route
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

    println!("Sampling example running.");
    println!("Timer fires every 200ms; sampling period=3 lets every 3rd exchange through.");
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down sampling example...");
    ctx.stop().await?;
    println!("Sampling example stopped cleanly.");

    Ok(())
}
