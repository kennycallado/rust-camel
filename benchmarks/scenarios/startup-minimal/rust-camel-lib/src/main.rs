//! Startup benchmark fixture - rust-camel-lib (Pair A partner).
//!
//! Mirrors Camel's `App.java` / `BenchRoute.java`: a hardcoded programmatic
//! route (timer -> log marker) that exercises the full public `CamelContext`
//! lifecycle: context construction -> component registration -> route
//! registration -> start -> timer fires -> marker logged -> idle until killed.
//!
//! Pair B (`rust-camel-yaml`) parses the equivalent route from a YAML/DSL
//! file at startup, exercising the same runtime but additionally paying the
//! parser cost. Comparing Pair A vs Pair B isolates "parser overhead" from
//! "runtime overhead" inside rust-camel.
//!
//! The harness measures RSS + wall-clock startup from outside this process;
//! the only in-band signal it watches for is the `BENCH_ROUTE_READY` log
//! line, which the EIP `.log()` step emits once the timer consumer fires.
//! After the marker, the CamelContext stays alive (matching Camel's `Main`
//! which blocks until external kill); the harness terminates the process
//! externally.

use camel_api::CamelError;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_processor::LogLevel;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    // Initialize the tracing subscriber first so every subsequent log line
    // (including the route's `.log(...)` EIP step output) lands on stdout.
    tracing_subscriber::fmt().with_target(false).init();

    // 1. Construct the CamelContext via the public builder.
    let mut ctx = CamelContext::builder().build().await?;

    // 2. Register the only component the route touches. rust-camel has no
    //    META-INF-style auto-discovery yet, so registration is explicit.
    //    The EIP `.log()` step below resolves to camel_processor::LogProcessor
    //    directly, so no `log:` endpoint component is needed (mirrors
    //    Camel Pair A's pom, which carries only camel-main + camel-timer).
    ctx.register_component(TimerComponent::new());

    // 3. Build the route programmatically - no YAML/DSL parsing.
    //    Mirrors Camel Pair A: `from("timer:bench?repeatCount=1&delay=0")
    //    .log("BENCH_ROUTE_READY")`. `period` is left at its 1000ms
    //    default; it is irrelevant because `repeatCount=1` fires exactly
    //    once after the (zero) initial delay, then the consumer stops.
    let route = RouteBuilder::from("timer:bench?repeatCount=1&delay=0")
        .route_id("bench-route")
        .log("BENCH_ROUTE_READY", LogLevel::Info)
        .build()?;

    // 4. Register the route and start the context.
    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    // 5. Keep the CamelContext alive until killed by the benchmark harness.
    //    The timer has already fired (repeatCount=1) and its consumer has
    //    stopped, but the context itself stays up - matching Camel's
    //    `App.java` which keeps `Main` running until external kill.
    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;
    Ok(())
}
