//! Exec component example for rust-camel.
//!
//! Demonstrates system-command execution via the exec component:
//!   1. `echo` profile — materialized, accepts any args, timeout 10s.
//!   2. `date` profile — no args, runs `date` and captures output.
//!
//! Configuration lives under `[components.exec]` / `[[components.exec.profiles]]`
//! in TOML, loaded by `ExecBundle::from_toml`. The component fails closed:
//! no profiles = nothing executes (ADR-0033, ADR-0036).
//!
//! Run:
//!   cargo run -p exec-example
//!
//! Press Ctrl+C to stop.

use camel_api::CamelError;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::ComponentBundle;
use camel_component_exec::ExecBundle;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

/// Register the Exec component with two profiles: echo and date.
fn register_exec_bundle(ctx: &mut CamelContext) {
    let toml_str = r#"
workspace_root = "."

[[profiles]]
name = "echo"
executable = "echo"
args = { allow = "any" }
timeout_secs = 10
working_dir = "."
accepted_exit_codes = [0]

[[profiles]]
name = "date"
executable = "date"
timeout_secs = 5
working_dir = "."
accepted_exit_codes = [0]
"#;
    let value: toml::Value = toml::from_str(toml_str).expect("parse toml"); // allow-unwrap
    let bundle = ExecBundle::from_toml(value).expect("bundle"); // allow-unwrap
    bundle.register_all(ctx);
}

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    println!("=== Exec Example ===\n");

    let mut ctx = CamelContext::builder().build().await?;
    register_exec_bundle(&mut ctx);
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // ------------------------------------------------------------------
    // Route 1: Echo command via exec profile
    // ------------------------------------------------------------------
    println!("=== Route 1: exec:echo (camel exec-runner profile) ===");
    println!("  timer -> set_header CamelExecArgs -> exec:echo -> log\n");

    let route1 = RouteBuilder::from("timer:exec-echo?period=500&repeatCount=1")
        .route_id("exec-echo")
        .set_body("hello from exec example")
        .set_header(
            camel_component_exec::headers::CAMEL_EXEC_ARGS,
            serde_json::json!(["-n", "Hello", "World"]),
        )
        .to("exec:echo")
        .to("log:info?showBody=true&showHeaders=true")
        .build()?;
    ctx.add_route_definition(route1).await?;

    // ------------------------------------------------------------------
    // Route 2: Date command (no args, just captures stdout)
    // ------------------------------------------------------------------
    println!("=== Route 2: exec:date (no args) ===");
    println!("  timer -> exec:date -> log\n");

    let route2 = RouteBuilder::from("timer:exec-date?period=500&repeatCount=1&delay=100")
        .route_id("exec-date")
        .to("exec:date")
        .to("log:info?showBody=true&showHeaders=true")
        .build()?;
    ctx.add_route_definition(route2).await?;

    println!("\nStarting exec example... Press Ctrl+C to stop.\n");

    ctx.start().await?;

    tokio::signal::ctrl_c().await.ok();
    println!("\nShutting down...");
    ctx.stop().await?;
    println!("Exec example stopped.");
    Ok(())
}
