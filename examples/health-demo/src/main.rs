use camel_api::CamelError;
use camel_builder::RouteBuilder;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_config::{CamelConfig, HealthCamelConfig, ObservabilityConfig};
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter("info")
        .init();

    info!("Starting health demo with CamelConfig-managed HealthServer");

    let health_port = 8080;
    let config = CamelConfig {
        routes: vec![],
        watch: false,
        runtime_journal: None,
        log_level: "INFO".to_string(),
        timeout_ms: 5000,
        drain_timeout_ms: 10_000,
        watch_debounce_ms: 300,
        components: Default::default(),
        observability: ObservabilityConfig {
            health: Some(HealthCamelConfig {
                enabled: true,
                host: "0.0.0.0".to_string(),
                port: health_port,
            }),
            ..Default::default()
        },
        supervision: None,
    };

    let mut ctx = CamelConfig::configure_context(&config).await?;

    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from("timer:health?period=5000")
        .route_id("health-demo-route")
        .to("log:health?showBody=false")
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    info!("Health endpoints:");
    info!("  - Liveness:  http://0.0.0.0:{}/healthz", health_port);
    info!("  - Readiness: http://0.0.0.0:{}/readyz", health_port);
    info!("  - Health:    http://0.0.0.0:{}/health", health_port);
    info!("Press Ctrl+C to stop");

    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;
    info!("Shutdown complete");
    Ok(())
}
