//! # Tracer EIP Demo
//!
//! Demonstrates automatic message flow tracing using the Tracer EIP.
//!
//! Each route step emits a structured JSON span containing:
//! - `correlation_id`: UUID for distributed tracing
//! - `route_id`: Route identifier
//! - `step_id`: Step identifier (e.g., "step-0")
//! - `duration_ms`: Processing duration in milliseconds
//! - `status`: "success" or "error"
//!
//! Run with:
//! ```bash
//! cargo run -p tracer-demo
//! ```

use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::{
    CamelContext, DetailLevel, OutputFormat, StdoutOutput, TracerConfig, TracerOutputs,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Tracer EIP Demo ===\n");
    println!("Each route step emits a JSON span to stdout.");
    println!("Fields: correlation_id, route_id, step_id, duration_ms, status\n");

    // Configure tracing with medium detail level (includes header count + body type)
    let tracer_config = TracerConfig {
        enabled: true,
        detail_level: DetailLevel::Medium,
        outputs: TracerOutputs {
            stdout: StdoutOutput {
                enabled: true,
                format: OutputFormat::Json,
            },
            file: None,
            opentelemetry: None,
        },
    };

    let mut ctx = CamelContext::new();
    ctx.set_tracer_config(tracer_config);

    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // Route: timer fires every second → set a header → log
    let route = RouteBuilder::from("timer:tick?period=1000&repeatCount=3")
        .route_id("demo-route")
        .set_header("source", camel_api::Value::String("timer".into()))
        .to("log:info?showHeaders=true")
        .build()?;

    ctx.add_route_definition(route)?;

    println!("Starting — will fire 3 times then stop...\n");
    ctx.start().await?;

    tokio::time::sleep(tokio::time::Duration::from_secs(4)).await;

    ctx.stop().await?;

    println!("\nDone.");
    Ok(())
}
