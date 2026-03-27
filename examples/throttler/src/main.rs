use camel_api::CamelError;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use std::time::Duration;

/// Throttler EIP example — rate limit message processing.
///
/// A timer fires rapidly (every 100ms). The throttler limits processing
/// to 2 messages per second. With the default Delay strategy, excess
/// messages are queued and processed when capacity becomes available.
///
/// Flow:
///   timer (100ms period, 20 fires)
///       -> throttle (2 per second, Delay strategy)
///           -> log:throttled
///
/// Watch the logs to see the throttling effect: messages arrive every
/// 100ms but are logged at most 2 per second.
#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from("timer:tick?period=100&repeatCount=20")
        .route_id("throttler-demo")
        // Throttle: limit to 2 requests per second
        // Default strategy is Delay - queues messages until capacity available
        .throttle(2, Duration::from_secs(1))
        .to("log:throttled?showBody=true")
        .end_throttle()
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("Throttler example running.");
    println!("Timer fires every 100ms, throttler allows 2 per second.");
    println!("Watch the logs to see the throttling effect.");
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down throttler example...");
    ctx.stop().await?;
    println!("Throttler example stopped cleanly.");

    Ok(())
}
