//! Lazy Route Example
//!
//! Demonstrates a route with `auto_startup(false)` that doesn't start automatically.
//! The route is manually started after a delay using the RouteController API.

use camel_api::{CamelError, RouteController, RouteStatus};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_core::context::CamelContext;
use camel_log::LogComponent;
use camel_timer::TimerComponent;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt::init();

    let mut ctx = CamelContext::new();

    // Register required components
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // Create a "lazy" route that doesn't auto-start
    let lazy_route = RouteBuilder::from("timer:lazy?period=2000")
        .route_id("lazy-route")
        .auto_startup(false)
        .to("log:info")
        .build()?;

    ctx.add_route_definition(lazy_route)?;

    // Start the context - only auto_startup routes will start
    ctx.start().await?;

    // Check status: lazy route should be Stopped
    let status = ctx
        .route_controller()
        .lock()
        .await
        .route_status("lazy-route");
    println!("Route status before manual start: {:?}", status);
    assert_eq!(status, Some(RouteStatus::Stopped));

    // Wait 3 seconds before manually starting the lazy route
    println!("Waiting 3 seconds before manually starting the lazy route...");
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    // Manually start the lazy route
    println!("Manually starting the lazy route...");
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
    assert_eq!(status, Some(RouteStatus::Started));

    // Let the route run for 3 more seconds
    println!("Route is now running. Waiting 3 more seconds...");
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    // Stop the context cleanly
    println!("Stopping context...");
    ctx.stop().await?;

    println!("Done!");
    Ok(())
}
