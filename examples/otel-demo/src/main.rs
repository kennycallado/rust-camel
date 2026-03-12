//! OpenTelemetry demo for rust-camel.
//!
//! Demonstrates integration of OpenTelemetry tracing and metrics with rust-camel.
//!
//! # Requirements
//!
//! Requires `grafana/otel-lgtm` running locally:
//! ```bash
//! docker run -p 3000:3000 -p 4317:4317 -p 4318:4318 grafana/otel-lgtm
//! ```
//!
//! # Running
//!
//! ```bash
//! cargo run -p otel-demo
//! ```
//!
//! Then open http://localhost:3000 to view traces, logs, and metrics in Grafana.
//!
//! # What it does
//!
//! - Creates a timer that fires every 5 seconds
//! - Logs each tick
//! - Makes an HTTP GET request to httpbin.org/get
//! - All exchanges are traced and metrics are exported via OTLP

use camel_api::Value;
use camel_api::body::Body;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_http::{HttpComponent, HttpsComponent};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_core::{DetailLevel, StdoutOutput, TracerConfig, TracerOutputs};
use camel_otel::{OtelConfig, OtelService};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create OpenTelemetry configuration
    // Points to the OTLP collector (grafana/otel-lgtm) running locally
    // OtelService::start() will initialize the tracing subscriber with
    // fmt layer (console) + OTel log bridge (OTLP export) automatically.
    let otel_config = OtelConfig::new("http://localhost:4317", "rust-camel-otel-demo");

    // Create the OpenTelemetry service
    // This will initialize global TracerProvider and MeterProvider
    let otel_service = OtelService::new(otel_config);

    // Build CamelContext with the OtelService as a lifecycle service
    // The OtelService will be started before routes and stopped after routes
    let mut ctx = CamelContext::new()
        .with_lifecycle(otel_service)
        .with_tracer_config(TracerConfig {
            enabled: true,
            detail_level: DetailLevel::Medium,
            outputs: TracerOutputs {
                stdout: StdoutOutput {
                    enabled: false, // suppress noisy JSON to stdout
                    ..Default::default()
                },
                file: None,
            },
        });

    // Register required components
    ctx.register_component(TimerComponent::new());
    ctx.register_component(HttpComponent::new());
    ctx.register_component(HttpsComponent::new());
    ctx.register_component(LogComponent::new());

    // Define the route:
    // timer:tick (every 5s) -> log:info -> HTTP GET httpbin.org/get
    let route = RouteBuilder::from("timer:tick?period=5000")
        .route_id("otel-demo-route")
        .to("log:info?showBody=true&showHeaders=true")
        .process(|mut exchange| {
            Box::pin(async move {
                // Clear body for GET request
                exchange.input.body = Body::Empty;
                // Add a request ID header
                exchange
                    .input
                    .set_header("X-Demo-Source", Value::String("rust-camel-otel".into()));
                Ok(exchange)
            })
        })
        .to("https://httpbin.org/get?allowPrivateIps=false")
        .process(|exchange| {
            Box::pin(async move {
                // Log the response
                if let Some(status) = exchange.input.header("CamelHttpResponseCode") {
                    tracing::info!("HTTP response status: {:?}", status);
                }
                Ok(exchange)
            })
        })
        .to("log:response?showBody=true")
        .build()?;

    ctx.add_route_definition(route)?;

    tracing::info!("Starting CamelContext with OpenTelemetry integration");
    ctx.start().await?;

    tracing::info!("Demo running! Timer firing every 5 seconds.");
    tracing::info!("Making HTTP requests to https://httpbin.org/get");
    tracing::info!("OpenTelemetry data being exported to localhost:4317");
    tracing::info!("");
    tracing::info!("Press Ctrl+C to stop, or wait 30 seconds for auto-shutdown.");

    // Run for 30 seconds then stop
    tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;

    tracing::info!("Shutting down...");
    ctx.stop().await?;

    tracing::info!("OpenTelemetry demo complete.");
    tracing::info!("Check http://localhost:3000 for traces and metrics in Grafana.");

    Ok(())
}
