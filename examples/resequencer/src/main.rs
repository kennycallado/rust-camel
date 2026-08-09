//! Example: EIP Resequencer — reorder exchanges by a sequence number.
//!
//! The `set_header` step copies the auto-incrementing `CamelTimerCounter`
//! into a `seq` header. The `resequence.batch` step buffers per-region,
//! sorts by `seq`, and emits the buffered burst when the size-3 window
//! fills. See `routes.yaml` for the route declaration.

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

    println!("Resequencer example running.");
    println!("Timer fires every 100ms; resequencer batches 3 sorted emissions.");
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down resequencer example...");
    ctx.stop().await?;
    println!("Resequencer example stopped cleanly.");

    Ok(())
}
