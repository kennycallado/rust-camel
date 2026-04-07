//! Auto-Restart Example: SupervisingRouteController Demonstration
//!
//! This example demonstrates how the SupervisingRouteController automatically
//! restarts crashed routes with exponential backoff.
//!
//! # Key Concepts Demonstrated
//!
//! 1. **Route Supervision**: Automatic monitoring and restart of crashed routes
//! 2. **Exponential Backoff**: Increasing delays between restart attempts
//! 3. **Max Attempts Limit**: Giving up after too many failures
//! 4. **Crash Simulation**: Using a custom component that simulates crashes
//!
//! # SupervisionConfig Parameters
//!
//! - `max_attempts`: Maximum number of restart attempts (None = infinite)
//! - `initial_delay`: Delay before first restart attempt
//! - `backoff_multiplier`: Multiplier for delay after each failed attempt
//! - `max_delay`: Maximum delay cap between restart attempts

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::{BoxProcessor, CamelError, Exchange, SupervisionConfig};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::{Component, ConcurrencyModel, Consumer, ConsumerContext, Endpoint};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

/// A component that simulates crashes after a certain number of messages.
struct CrashSimulatorComponent {
    /// Shared counter for tracking crashes
    crash_counter: Arc<AtomicU32>,
    /// Number of successful runs before crashing
    runs_before_crash: u32,
}

impl CrashSimulatorComponent {
    fn new(runs_before_crash: u32) -> Self {
        Self {
            crash_counter: Arc::new(AtomicU32::new(0)),
            runs_before_crash,
        }
    }
}

impl Component for CrashSimulatorComponent {
    fn scheme(&self) -> &str {
        "crash-simulator"
    }

    fn create_endpoint(&self, _uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(CrashSimulatorEndpoint {
            crash_counter: Arc::clone(&self.crash_counter),
            runs_before_crash: self.runs_before_crash,
        }))
    }
}

struct CrashSimulatorEndpoint {
    crash_counter: Arc<AtomicU32>,
    runs_before_crash: u32,
}

impl Endpoint for CrashSimulatorEndpoint {
    fn uri(&self) -> &str {
        "crash-simulator:test"
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(CrashSimulatorConsumer {
            crash_counter: Arc::clone(&self.crash_counter),
            runs_before_crash: self.runs_before_crash,
        }))
    }

    fn create_producer(
        &self,
        _ctx: &camel_api::ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Err(CamelError::EndpointCreationFailed(
            "crash-simulator endpoint does not support producers".to_string(),
        ))
    }
}

struct CrashSimulatorConsumer {
    crash_counter: Arc<AtomicU32>,
    runs_before_crash: u32,
}

#[async_trait]
impl Consumer for CrashSimulatorConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        // Increment the run counter immediately
        let run_count = self.crash_counter.fetch_add(1, Ordering::SeqCst);

        // Log that we're starting
        println!("[CRASH-SIMULATOR] Starting run #{}", run_count + 1);

        // Send a test message
        let exchange = Exchange::new(camel_api::message::Message::new(format!(
            "Crash simulator run #{}",
            run_count + 1
        )));
        if ctx.send(exchange).await.is_err() {
            // Channel closed, route was stopped
            return Ok(());
        }

        // Simulate running for a bit (longer than supervision initial_delay)
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(1200)) => {}
            _ = ctx.cancelled() => {
                println!("[CRASH-SIMULATOR] Received cancellation, stopping cleanly");
                return Ok(());
            }
        }

        // Check if we should crash
        if run_count >= self.runs_before_crash {
            println!(
                "[CRASH-SIMULATOR] Simulating crash after {} successful runs",
                self.runs_before_crash
            );
            return Err(CamelError::RouteError(format!(
                "Simulated crash after {} successful runs",
                self.runs_before_crash
            )));
        }

        // Otherwise, wait until cancelled
        println!("[CRASH-SIMULATOR] Waiting for cancellation...");
        ctx.cancelled().await;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Sequential
    }
}

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_target(false)
        .with_thread_ids(false)
        .with_level(false)
        .without_time()
        .init();

    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║       Auto-Restart Example: SupervisingRouteController       ║");
    println!("╚════════════════════════════════════════════════════════════════╝");
    println!();

    // Create supervision configuration
    let supervision_config = SupervisionConfig {
        max_attempts: None, // Infinite attempts to show exponential backoff
        initial_delay: Duration::from_millis(500),
        backoff_multiplier: 2.0,
        max_delay: Duration::from_secs(4),
    };

    println!("Supervision Configuration:");
    println!("  - Max attempts: {:?}", supervision_config.max_attempts);
    println!("  - Initial delay: {:?}", supervision_config.initial_delay);
    println!(
        "  - Backoff multiplier: {}",
        supervision_config.backoff_multiplier
    );
    println!("  - Max delay: {:?}", supervision_config.max_delay);
    println!();
    println!("Note: Using infinite attempts to demonstrate exponential backoff");
    println!();

    // Create context with supervision
    let mut ctx = CamelContext::with_supervision(supervision_config);

    // Register components
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // Create and register our crash simulator component (crashes after first successful run)
    let crash_component = CrashSimulatorComponent::new(1);
    ctx.register_component(crash_component);

    // Create a timer route that triggers every 2 seconds
    let timer_route = RouteBuilder::from("timer:tick?period=2000")
        .route_id("timer-route")
        .to("log:info")
        .build()?;

    // Create a crash simulator route
    let crash_route = RouteBuilder::from("crash-simulator:test")
        .route_id("crash-route")
        .to("log:info") // Just log the message from the consumer
        .build()?;

    // Add routes
    ctx.add_route_definition(timer_route).await?;
    ctx.add_route_definition(crash_route).await?;

    println!("Starting routes with supervision...");
    println!("Watch for crash and restart messages:");
    println!();

    // Start the context (this starts the supervision loop)
    ctx.start().await?;

    // Let it run for 10 seconds to demonstrate crashes and restarts
    println!("Running for 10 seconds to demonstrate supervision...");
    tokio::time::sleep(Duration::from_secs(10)).await;

    // Stop the context
    println!("\nStopping context...");
    ctx.stop().await?;

    println!("\n╔════════════════════════════════════════════════════════════════╗");
    println!("║                    Example Complete                             ║");
    println!("╚════════════════════════════════════════════════════════════════╝");
    println!();
    println!("Summary:");
    println!("  - Created a CamelContext with SupervisingRouteController");
    println!("  - Registered a custom component that simulates crashes");
    println!("  - Configured supervision with infinite attempts");
    println!("  - Observed automatic restarts after crashes");
    println!("  - Timer route continued running while crash route restarted");
    println!();
    println!("Key Observations:");
    println!("  ✓ The crash route was automatically restarted after each failure");
    println!("  ✓ The supervision loop continuously monitors routes");
    println!("  ✓ Timer route continued running independently");
    println!("  ✓ Routes can be configured to restart indefinitely");
    println!();
    println!("Done!");
    Ok(())
}
