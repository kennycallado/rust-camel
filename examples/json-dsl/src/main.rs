//! # JSON DSL Route Loader Example
//!
//! Demonstrates the JSON DSL parser loading routes from a `.json` file.
//! Mirrors the `yaml-dsl` example using JSON route definitions instead.
//!
//! ```bash
//! cd examples/json-dsl && cargo run
//! ```

use std::path::Path;

use camel_api::CamelError;
use camel_component_direct::DirectComponent;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_dsl::json::load_json_from_file;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║         JSON DSL - Route Loader Example                       ║");
    println!("╚════════════════════════════════════════════════════════════════╝");
    println!();

    println!("[1] Creating Camel context and registering components...");

    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap

    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(DirectComponent::new());

    println!("    ✓ Components registered: timer, log, direct");

    println!();
    println!("[2] Loading routes from JSON file...");

    let config_path = Path::new("config/routes.json");

    let routes = load_json_from_file(config_path).map_err(|e| {
        println!("    ✗ Failed to load routes: {}", e);
        println!();
        println!("    Make sure you're running from the example directory:");
        println!("    cd examples/json-dsl && cargo run");
        e
    })?;

    println!("    ✓ Loaded {} route definitions", routes.len());

    println!();
    println!("[3] Inspecting loaded routes:");
    println!();

    for route in &routes {
        let cb = route.circuit_breaker_config().is_some();
        let flags = if cb { " [circuit_breaker]" } else { "" };
        println!(
            "    {:<25} | from: {}{}",
            route.route_id(),
            route.from_uri(),
            flags
        );
    }

    println!();
    println!("[4] Adding routes to context...");

    for route in routes {
        let route_id = route.route_id().to_string();
        ctx.add_route_definition(route).await?;
        println!("    ✓ Added: {}", route_id);
    }

    println!();
    println!("[5] Starting Camel context...");

    ctx.start().await?;

    println!("    ✓ Context started");
    println!();

    print_banner();

    println!("Running for 25 seconds to demonstrate all features...");
    println!();

    tokio::time::sleep(tokio::time::Duration::from_secs(25)).await;

    println!();
    println!("[6] Shutting down...");

    ctx.stop().await?;

    println!("    ✓ Context stopped");
    println!();
    println!("Example complete!");

    Ok(())
}

fn print_banner() {
    println!("────────────────────────────────────────────────────────────────");
    println!();
    println!("JSON DSL Features Demonstrated:");
    println!();
    println!("  BASIC STEPS:");
    println!("    • timer-to-log     - Basic pipeline with headers");
    println!("    • set-body-demo    - Set body to literal value");
    println!();
    println!("  CONTROL FLOW:");
    println!("    • filter-demo      - Filter with simple: expression");
    println!("    • choice-demo      - Content-based router (when/otherwise)");
    println!("    • stop-demo        - Stop pipeline early");
    println!();
    println!("  ENTERPRISE INTEGRATION PATTERNS:");
    println!("    • split-demo       - Split message by lines");
    println!("    • wiretap-demo     - Fire-and-forget tap to audit");
    println!();
    println!("────────────────────────────────────────────────────────────────");
    println!();
}
