//! # YAML DSL Route Loader Example
//!
//! This example demonstrates how to use the YAML DSL parser to load routes
//! from a YAML configuration file. This enables declarative route definitions
//! that can be modified without recompiling the application.
//!
//! ## Features Demonstrated
//!
//! - Loading routes from a YAML file
//! - Using all available step types (to, set_header, log)
//! - Configuring auto_startup and startup_order
//! - Error handling for YAML parsing
//! - Inspecting loaded route status
//!
//! ## YAML Route Structure
//!
//! ```yaml
//! routes:
//!   - id: "my-route"
//!     from: "timer:tick?period=1000"
//!     auto_startup: true
//!     startup_order: 100
//!     steps:
//!       - log: "Processing message"
//!       - set_header:
//!           key: "source"
//!           value: "yaml"
//!       - to: "log:info"
//! ```
//!
//! ## Available Step Types
//!
//! | Step Type | Description | Example |
//! |-----------|-------------|---------|
//! | `to` | Send to an endpoint | `- to: "log:info"` |
//! | `set_header` | Set a message header | `- set_header: { key: "x", value: "y" }` |
//! | `log` | Log a message | `- log: "Hello"` |
//!
//! ## Running the Example
//!
//! ```bash
//! cargo run -p yaml-dsl
//! ```

use std::path::Path;

use camel_api::CamelError;
use camel_component_direct::DirectComponent;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_dsl::{load_from_file, parse_yaml};

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    // Initialize tracing for logging output
    tracing_subscriber::fmt()
        .with_target(false)
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║              YAML DSL Route Loader Example                     ║");
    println!("╚════════════════════════════════════════════════════════════════╝");
    println!();

    // =========================================================================
    // Step 1: Create the Camel Context
    // =========================================================================
    // Register the components that our YAML routes will use.
    // The YAML file references: timer, log, and direct components.

    println!("[1] Creating Camel context and registering components...");

    let mut ctx = CamelContext::new();

    // Register components needed by the YAML routes
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(DirectComponent::new());

    println!("    ✓ Components registered: timer, log, direct");

    // =========================================================================
    // Step 2: Load Routes from YAML File
    // =========================================================================
    // The load_from_file function reads the YAML file and parses it
    // into RouteDefinition objects that can be added to the context.

    println!();
    println!("[2] Loading routes from YAML file...");

    let config_path = Path::new("config/routes.yaml");

    // Load routes from the file
    let routes = load_from_file(config_path).map_err(|e| {
        println!("    ✗ Failed to load routes: {}", e);
        println!();
        println!("    Make sure you're running from the example directory:");
        println!("    cd examples/yaml-dsl && cargo run");
        e
    })?;

    println!("    ✓ Loaded {} route definitions", routes.len());

    // =========================================================================
    // Step 3: Inspect Loaded Routes (Before Adding)
    // =========================================================================
    // Before adding routes to the context, we can inspect them.
    // This is useful for validation or logging purposes.

    println!();
    println!("[3] Inspecting loaded routes:");
    println!();

    for route in &routes {
        println!(
            "    Route: {:<20} | from: {}",
            route.route_id(),
            route.from_uri()
        );
        println!(
            "      auto_startup: {:<5} | startup_order: {} | steps: {}",
            route.auto_startup(),
            route.startup_order(),
            route.steps().len()
        );
    }

    // =========================================================================
    // Step 4: Add Routes to Context
    // =========================================================================
    // Add all loaded routes to the context. Routes will be started
    // in order of their startup_order value (lower = earlier).

    println!();
    println!("[4] Adding routes to context...");

    for route in routes {
        let route_id = route.route_id().to_string();
        ctx.add_route_definition(route)?;
        println!("    ✓ Added route: {}", route_id);
    }

    // =========================================================================
    // Step 5: Start the Context
    // =========================================================================
    // Starting the context initializes all routes with auto_startup=true.
    // Routes are started in order of their startup_order value.

    println!();
    println!("[5] Starting Camel context...");

    ctx.start().await?;

    println!("    ✓ Context started successfully");
    println!();

    // =========================================================================
    // Step 6: Display Running Status
    // =========================================================================

    print_banner();

    // =========================================================================
    // Step 7: Run and Demonstrate
    // =========================================================================
    // Let the routes run for a while to demonstrate functionality.
    // In a real application, you might wait for a shutdown signal.

    println!("Routes are now running. Demonstrating for 10 seconds...");
    println!();

    // Run for 10 seconds to show the routes working
    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;

    // =========================================================================
    // Step 8: Demonstrate Parsing YAML from String
    // =========================================================================
    // Besides loading from files, you can also parse YAML strings directly.
    // This is useful for embedded configurations or dynamic route creation.

    println!();
    println!("[6] Demonstrating parse_yaml from string...");

    let yaml_string = r#"
routes:
  - id: "dynamic-route"
    from: "timer:dynamic?period=5000&repeatCount=1"
    steps:
      - log: "This route was created from a YAML string!"
      - to: "log:dynamic-output"
"#;

    match parse_yaml(yaml_string) {
        Ok(dynamic_routes) => {
            println!("    ✓ Parsed {} route(s) from string", dynamic_routes.len());
            for route in &dynamic_routes {
                println!("      - {}", route.route_id());
            }
            // Note: We don't add these to the context since we're about to stop
        }
        Err(e) => {
            println!("    ✗ Failed to parse: {}", e);
        }
    }

    // =========================================================================
    // Step 9: Graceful Shutdown
    // =========================================================================

    println!();
    println!("[7] Shutting down gracefully...");

    ctx.stop().await?;

    println!("    ✓ Context stopped");
    println!();
    println!("Example complete!");

    Ok(())
}

fn print_banner() {
    println!("────────────────────────────────────────────────────────────────");
    println!();
    println!("YAML Routes Loaded:");
    println!();
    println!("  1. timer-to-log       (startup_order: 100)");
    println!("     Timer → Log with headers");
    println!();
    println!("  2. header-pipeline    (startup_order: 200)");
    println!("     Timer → Set Headers → Direct:processor");
    println!();
    println!("  3. direct-processor   (startup_order: 50, starts first!)");
    println!("     Direct → Log (receives from header-pipeline)");
    println!();
    println!("  4. disabled-route     (auto_startup: false)");
    println!("     Timer → Log (NOT STARTED)");
    println!();
    println!("  5. chained-pipeline   (startup_order: 150)");
    println!("     Timer → Multi-step pipeline with logging");
    println!();
    println!("Startup Order: direct-processor → timer-to-log → chained-pipeline");
    println!("              → header-pipeline");
    println!();
    println!("────────────────────────────────────────────────────────────────");
    println!();
}
