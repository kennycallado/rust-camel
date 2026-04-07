//! # Hot Reload YAML Example — File-Watcher Live Route Updates
//!
//! Demonstrates `watch_and_reload`: a background task that watches YAML route
//! files on disk and applies diffs to the live context with zero downtime.
//!
//! ## What this example shows
//!
//! - Loading initial routes from a YAML file via `camel_dsl::discover_routes`
//! - Starting `watch_and_reload` with a `CancellationToken` for graceful shutdown
//! - Reading `watch_debounce_ms` from `Camel.toml` to configure debounce delay
//! - Editing the YAML file while the example is running → the pipeline swaps
//!   atomically without stopping the route or losing messages
//!
//! ## Running the example
//!
//! ```bash
//! cd examples/hot-reload-yaml && cargo run
//! ```
//!
//! While it is running, edit `routes/route.yaml` in a separate terminal — for
//! example change `V1` to `V2` — and save the file. The running context will
//! pick up the change within the configured debounce window (default 300 ms).
//!
//! ## How it works
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │  File system                                                    │
//! │  routes/route.yaml  ──(inotify/FSEvents)──► watch_and_reload   │
//! │                                               │                 │
//! │                                   debounce (watch_debounce_ms) │
//! │                                               │                 │
//! │                                      discover_routes()         │
//! │                                               │                 │
//! │                                      compute_reload_actions    │
//! │                                               │                 │
//! │                              Swap / Add / Remove on controller  │
//! └─────────────────────────────────────────────────────────────────┘
//! ```

use std::path::PathBuf;

use camel_api::CamelError;
use camel_component_log::LogComponent;
use camel_component_mock::MockComponent;
use camel_component_timer::TimerComponent;
use camel_config::CamelConfig;
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
    println!("║        Hot Reload YAML — File-Watcher Live Route Updates       ║");
    println!("╚════════════════════════════════════════════════════════════════╝");
    println!();

    // ── 0. Load configuration (watch_debounce_ms etc.) ───────────────────────
    // Falls back to the CamelConfig field default (300 ms) if Camel.toml is absent.
    let debounce_ms = CamelConfig::from_file("Camel.toml")
        .map(|c| c.watch_debounce_ms)
        .unwrap_or(300);
    println!("[0] watch_debounce_ms = {debounce_ms} ms  (set in Camel.toml)");

    // ── 1. Resolve the routes directory ──────────────────────────────────────
    //
    // We watch `routes/*.yaml` relative to the example's working directory.
    // When running with `cargo run` from the example directory the CWD is the
    // package root, so `routes/route.yaml` resolves correctly.
    let pattern = "routes/*.yaml".to_string();
    let patterns = vec![pattern.clone()];

    let routes_path = PathBuf::from("routes/route.yaml");
    if !routes_path.exists() {
        eprintln!("ERROR: routes/route.yaml not found.");
        eprintln!("Make sure you run from the example directory:");
        eprintln!("  cd examples/hot-reload-yaml && cargo run");
        std::process::exit(1);
    }

    // ── 2. Build context and register components ──────────────────────────────
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(MockComponent::new()); // available for YAML routes

    println!("[1] Components registered: timer, log, mock");

    // ── 3. Discover and load initial routes ───────────────────────────────────
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

    // ── 4. Start context ──────────────────────────────────────────────────────
    ctx.start().await?;
    println!("[3] Context started");
    println!();

    // ── 5. Start file watcher with graceful-shutdown token ───────────────────
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
            // debounce loaded from Camel.toml → watch_debounce_ms
            std::time::Duration::from_millis(debounce_ms),
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
    println!("  Try editing routes/route.yaml in another terminal, e.g.:");
    println!("    - Change V1 to V2 in the log message");
    println!("    - Add a second route with a different timer period");
    println!("    - Remove the route entirely");
    println!();

    // ── 6. Wait for Ctrl+C ────────────────────────────────────────────────────
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
