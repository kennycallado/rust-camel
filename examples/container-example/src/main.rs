//! Demonstrates container management features:
//!   - Route 1: timer → list containers → log
//!   - Route 2: timer → run alpine container → log
//!   - Route 3: container events consumer → log
//!   - Route 4: exec demo — run container, exec command, log output
//!   - Route 5: network demo — create/list/remove Docker network
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
use camel_component_container::{
    ContainerComponent, HEADER_CONTAINER_NAME, cleanup_tracked_containers,
};
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

    let mut ctx = CamelContext::builder().build().await.unwrap();
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

    // Route 4: Exec demo — run container, exec command, log output
    let exec_demo_route = RouteBuilder::from("timer:exec-demo?period=30000")
        .route_id("container-exec-demo")
        .process(|mut exchange| async move {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis();
            let name = format!("exec-demo-{}", timestamp);
            exchange
                .input
                .set_header(HEADER_CONTAINER_NAME, Value::String(name));
            Ok(exchange)
        })
        .to("container:run?image=alpine:latest&cmd=sleep 30&autoRemove=true")
        .process(|mut exchange| async move {
            let container_id = exchange
                .input
                .header("CamelContainerId")
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            exchange.input.set_header(
                "CamelContainerAction",
                Value::String("exec".to_string()),
            );
            exchange.input.set_header(
                "CamelContainerId",
                Value::String(container_id),
            );
            exchange.input.set_header(
                "CamelContainerCmd",
                Value::String("echo 'Hello from exec!' && uname -a".to_string()),
            );
            Ok(exchange)
        })
        .to("container:exec")
        .to("log:info?showHeaders=true&showBody=true")
        .build()?;

    // Route 5: Network demo — create network, list, remove
    let network_demo_route = RouteBuilder::from("timer:net-demo?period=45000")
        .route_id("container-network-demo")
        .process(|mut exchange| async move {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis();
            let name = format!("camel-net-{}", timestamp);
            exchange
                .input
                .set_header("CamelContainerAction", Value::String("network-create".to_string()));
            exchange
                .input
                .set_header("CamelContainerName", Value::String(name));
            Ok(exchange)
        })
        .to("container:network-create")
        .process(|mut exchange| async move {
            let network_id = exchange
                .input
                .header("CamelContainerNetwork")
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            println!("Created network: {}", network_id);
            exchange.input.set_header(
                "CamelContainerAction",
                Value::String("network-remove".to_string()),
            );
            Ok(exchange)
        })
        .to("container:network-remove")
        .to("log:info?showHeaders=true")
        .build()?;

    ctx.add_route_definition(list_route).await?;
    ctx.add_route_definition(lifecycle_route).await?;
    ctx.add_route_definition(events_route).await?;
    ctx.add_route_definition(exec_demo_route).await?;
    ctx.add_route_definition(network_demo_route).await?;

    println!();
    println!("Container example started!");
    println!("  - Route 'container-list': Lists containers every 10s");
    println!("  - Route 'container-lifecycle': Runs alpine container every 20s");
    println!("    - autoPull=true (default): pulls image if needed");
    println!("    - cmd='sleep 15': keeps container alive");
    println!("    - autoRemove=true (default): auto-removes on exit");
    println!("  - Route 'container-events': Streams formatted Docker events");
    println!("  - Route 'container-exec-demo': Runs container & exec command every 30s");
    println!("  - Route 'container-network-demo': Creates/removes network every 45s");
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
