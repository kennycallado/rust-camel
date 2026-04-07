//! # Container Nginx Example — Web Server with Logs Streaming
//!
//! Demonstrates running a web server (nginx) with host networking
//! and streaming its logs through the route.
//!
//! ## Running the example
//!
//! ```bash
//! cd examples/container-nginx && cargo run
//! ```
//!
//! Then open http://localhost:80 in your browser.

use std::path::PathBuf;

use camel_api::CamelError;
use camel_component_container::{ContainerComponent, cleanup_tracked_containers};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║          Container Nginx — Web Server with Port Mapping        ║");
    println!("╚════════════════════════════════════════════════════════════════╝");
    println!();

    let routes_path = PathBuf::from("routes/route.yaml");
    if !routes_path.exists() {
        eprintln!("ERROR: routes/route.yaml not found.");
        eprintln!("Make sure you run from the example directory:");
        eprintln!("  cd examples/container-nginx && cargo run");
        std::process::exit(1);
    }

    let mut ctx = CamelContext::new();
    ctx.register_component(ContainerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(TimerComponent::new());

    println!("[1] Components registered: container, log, timer");

    let patterns = vec!["routes/*.yaml".to_string()];
    let initial_defs = camel_dsl::discover_routes(&patterns).map_err(|e| {
        eprintln!("Failed to load initial routes: {e}");
        CamelError::RouteError(e.to_string())
    })?;

    println!("[2] Loaded {} route(s)", initial_defs.len());
    for def in &initial_defs {
        println!("    • {} (from: {})", def.route_id(), def.from_uri());
    }

    for def in initial_defs {
        ctx.add_route_definition(def).await?;
    }

    ctx.start().await?;
    println!("[3] Context started");
    println!();

    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║  Nginx will start in 3 seconds...                              ║");
    println!("║  Using network=host - accessible at http://localhost:80        ║");
    println!("║  Press Ctrl+C to stop.                                         ║");
    println!("╚════════════════════════════════════════════════════════════════╝");
    println!();

    tokio::signal::ctrl_c()
        .await
        .expect("Failed to listen for Ctrl+C");

    println!();
    println!("Shutting down...");
    ctx.stop().await?;

    cleanup_tracked_containers().await;
    println!("Done.");

    Ok(())
}
