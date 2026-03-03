//! Lazy Route Example
//!
//! Demonstrates a route with `auto_startup(false)` that doesn't start automatically.
//! The route is manually started after a delay using the RouteController API.

use camel_api::{CamelError, RouteController};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::new();

    // Register required components
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // Create a "lazy" route that doesn't auto-start
    let lazy_route = RouteBuilder::from("timer:lazy?period=1000")
        .route_id("lazy-route")
        .auto_startup(false)
        .to("log:info?showHeaders=true&showCorrelationId=true")
        .build()?;

    ctx.add_route_definition(lazy_route)?;

    // Start the context - only auto_startup routes will start
    ctx.start().await?;

    println!("Lazy route example running.");
    println!("Route 'lazy-route' is NOT started automatically.");
    println!("It will be started manually in 3 seconds...");

    // Wait 3 seconds before manually starting the lazy route
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    // Check status: lazy route should be Stopped
    let status = ctx
        .route_controller()
        .lock()
        .await
        .route_status("lazy-route");
    println!("Route status before manual start: {:?}", status);

    // Manually start the lazy route
    println!("Starting route 'lazy-route' now...");
    ctx.route_controller()
        .lock()
        .await
        .start_route("lazy-route")
        .await?;

    // Check status: should now be Started
    let status = ctx
        .route_controller()
        .lock()
        .await
        .route_status("lazy-route");
    println!("Route status after manual start: {:?}", status);

    println!("Route started! Press Ctrl+C to stop.");

    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;
    Ok(())
}
