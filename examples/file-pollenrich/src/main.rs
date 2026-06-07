//! Example: EIP-7 pollEnrich — read a config file mid-route.
//!
//! This example demonstrates using pollEnrich to read a JSON config file
//! from disk at runtime, triggered by a timer. The file content is merged
//! into the exchange body (original headers/properties preserved).
//!
//! See `routes.yaml` for an equivalent YAML declaration of the same route.

use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_file::FileComponent;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_target(false).init();

    let config_dir = std::env::temp_dir().join("rust-camel-pollenrich");
    std::fs::create_dir_all(&config_dir).ok();
    let config_path = config_dir.join("config.json");
    std::fs::write(
        &config_path,
        r#"{"service_url": "https://api.example.com", "timeout_ms": 5000}"#,
    )?;
    let config_dir_str = config_dir.to_str().unwrap(); // allow-unwrap

    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(FileComponent::new());
    ctx.register_component(LogComponent::new());

    // Route: timer → pollEnrich(file:config.json) → log enriched body
    let route = RouteBuilder::from("timer:tick?period=1000&repeatCount=3")
        .route_id("pollenrich-demo")
        .poll_enrich(
            format!("file:{config_dir_str}?fileName=config.json&noop=true"),
            5000,
        )
        .stream_cache_default()
        .to("log:enriched?showBody=true&showHeaders=true&showCorrelationId=true")
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("pollEnrich example running.");
    println!("  Config file: {}", config_path.display());
    println!();
    println!("Every 1 second, the timer fires, pollEnrich reads config.json,");
    println!("and the enriched body is logged. The example stops after 3 ticks.");
    println!();
    println!("Press Ctrl+C to stop early...");

    tokio::signal::ctrl_c().await?;
    println!("\nShutting down...");
    ctx.stop().await?;
    println!("Done.");

    Ok(())
}
