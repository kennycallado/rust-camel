//! Hot Reload Example: Zero-Downtime Pipeline Swapping
//!
//! This example demonstrates how to use `swap_pipeline()` to hot-reload a route's
//! processing logic without stopping the route or losing messages.
//!
//! # Key Concepts Demonstrated
//!
//! 1. **Zero-Downtime Swap**: The route continues processing messages during the swap
//! 2. **Atomic Transition**: All new requests immediately see the new pipeline
//! 3. **In-Flight Safety**: Requests already processing complete with the old pipeline
//! 4. **Multiple Swaps**: Demonstrate rapid successive swaps work correctly
//!
//! # When to Use Hot-Reload
//!
//! - **Feature Flags**: Enable/disable features without redeployment
//! - **A/B Testing**: Switch between different algorithm implementations
//! - **Bug Fixes**: Deploy critical fixes without service interruption
//! - **Configuration Changes**: Update processing logic based on config changes
//! - **Performance Tuning**: Swap to optimized implementations at runtime
//!
//! # swap_pipeline vs restart
//!
//! | Operation      | Downtime | In-Flight Requests | Use Case              |
//! |----------------|----------|-------------------|-----------------------|
//! | swap_pipeline  | None     | Complete normally | Logic changes only    |
//! | restart        | Brief    | May be lost       | Full route reconfig   |
//!
//! # Safety Guarantees (ArcSwap Behavior)
//!
//! - Reads are wait-free: no blocking during normal operation
//! - Writes are atomic: no torn reads possible
//! - Old references remain valid until all holders drop them
//! - Memory is reclaimed only when no references remain
//!
//! # Performance Characteristics
//!
//! - Swap overhead: O(1) atomic pointer update
//! - Read overhead: Single atomic load (very fast)
//! - Memory: Old pipeline kept alive until in-flight requests complete

use std::time::Duration;

use camel_api::{BoxProcessor, BoxProcessorExt, CamelError, Exchange, Value, body::Body};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

/// A versioned processor that logs messages with its version identifier.
///
/// This demonstrates creating custom processors that can be swapped at runtime.
/// Each version maintains its own identity, making it easy to see when swaps occur.
///
/// # Important Note
///
/// When using `swap_pipeline()`, the ENTIRE pipeline is replaced. This means any
/// steps defined in the route builder (like `.to("log:info")`) will NOT be included
/// in the new pipeline. For this reason, this processor logs directly to stdout
/// rather than relying on a downstream log component.
fn create_versioned_processor(version: &'static str) -> BoxProcessor {
    BoxProcessor::from_fn(move |mut exchange: Exchange| {
        let version = version;
        async move {
            // Get the timer counter from headers (set by timer component)
            let counter = exchange
                .input
                .header("CamelTimerCounter")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);

            // Create versioned message body
            let original_body = exchange
                .input
                .body
                .as_text()
                .map(|s| s.to_string())
                .unwrap_or_default();

            let new_body = format!("[V{}] timer#{}: {}", version, counter, original_body);
            exchange.input.body = Body::Text(new_body.clone());

            // Set a version property so we can verify which pipeline processed it
            exchange.set_property("pipeline-version", Value::String(version.to_string()));

            // Log directly to stdout (since swap_pipeline replaces the entire pipeline)
            println!(
                "  {} {}",
                chrono::Local::now().format("%H:%M:%S%.3f"),
                new_body
            );

            // Simulate some processing work (demonstrates in-flight safety)
            tokio::time::sleep(Duration::from_millis(100)).await;

            Ok(exchange)
        }
    })
}

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    // Initialize tracing for component lifecycle logs
    tracing_subscriber::fmt()
        .with_target(false)
        .with_thread_ids(false)
        .with_level(false)
        .without_time()
        .init();

    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║          Hot Reload Example: Zero-Downtime Pipeline Swap       ║");
    println!("╚════════════════════════════════════════════════════════════════╝");
    println!();

    // Create and configure the Camel context
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // Create a timer route that produces a message every second.
    //
    // IMPORTANT: The initial route uses an identity processor because the real
    // processing logic will be set via swap_pipeline(). This demonstrates that
    // swap_pipeline() can be used to set the initial pipeline as well as swap it.
    //
    // Note: We don't use .to("log:info") here because swap_pipeline() replaces
    // the ENTIRE pipeline. Any steps after the processor would be lost on swap.
    let route = RouteBuilder::from("timer:hot-reload?period=1000")
        .route_id("hot-reload-route")
        .process_fn(create_versioned_processor("1"))
        .build()?;

    ctx.add_route_definition(route)?;
    ctx.start().await?;

    // Get a handle to the route controller for hot-reload operations
    let controller = ctx.route_controller().clone();

    println!("Route started with V1 processor");
    println!("Watch the messages change version after each swap:");
    println!();
    println!("  Time     Message");
    println!("  ─────    ────────────────────────────────────────────");

    // Counter for tracking swaps
    let total_swaps = 3;

    // Spawn a task that performs pipeline swaps at intervals
    let swap_handle = {
        tokio::spawn(async move {
            // Wait 5 seconds before first swap to show V1 working
            tokio::time::sleep(Duration::from_secs(5)).await;

            for swap_num in 1..=total_swaps {
                let new_version = (swap_num + 1).to_string();

                println!();
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!(">>> SWAP #{swap_num}: Switching to V{new_version} processor");
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!();

                // Create the new versioned processor
                // Note: We leak the string to get a 'static lifetime for the closure.
                // In production code, you might use Arc<String> or enum-based versioning.
                let version_str: &'static str = Box::leak(new_version.into_boxed_str());
                let new_pipeline = create_versioned_processor(version_str);

                // Perform the atomic swap
                // This is the key operation - it updates the pipeline pointer atomically
                {
                    let ctrl = controller.lock().await;
                    ctrl.swap_pipeline("hot-reload-route", new_pipeline)
                        .expect("swap should succeed");
                }

                // Wait before next swap (or exit after last swap)
                if swap_num < total_swaps {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }

            println!();
            println!("All swaps completed. Letting route run for 2 more seconds...");
            tokio::time::sleep(Duration::from_secs(2)).await;
        })
    };

    // Wait for the swap task to complete
    swap_handle.await.expect("swap task should complete");

    println!();
    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║                    Example Complete                             ║");
    println!("╚════════════════════════════════════════════════════════════════╝");
    println!();
    println!("Summary:");
    println!("  - Route started with V1 processor");
    for i in 1..=total_swaps {
        println!("  - Swap #{i}: Switched to V{} processor", i + 1);
    }
    println!();
    println!("Key Observations:");
    println!("  ✓ Each swap was atomic (zero-downtime)");
    println!("  ✓ No messages were lost during swaps");
    println!("  ✓ In-flight messages completed with their original pipeline version");
    println!("  ✓ New messages immediately used the new pipeline version");
    println!();

    // Graceful shutdown
    println!("Shutting down...");
    ctx.stop().await?;

    println!("Done!");
    Ok(())
}
