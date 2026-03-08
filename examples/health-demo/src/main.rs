use camel_api::CamelError;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_prometheus::PrometheusService;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter("info")
        .init();

    info!("=== Health Monitoring Demo ===");
    info!(
        "This example demonstrates the health monitoring system with Kubernetes-ready endpoints\n"
    );

    // Create Prometheus service with dynamic port allocation
    let prometheus = PrometheusService::new(9090);

    // Create CamelContext with the prometheus service
    let mut ctx = CamelContext::new().with_lifecycle(prometheus);

    // Register components
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // Add a simple route
    let route = RouteBuilder::from("timer:health?period=5000")
        .route_id("health-check-route")
        .to("log:health?showBody=false")
        .build()?;

    ctx.add_route_definition(route)?;

    info!("Starting CamelContext with health monitoring...\n");

    // Start the context
    ctx.start().await?;

    // Get the port accessor before moving ctx
    let port = {
        // Access the prometheus service port
        // Note: In a real scenario, you'd store the port accessor before adding to context
        // For this demo, we'll use the known port
        9090
    };

    info!("Health Monitoring Endpoints:");
    info!("  Liveness:  http://0.0.0.0:{}/healthz", port);
    info!("  Readiness: http://0.0.0.0:{}/readyz", port);
    info!("  Health:    http://0.0.0.0:{}/health", port);
    info!("  Metrics:   http://0.0.0.0:{}/metrics\n", port);

    // Demonstrate health_check() API
    info!("=== Health Check API Demo ===");

    let report = ctx.health_check();
    info!("System Status: {:?}", report.status);
    info!("Timestamp: {}", report.timestamp.to_rfc3339());

    if report.services.is_empty() {
        info!("Services: (none - health checker not wired to context)");
        info!("Note: To wire health checker, inject it before adding service to context");
    } else {
        info!("Services:");
        for service in &report.services {
            info!("  - {}: {:?}", service.name, service.status);
        }
    }

    info!("\n=== Kubernetes Integration ===");
    info!("Use these probe configurations in your pod spec:");
    info!("");
    info!("livenessProbe:");
    info!("  httpGet:");
    info!("    path: /healthz");
    info!("    port: {}", port);
    info!("  initialDelaySeconds: 5");
    info!("  periodSeconds: 10");
    info!("");
    info!("readinessProbe:");
    info!("  httpGet:");
    info!("    path: /readyz");
    info!("    port: {}", port);
    info!("  initialDelaySeconds: 5");
    info!("  periodSeconds: 5");
    info!("");

    info!("=== Running ===");
    info!("Press Ctrl+C to stop");
    info!("Try: curl http://localhost:{}/health", port);

    // Wait for shutdown signal
    tokio::signal::ctrl_c().await.ok();

    info!("\nShutting down...");
    ctx.stop().await?;

    info!("Health monitoring demo stopped");
    Ok(())
}
