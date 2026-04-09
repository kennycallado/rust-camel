//! Hot Reload Example: Zero-Downtime Pipeline Swapping via File Watcher
//!
//! This example demonstrates how to use `watch_and_reload` to hot-reload routes
//! from YAML files with zero downtime when files change on disk.
//!
//! # Key Concepts Demonstrated
//!
//! 1. **Zero-Downtime Swap**: The route continues processing messages during the swap
//! 2. **Atomic Transition**: All new requests immediately see the new pipeline
//! 3. **In-Flight Safety**: Requests already processing complete with the old pipeline
//! 4. **File Watcher**: Automatic reload when route files change
//!
//! # Running the Example
//!
//! ```bash
//! cd examples/hot-reload && cargo run
//! ```
//!
//! While running, edit `routes/route.yaml` in another terminal to see live reloads.

use std::path::PathBuf;

use camel_api::CamelError;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::CamelContext;
use camel_core::reload_watcher::{resolve_watch_dirs, watch_and_reload};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║          Hot Reload Example: Zero-Downtime Pipeline Swap       ║");
    println!("╚════════════════════════════════════════════════════════════════╝");
    println!();

    let pattern = "routes/*.yaml".to_string();
    let patterns = vec![pattern.clone()];

    let routes_path = PathBuf::from("routes/route.yaml");
    if !routes_path.exists() {
        eprintln!("ERROR: routes/route.yaml not found.");
        eprintln!("Make sure you run from the example directory:");
        eprintln!("  cd examples/hot-reload && cargo run");
        std::process::exit(1);
    }

    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    println!("[1] Components registered: timer, log");

    let initial_defs = camel_dsl::discover_routes(&patterns).map_err(|e| {
        eprintln!("Failed to load initial routes: {e}");
        CamelError::RouteError(e.to_string())
    })?;

    println!(
        "[2] Loaded {} route(s) from {}",
        initial_defs.len(),
        pattern
    );
    for def in &initial_defs {
        println!("    - {} (from: {})", def.route_id(), def.from_uri());
    }

    for def in initial_defs {
        ctx.add_route_definition(def).await?;
    }

    ctx.start().await?;
    println!("[3] Context started");
    println!();

    let ctrl = ctx.runtime_execution_handle();
    let watch_patterns = patterns.clone();
    let shutdown = CancellationToken::new();
    let shutdown_watcher = shutdown.clone();

    tokio::spawn(async move {
        let watch_dirs = resolve_watch_dirs(&watch_patterns);
        let result = watch_and_reload(
            watch_dirs,
            ctrl,
            move || {
                camel_dsl::discover_routes(&watch_patterns)
                    .map_err(|e| CamelError::RouteError(e.to_string()))
            },
            Some(shutdown_watcher),
            std::time::Duration::from_secs(10),
            std::time::Duration::from_millis(300),
        )
        .await;
        if let Err(e) = result {
            tracing::error!("File watcher error: {e}");
        }
    });

    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║  Watcher active. Edit routes/route.yaml to see live reloads.  ║");
    println!("║  Press Ctrl+C to stop.                                        ║");
    println!("╚════════════════════════════════════════════════════════════════╝");
    println!();
    println!("  Try editing routes/route.yaml in another terminal:");
    println!("    - Change the log message content");
    println!("    - Modify the timer period");
    println!();

    tokio::signal::ctrl_c()
        .await
        .expect("Failed to listen for Ctrl+C");

    println!();
    println!("Shutting down...");
    shutdown.cancel();
    ctx.stop().await?;
    println!("Done.");

    Ok(())
}
