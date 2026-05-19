//! WASM plugin processor example.
//!
//! Runs a pre-built echo plugin (`fixtures/echo.wasm`) through a
//! timer → WASM → log route with Phase 4 hardening params:
//! - `timeout=5`           → epoch-based 5 s per-call deadline
//! - `max-memory=10485760` → 10 MB linear memory cap
//!
//! # Running
//!
//! ```bash
//! cargo run -p wasm-example
//! ```
//!
//! # Building your own plugin
//!
//! See `fixtures/` for the guest source and README for build instructions.

use std::sync::{Arc, Mutex};

use camel_api::CamelError;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_component_wasm::WasmComponent;
use camel_core::Registry;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    // Resolve fixtures/ relative to this source file so the example works
    // regardless of the working directory.
    let fixtures_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures");

    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap

    let registry = Arc::new(Mutex::new(Registry::new()));
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    // base_dir = fixtures/ → "echo.wasm" resolves to fixtures/echo.wasm
    ctx.register_component(WasmComponent::new(registry, fixtures_dir));

    // Phase 4 hardening: 5 s timeout, 10 MB memory cap
    let route = RouteBuilder::from("timer:tick?period=1000&repeatCount=3")
        .route_id("wasm-example")
        .to("wasm:echo.wasm?timeout=5&max-memory=10485760")
        .to("log:info")
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("WASM example: 3 ticks through echo plugin, then exit.");
    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

    ctx.stop().await?;
    Ok(())
}
