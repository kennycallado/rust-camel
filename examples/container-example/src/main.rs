//! Container component example for rust-camel.
//!
//! Demonstrates "batteries included" container management:
//!   - Route 1: timer → list containers → log
//!   - Route 2: timer → run container (auto-pull, auto-remove) → log
//!   - Route 3: container events consumer → log
//!
//! Requires a running Docker daemon. Just run:
//!
//!   cargo run -p container-example
//!
//! The example will automatically pull alpine:latest if needed (autoPull=true).
//! Press Ctrl+C to stop. Tracked containers are cleaned up on shutdown.

use std::time::{SystemTime, UNIX_EPOCH};

use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_container::{cleanup_tracked_containers, ContainerComponent, HEADER_CONTAINER_NAME};
use camel_component_direct::DirectComponent;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(DirectComponent::new());
    ctx.register_component(ContainerComponent::new());

    // Route 1: List containers every 10 seconds
    let list_route = RouteBuilder::from("timer:list?period=10000")
        .route_id("container-list")
        .to("container:list")
        .to("log:info?showBody=true")
        .build()?;

    // Route 2: Run a container with simplified API
    // - autoPull=true (default): pulls alpine if not present
    // - cmd=sleep 15: keeps container alive for 15s
    // - autoRemove=true (default): auto-removes container when it exits
    let lifecycle_route = RouteBuilder::from("timer:lifecycle?period=20000")
        .route_id("container-lifecycle")
        .process(|mut exchange| async move {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis();
            let name = format!("example-{}", timestamp);
            exchange
                .input
                .set_header(HEADER_CONTAINER_NAME, Value::String(name));
            Ok(exchange)
        })
        .to("log:info?message=🚀%20Starting%20container...")
        .to("container:run?image=alpine:latest&cmd=sleep 15")
        .to("log:info?showHeaders=true")
        .build()?;

    // Route 3: Subscribe to Docker events (formatted for readability)
    let events_route = RouteBuilder::from("container:events")
        .route_id("container-events")
        .to("log:info?showBody=true")
        .build()?;

    ctx.add_route_definition(list_route)?;
    ctx.add_route_definition(lifecycle_route)?;
    ctx.add_route_definition(events_route)?;

    println!();
    println!("Container example started!");
    println!("  - Route 'container-list': Lists containers every 10s");
    println!("  - Route 'container-lifecycle': Runs alpine container every 20s");
    println!("    - autoPull=true (default): pulls image if needed");
    println!("    - cmd='sleep 15': keeps container alive");
    println!("    - autoRemove=true (default): auto-removes on exit");
    println!("  - Route 'container-events': Streams formatted Docker events");
    println!();
    println!("Containers are tracked and cleaned up on shutdown.");
    println!("Watch the logs to see the lifecycle in action.");
    println!("Press Ctrl+C to stop.");
    println!();

    ctx.start().await?;

    tokio::signal::ctrl_c().await.ok();
    println!("\nStopping...");
    ctx.stop().await?;
    
    // Cleanup any tracked containers that are still running
    cleanup_tracked_containers().await;
    
    Ok(())
}
