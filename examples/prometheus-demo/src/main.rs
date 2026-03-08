use camel_api::CamelError;
use camel_api::body::Body;
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
        .with_env_filter("info,camel_prometheus=debug")
        .init();

    info!("Starting Prometheus metrics demo with Lifecycle integration");

    let prometheus = PrometheusService::new(9090);

    let mut ctx = CamelContext::new()
        .with_lifecycle(prometheus)
        .with_tracing();

    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from("timer:metrics?period=1000&repeatCount=30")
        .route_id("prometheus-demo")
        .process(|mut exchange| {
            let start = std::time::Instant::now();
            async move {
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

                exchange.input.body = Body::Json(serde_json::json!({
                    "message": "processed",
                    "elapsed_us": start.elapsed().as_micros(),
                }));
                Ok(exchange)
            }
        })
        .to("log:output?showBody=true")
        .build()?;

    ctx.add_route_definition(route)?;

    info!("Routes configured. Prometheus server will start automatically on http://0.0.0.0:9090");
    info!("Metrics available at http://0.0.0.0:9090/metrics");

    ctx.start().await?;

    info!("Routes started. Press Ctrl+C to stop.");
    tokio::signal::ctrl_c().await.ok();

    ctx.stop().await?;

    info!("Shutdown complete");
    Ok(())
}
