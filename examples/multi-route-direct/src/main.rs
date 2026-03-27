use camel_api::body::Body;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_direct::DirectComponent;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::new();

    let direct = DirectComponent::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(direct);

    // Route A: Timer -> set header -> send to direct:pipeline
    let route_a = RouteBuilder::from("timer:tick?period=1000&repeatCount=5")
        .route_id("route-a-timer")
        .set_header("source", Value::String("timer".into()))
        .to("direct:pipeline")
        .build()?;

    // Route B: Receive from direct:pipeline -> uppercase body -> log
    let route_b = RouteBuilder::from("direct:pipeline")
        .route_id("route-b-pipeline")
        .map_body(|body: Body| {
            if let Some(text) = body.as_text() {
                Body::Text(text.to_uppercase())
            } else {
                body
            }
        })
        .to("log:output?showHeaders=true&showBody=true&showCorrelationId=true")
        .error_handler(
            ErrorHandlerConfig::log_only()
                .on_exception(|_| true)
                .retry(1)
                .build(),
        )
        .build()?;

    ctx.add_route_definition(route_a).await?;
    ctx.add_route_definition(route_b).await?;
    ctx.start().await?;

    println!("Multi-route direct example running.");
    println!("Route A (timer -> direct:pipeline) -> Route B (uppercase -> log)");
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down multi-route direct example...");
    ctx.stop().await?;
    println!("Multi-route direct example stopped cleanly.");

    Ok(())
}
