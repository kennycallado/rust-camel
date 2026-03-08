use std::sync::Arc;
use std::time::Duration;

use camel_api::CamelError;
use camel_api::body::Body;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_prometheus::{PrometheusMetrics, MetricsServer};
use tokio::signal;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    // Create Prometheus metrics collector
    let metrics = Arc::new(PrometheusMetrics::new());
    let metrics_for_server = Arc::clone(&metrics);

    // Create context with Prometheus metrics collector
    let mut ctx = CamelContext::with_metrics(metrics);

    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // Create a route that processes exchanges every second
    let route = RouteBuilder::from("timer:prometheus?period=1000")
        .route_id("prometheus-demo")
        .process(|mut exchange| {
            async move {
                // Simulate some work with variable processing time
                let processing_time_ms = 10 + (rand::random::<u64>() % 90);
                tokio::time::sleep(Duration::from_millis(processing_time_ms)).await;

                exchange.input.body = Body::Json(serde_json::json!({
                    "message": "processed",
                    "processing_time_ms": processing_time_ms,
                }));
                Ok(exchange)
            }
        })
        .to("log:prometheus?showBody=true&showCorrelationId=true")
        .build()?;

    ctx.add_route_definition(route)?;

    // Parse server address at top level
    let addr = "127.0.0.1:9090".parse().expect("Invalid address");
    
    // Start the metrics server in the background
    let metrics_server_task = tokio::spawn(async move {
        MetricsServer::run(addr, metrics_for_server).await;
    });

    // Start the Camel context
    ctx.start().await?;

    println!("Prometheus demo running.");
    println!("Metrics server listening on http://localhost:9090/metrics");
    println!("Processing exchanges every second...");
    println!("Press Ctrl+C to stop gracefully.");

    // Wait for Ctrl+C
    signal::ctrl_c().await?;

    println!("Shutting down gracefully...");

    // Stop the Camel context
    ctx.stop().await?;

    // Cancel the metrics server task
    metrics_server_task.abort();

    println!("Done.");

    Ok(())
}