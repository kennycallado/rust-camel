//! # Tracer EIP Demo
//!
//! Demonstrates automatic message flow tracing using the Tracer EIP.
//!
//! Four sections show progressively more detail:
//!
//! ## Section 1 — Minimal
//! Fields: `correlation_id`, `route_id`, `step_id`, `step_index`, `timestamp`,
//! `duration_ms`, `status`
//!
//! ## Section 2 — Medium
//! All Minimal fields plus: `headers_count`, `body_type`, `has_error`, `output_body_type`
//!
//! ## Section 3 — Full
//! All Medium fields plus: `header_0`, `header_1`, `header_2` (up to 3 headers)
//!
//! ## Section 4 — Error
//! Shows `status="error"`, `error`, and `error_type` fields on a failing processor
//!
//! Run with:
//! ```bash
//! cargo run -p tracer-demo
//! ```

use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::{
    CamelContext, DetailLevel, OutputFormat, StdoutOutput, TracerConfig, TracerOutputs,
};
use tracing_subscriber::Layer;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Initialize a global tracing subscriber that only captures `camel_tracer` spans.
/// Must be called once per process; subsequent calls are silently ignored.
fn init_tracer_subscriber() {
    let layer = tracing_subscriber::fmt::layer()
        .json()
        .with_span_events(FmtSpan::CLOSE)
        .with_target(true)
        .with_filter(tracing_subscriber::filter::filter_fn(|meta| {
            meta.target() == "camel_tracer"
        }))
        .boxed();

    let _ = tracing_subscriber::registry().with(layer).try_init();
}

fn tracer_config(detail_level: DetailLevel) -> TracerConfig {
    TracerConfig {
        enabled: true,
        detail_level,
        outputs: TracerOutputs {
            stdout: StdoutOutput {
                enabled: true,
                format: OutputFormat::Json,
            },
            file: None,
        },
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Section 1: Minimal
// ---------------------------------------------------------------------------

async fn run_section_1() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Section 1: Minimal ===");
    println!(
        "Fields: correlation_id, route_id, step_id, step_index, timestamp, duration_ms, status"
    );
    println!("Route: timer → log  (3 exchanges)\n");

    let mut ctx = CamelContext::new();
    ctx.set_tracer_config(tracer_config(DetailLevel::Minimal));
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from("timer:tick?period=500&repeatCount=3")
        .route_id("minimal-route")
        .to("log:info?showBody=true")
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;
    tokio::time::sleep(tokio::time::Duration::from_millis(2500)).await;
    ctx.stop().await?;

    println!("\n--- Section 1 complete ---\n");
    Ok(())
}

// ---------------------------------------------------------------------------
// Section 2: Medium
// ---------------------------------------------------------------------------

async fn run_section_2() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Section 2: Medium ===");
    println!("Fields: + headers_count, body_type, has_error, output_body_type");
    println!("Route: timer → set_header → transform → log  (2 exchanges)\n");

    let mut ctx = CamelContext::new();
    ctx.set_tracer_config(tracer_config(DetailLevel::Medium));
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from("timer:tick?period=500&repeatCount=2")
        .route_id("medium-route")
        .set_header("source", Value::String("timer".into()))
        .map_body(|_| camel_api::Body::Text("hello from medium".into()))
        .to("log:info?showHeaders=true&showBody=true")
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;
    tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;
    ctx.stop().await?;

    println!("\n--- Section 2 complete ---\n");
    Ok(())
}

// ---------------------------------------------------------------------------
// Section 3: Full
// ---------------------------------------------------------------------------

async fn run_section_3() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Section 3: Full ===");
    println!("Fields: + header_0, header_1, header_2 (up to 3 headers captured)");
    println!("Route: timer → set_header×3 → log  (2 exchanges)\n");

    let mut ctx = CamelContext::new();
    ctx.set_tracer_config(tracer_config(DetailLevel::Full));
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from("timer:tick?period=500&repeatCount=2")
        .route_id("full-route")
        .set_header("env", Value::String("production".into()))
        .set_header("version", Value::String("1.0.0".into()))
        .set_header("region", Value::String("us-east-1".into()))
        .to("log:info?showHeaders=true&showBody=true")
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;
    tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;
    ctx.stop().await?;

    println!("\n--- Section 3 complete ---\n");
    Ok(())
}

// ---------------------------------------------------------------------------
// Section 4: Error
// ---------------------------------------------------------------------------

async fn run_section_4() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Section 4: Error ===");
    println!("Fields: status=\"error\", error=<message>, error_type=<type>");
    println!("Route: timer → processor(returns Err)  (2 exchanges)\n");

    let mut ctx = CamelContext::new();
    ctx.set_tracer_config(tracer_config(DetailLevel::Minimal));
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from("timer:tick?period=500&repeatCount=2")
        .route_id("error-route")
        .process(|_exchange| async move {
            Err(CamelError::ProcessorError(
                "intentional failure to show error fields".into(),
            ))
        })
        .to("log:info?showBody=true")
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;
    tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;
    ctx.stop().await?;

    println!("\n--- Section 4 complete ---\n");
    Ok(())
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize once; all 4 sections share this subscriber.
    init_tracer_subscriber();

    println!("╔══════════════════════════════════════════════╗");
    println!("║          rust-camel  Tracer EIP Demo          ║");
    println!("╚══════════════════════════════════════════════╝");
    println!();
    println!("Each route step emits a JSON span to stdout.");
    println!("Sections run sequentially and exit automatically.");
    println!();

    run_section_1().await?;
    run_section_2().await?;
    run_section_3().await?;
    run_section_4().await?;

    println!("All sections complete.");
    Ok(())
}
