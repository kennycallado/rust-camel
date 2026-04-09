use camel_api::CamelError;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_config::CamelConfig;
use camel_core::CamelContext;
use std::env;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_max_level(tracing::Level::INFO)
        .init();

    // Get profile from environment or use default
    let profile = env::var("CAMEL_PROFILE").unwrap_or_else(|_| "development".to_string());
    println!("Loading configuration with profile: {}", profile);

    // Load configuration with profile
    let config = CamelConfig::from_file_with_profile("Camel.toml", Some(&profile))
        .map_err(|e| CamelError::Config(e.to_string()))?;

    println!("Log level: {}", config.log_level);

    // Create context
    let mut ctx = CamelContext::builder().build().await.unwrap();

    // Register components
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // Load routes
    let routes = camel_config::discover_routes(&config.routes)
        .map_err(|e| CamelError::Config(e.to_string()))?;

    let route_count = routes.len();
    for route in routes {
        ctx.add_route_definition(route).await?;
    }

    println!("Starting context with {} routes...", route_count);

    ctx.start().await?;

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("Shutting down...");
    ctx.stop().await?;
    Ok(())
}
