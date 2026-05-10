//! # Function Deno Enrich Example
//!
//! Timer triggers every 2s → set_body → Deno function (uppercase + header) → log.
//!
//! ## Prerequisites
//!
//! ```bash
//! cd crates/services/camel-function
//! docker build -t kennycallado/deno-runner:latest runner/
//! ```
//!
//! ## Running
//!
//! ```bash
//! cd examples/function-deno-enrich && cargo run
//! ```

use std::path::PathBuf;

use camel_api::CamelError;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_function::{ContainerProvider, FunctionConfig, FunctionRuntimeService};

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("=== Function Deno Enrich Example ===");
    println!();

    let provider = ContainerProvider::builder()
        .image("kennycallado/deno-runner:latest")
        .build()
        .map_err(|e| CamelError::Config(e.to_string()))?;

    let config = FunctionConfig::default();
    let service = FunctionRuntimeService::with_container_provider(config, provider);

    let mut ctx = CamelContext::builder()
        .with_lifecycle(service)
        .build()
        .await?;

    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    println!("[1] Context built with function runtime");

    let pattern = "routes/*.yaml".to_string();
    let patterns = vec![pattern.clone()];

    let routes_path = PathBuf::from("routes/route.yaml");
    if !routes_path.exists() {
        eprintln!("ERROR: routes/route.yaml not found.");
        eprintln!("  cd examples/function-deno-enrich && cargo run");
        std::process::exit(1);
    }

    let initial_defs = camel_dsl::discover_routes(&patterns).map_err(|e| {
        eprintln!("Failed to load routes: {e}");
        CamelError::RouteError(e.to_string())
    })?;

    println!("[2] Loaded {} route(s)", initial_defs.len());

    for def in initial_defs {
        ctx.add_route_definition(def).await?;
    }

    ctx.start().await?;
    println!("[3] Running. Messages every 2s. Ctrl+C to stop.");

    tokio::signal::ctrl_c().await.expect("Ctrl+C");

    println!("Shutting down...");
    ctx.stop().await?;
    println!("Done.");

    Ok(())
}
