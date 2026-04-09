use camel_api::CamelError;
use camel_api::Value;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let fixed_route = RouteBuilder::from("timer:tick?period=2000&repeatCount=5")
        .route_id("delayer-fixed")
        .delay(Duration::from_millis(500))
        .to("log:delayed?showBody=true")
        .build()?;

    let dynamic_route = RouteBuilder::from("timer:tick?period=2000&repeatCount=5")
        .route_id("delayer-dynamic")
        .set_header("CamelDelayMs", Value::Number(1000.into()))
        .delay_with_header(Duration::from_secs(1), "CamelDelayMs")
        .to("log:dynamic-delayed?showBody=true")
        .build()?;

    ctx.add_route_definition(fixed_route).await?;
    ctx.add_route_definition(dynamic_route).await?;
    ctx.start().await?;

    println!("Delayer example running.");
    println!("Route 1 (fixed): timer -> delay(500ms) -> log:delayed");
    println!(
        "Route 2 (dynamic): timer -> set_header(CamelDelayMs=1000) -> delay_with_header -> log:dynamic-delayed"
    );
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down delayer example...");
    ctx.stop().await?;
    println!("Delayer example stopped cleanly.");

    Ok(())
}
