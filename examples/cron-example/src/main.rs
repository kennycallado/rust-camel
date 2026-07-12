use camel_api::CamelError;
use camel_api::Value;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_cron::CronComponent;
use camel_component_log::LogComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap
    ctx.register_component(CronComponent::new());
    ctx.register_component(LogComponent::new());

    // Fire every minute at second 0.
    // Use `+` as space separator in the cron expression (Apache Camel convention).
    let route = RouteBuilder::from("cron:tick?schedule=*+*+*+*+*")
        .route_id("cron-demo")
        .set_body("cron-fired")
        .set_header("source", Value::String("cron".into()))
        .to("log:cron-result?level=info&showBody=true&showHeaders=true")
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("Cron example running.");
    println!("Route: cron:tick (every minute) -> set_body -> log:cron-result");
    println!("Watch the logs — an exchange fires at the top of each minute.");
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down cron example...");
    ctx.stop().await?;
    println!("Cron example stopped cleanly.");

    Ok(())
}
