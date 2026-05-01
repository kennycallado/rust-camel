//! OpenTelemetry demo for rust-camel.
//!
//! Demonstrates integration of OpenTelemetry tracing, metrics, and logs with rust-camel.
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
//! - Logs each tick (logs exported via OTLP log bridge)
//! - Makes an HTTP GET request to httpbin.org/get
//! - All exchanges are traced and route-level metrics are recorded automatically
//!
//! # OTel Features Demonstrated
//!
//! - **Traces**: Distributed tracing via OTLP span exporter
//! - **Metrics**: Route-level metrics (duration, exchanges, errors) auto-recorded
//! - **Logs**: Log bridge exports `tracing` logs via OTLP
//! - **Auto-registration**: `OtelService.as_metrics_collector()` enables metrics

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
    // metrics_interval_ms=15000 exports metrics every 15s (default 60s is too slow for demos)
    let otel_config = OtelConfig::new("http://localhost:4317", "rust-camel-otel-demo")
        .with_metrics_interval_ms(15000);

    // Create the OpenTelemetry service
    // OtelService manages:
    // - Global TracerProvider (spans exported via OTLP)
    // - Global MeterProvider (metrics exported via OTLP)
    // - Global LoggerProvider + Log bridge (logs exported via OTLP)
    // - as_metrics_collector() for automatic route metrics
    let mut otel_service = OtelService::new(otel_config);

    // Initialize tracing subscriber with both stdout and OTel log bridge
    let logger_provider = otel_service.init_logger_provider()?;
    let otel_layer =
        opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(&logger_provider);

    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(otel_layer)
        .init();

    // Build CamelContext with the OtelService as a lifecycle service.
    // with_lifecycle() auto-registers the metrics collector from OtelService.
    let mut ctx = CamelContext::builder()
        .build()
        .await
        .unwrap()
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
            ..Default::default()
        })
        .await;

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
                // Log the response - this log is exported via OTLP log bridge
                if let Some(status) = exchange.input.header("CamelHttpResponseCode") {
                    tracing::info!("HTTP response status: {:?}", status);
                }
                Ok(exchange)
            })
        })
        .to("log:response?showBody=true")
        .build()?;

    ctx.add_route_definition(route).await?;

    println!("Starting CamelContext with OpenTelemetry integration");
    ctx.start().await?;

    println!("Demo running! Timer firing every 5 seconds.");
    println!("Making HTTP requests to https://httpbin.org/get");
    println!("Traces, metrics, and logs exported to localhost:4317");
    println!();
    println!("Press Ctrl+C to stop, or wait 30 seconds for auto-shutdown.");

    // Run for 30 seconds then stop
    tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;

    println!("Shutting down...");
    ctx.stop().await?;

    println!("OpenTelemetry demo complete.");
    println!("Check http://localhost:3000 for traces, metrics, and logs in Grafana.");

    Ok(())
}
