use camel_api::body::Body;
use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_seda::SedaComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap

    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(SedaComponent::new());

    // Route A: Timer -> set header -> send to seda:processing
    let route_a = RouteBuilder::from("timer:tick?period=1000&repeatCount=5")
        .route_id("producer-route")
        .set_header("source", Value::String("timer".into()))
        .to("seda:processing")
        .build()?;

    // Route B: Receive from seda:processing (2 concurrent consumers) -> uppercase body -> log
    let route_b = RouteBuilder::from("seda:processing?concurrentConsumers=2")
        .route_id("consumer-route")
        .map_body(|body: Body| {
            if let Some(text) = body.as_text() {
                Body::Text(text.to_uppercase())
            } else {
                body
            }
        })
        .to("log:output?showBody=true&showHeaders=true&showCorrelationId=true")
        .build()?;

    ctx.add_route_definition(route_a).await?;
    ctx.add_route_definition(route_b).await?;
    ctx.start().await?;

    println!("SEDA demo running.");
    println!("Route A (timer -> seda:processing) -> Route B (uppercase -> log)");
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down SEDA demo...");
    ctx.stop().await?;
    println!("SEDA demo stopped cleanly.");

    Ok(())
}
