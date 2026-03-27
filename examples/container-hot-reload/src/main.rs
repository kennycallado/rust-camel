//! # Container Hot-Reload Example — Docker Events with Live Route Updates
//!
//! Demonstrates hot-reload with the container component: watches YAML route files
//! and applies changes while Docker events continue to flow.
//!
//! ## What this example shows
//!
//! - Loading initial routes from YAML via `camel_dsl::discover_routes`
//! - Using `container:events` consumer to monitor Docker events
//! - Starting `watch_and_reload` for live route updates
//! - Proper cleanup with `cleanup_tracked_containers()` on shutdown
//!
//! ## Running the example
//!
//! ```bash
//! cd examples/container-hot-reload && cargo run
//! ```
//!
//! While it is running, edit `routes/route.yaml` in a separate terminal —
//! the running context will pick up the change within ~300 ms.

use std::path::PathBuf;

use camel_api::CamelError;
use camel_component_container::{cleanup_tracked_containers, ContainerComponent};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::reload_watcher::{resolve_watch_dirs, watch_and_reload};
use camel_core::CamelContext;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║     Container Hot-Reload — Docker Events with Live Updates     ║");
    println!("╚════════════════════════════════════════════════════════════════╝");
    println!();

    let pattern = "routes/*.yaml".to_string();
    let patterns = vec![pattern.clone()];

    let routes_path = PathBuf::from("routes/route.yaml");
    if !routes_path.exists() {
        eprintln!("ERROR: routes/route.yaml not found.");
        eprintln!("Make sure you run from the example directory:");
        eprintln!("  cd examples/container-hot-reload && cargo run");
        std::process::exit(1);
    }

    let mut ctx = CamelContext::new();
    ctx.register_component(ContainerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(TimerComponent::new());

    println!("[1] Components registered: container, log");

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
        println!("    • {} (from: {})", def.route_id(), def.from_uri());
    }

    for def in initial_defs {
        ctx.add_route_definition(def).await?;
    }

    ctx.start().await?;
    println!("[3] Context started — watching Docker events");
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
        )
        .await;
        if let Err(e) = result {
            tracing::error!("File watcher error: {e}");
        }
    });

    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║  Watching Docker events. Edit routes/route.yaml to reload.   ║");
    println!("║  Press Ctrl+C to stop.                                        ║");
    println!("╚════════════════════════════════════════════════════════════════╝");
    println!();
    println!("  Try running a container in another terminal:");
    println!("    docker run --rm alpine echo 'hello'");
    println!();

    tokio::signal::ctrl_c()
        .await
        .expect("Failed to listen for Ctrl+C");

    println!();
    println!("Shutting down...");
    shutdown.cancel();
    ctx.stop().await?;

    cleanup_tracked_containers().await;
    println!("Done.");

    Ok(())
}
