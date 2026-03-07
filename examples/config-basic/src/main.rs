use camel_api::CamelError;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_config::{CamelConfig, discover_routes};
use camel_core::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_max_level(tracing::Level::INFO)
        .init();

    // Load configuration from Camel.toml
    let config =
        CamelConfig::from_file("Camel.toml").map_err(|e| CamelError::Config(e.to_string()))?;

    // Create context
    let mut ctx = CamelContext::new();

    // Register components
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // Discover and load routes from configuration
    let routes = discover_routes(&config.routes).map_err(|e| CamelError::Config(e.to_string()))?;

    for route in routes {
        ctx.add_route_definition(route)?;
    }

    let route_count = ctx.route_controller().lock().await.route_count();
    println!(
        "Starting context with {} routes from config...",
        route_count
    );

    ctx.start().await?;

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("Shutting down...");
    ctx.stop().await?;
    Ok(())
}
