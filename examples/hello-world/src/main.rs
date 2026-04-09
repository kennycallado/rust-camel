use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_target(false) // Cleaner output
        .init();

    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from("timer:tick?period=1000&repeatCount=5")
        .route_id("hello-world") // Named routes
        .set_header("source", Value::String("timer".into()))
        .to("log:info?showHeaders=true&showCorrelationId=true") // Correlation ID
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("Hello World example running. Press Ctrl+C to stop.");

    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;
    Ok(())
}
