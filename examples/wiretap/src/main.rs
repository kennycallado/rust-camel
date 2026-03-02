use camel_api::CamelError;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_core::context::CamelContext;
use camel_component_log::LogComponent;
use camel_component_mock::MockComponent;
use camel_component_timer::TimerComponent;

/// WireTap EIP example — send a copy of exchanges to a monitoring endpoint.
///
/// A timer fires periodically. Each exchange is:
/// 1. Tapped (cloned) and sent to a monitoring/log endpoint (fire-and-forget)
/// 2. Processed normally down the main pipeline
///
/// The tap does not affect the main flow — errors in the tap are isolated.
///
/// Flow:
///   timer -> wireTap(log:monitor) -> log:main
#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::new();

    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(MockComponent::new());

    let route = RouteBuilder::from("timer:tick?period=1000&repeatCount=5")
        .route_id("wiretap-demo")
        // Tap: send a clone to monitoring (fire-and-forget)
        .wire_tap("log:monitor?showBody=true&showCorrelationId=true")
        // Main pipeline continues normally
        .to("log:main?showBody=true&showCorrelationId=true")
        .build()?;

    ctx.add_route_definition(route)?;
    ctx.start().await?;

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("Shutting down wiretap example...");
    ctx.stop().await?;
    println!("Wiretap example stopped cleanly.");

    Ok(())
}
