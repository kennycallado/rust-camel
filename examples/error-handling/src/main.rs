//! # Error Handling Patterns — A Complete Tour
//!
//! This example demonstrates every error handling pattern available in rust-camel:
//!
//! 1. **Dead Letter Channel (DLC)** — failed exchanges forwarded to a fallback endpoint
//! 2. **Retry with exponential backoff** — transient failures retried before giving up
//! 3. **`onException` with `handled_by`** — route errors to a specific endpoint based on type
//! 4. **`direct:` error propagation** — subroute errors bubble up to the caller's handler
//! 5. **Global vs per-route handlers** — per-route takes precedence over context-level config

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{BoxProcessor, BoxProcessorExt, CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_direct::DirectComponent;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

// ---------------------------------------------------------------------------
// Helpers — reusable failing processors
// ---------------------------------------------------------------------------

/// A processor that always fails. Simulates a broken step in the pipeline.
fn always_fail(reason: &'static str) -> BoxProcessor {
    BoxProcessor::from_fn(move |_ex| {
        Box::pin(async move { Err(CamelError::ProcessorError(reason.into())) })
    })
}

/// A processor that fails N times then succeeds — simulates a transient error.
fn fail_n_times(times: u32) -> BoxProcessor {
    let counter = Arc::new(AtomicU32::new(0));
    BoxProcessor::from_fn(move |ex| {
        let c = Arc::clone(&counter);
        Box::pin(async move {
            let n = c.fetch_add(1, Ordering::SeqCst);
            if n < times {
                tracing::warn!(attempt = n + 1, "Transient failure (will retry)");
                Err(CamelError::ProcessorError(format!(
                    "transient error, attempt {}",
                    n + 1
                )))
            } else {
                tracing::info!(attempt = n + 1, "Recovered after retries");
                Ok(ex)
            }
        })
    })
}

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::new();

    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(DirectComponent::new());

    // -----------------------------------------------------------------------
    // Global error handler: any route without a per-route handler uses this.
    // -----------------------------------------------------------------------
    ctx.set_error_handler(ErrorHandlerConfig::dead_letter_channel(
        "log:global-dlc?showHeaders=true&showBody=true&showCorrelationId=true",
    ));

    // -----------------------------------------------------------------------
    // Route 1: Basic Dead Letter Channel
    //
    // A timer fires once. The processor always fails. The per-route DLC
    // catches the error and logs it to "log:route1-dlc".
    // -----------------------------------------------------------------------
    let route1 = RouteBuilder::from("timer:route1?period=2000&repeatCount=1")
        .set_header("example", Value::String("basic-dlc".into()))
        .process_fn(always_fail("route1: permanent failure"))
        .error_handler(ErrorHandlerConfig::dead_letter_channel(
            "log:route1-dlc?showHeaders=true&showBody=true&showCorrelationId=true",
        ))
        .build()?;

    // -----------------------------------------------------------------------
    // Route 2: Retry with exponential backoff
    //
    // The processor fails twice then succeeds on the third attempt. The error
    // handler retries up to 3 times with exponential backoff (50ms → 100ms).
    // The DLC is never reached because the retry recovers the exchange.
    // -----------------------------------------------------------------------
    let route2 = RouteBuilder::from("timer:route2?period=2000&repeatCount=1")
        .set_header("example", Value::String("retry-backoff".into()))
        .process_fn(fail_n_times(2))
        .error_handler(
            ErrorHandlerConfig::dead_letter_channel(
                "log:route2-dlc?showHeaders=true&showBody=true&showCorrelationId=true",
            )
            .on_exception(|_| true) // match all errors
            .retry(3)
            .with_backoff(Duration::from_millis(50), 2.0, Duration::from_secs(1))
            .build(),
        )
        .build()?;

    // -----------------------------------------------------------------------
    // Route 3: onException with handled_by
    //
    // Processor errors are routed to a specific handler ("log:processor-errors")
    // instead of the default DLC. Other error types would still go to the DLC.
    // -----------------------------------------------------------------------
    let route3 = RouteBuilder::from("timer:route3?period=2000&repeatCount=1")
        .set_header("example", Value::String("on-exception-handled-by".into()))
        .process_fn(always_fail("route3: processor error"))
        .error_handler(
            ErrorHandlerConfig::dead_letter_channel(
                "log:route3-dlc?showHeaders=true&showBody=true&showCorrelationId=true",
            )
            .on_exception(|e| matches!(e, CamelError::ProcessorError(_)))
            .retry(1)
            .handled_by(
                "log:processor-errors?showHeaders=true&showBody=true&showCorrelationId=true",
            )
            .build(),
        )
        .build()?;

    // -----------------------------------------------------------------------
    // Route 4: direct: error propagation — bubble up
    //
    // A subroute ("direct:fragile") fails without its own error handler.
    // The error propagates back to the calling route, whose DLC catches it.
    // This demonstrates the ExchangeEnvelope request-reply pattern.
    // -----------------------------------------------------------------------
    let subroute_no_handler = RouteBuilder::from("direct:fragile")
        .process_fn(always_fail("subroute: fragile step failed"))
        .build()?;

    let route4 = RouteBuilder::from("timer:route4?period=2000&repeatCount=1")
        .set_header("example", Value::String("direct-bubble-up".into()))
        .to("direct:fragile")
        .error_handler(ErrorHandlerConfig::dead_letter_channel(
            "log:route4-dlc?showHeaders=true&showBody=true&showCorrelationId=true",
        ))
        .build()?;

    // -----------------------------------------------------------------------
    // Route 5: direct: error contained in subroute
    //
    // A subroute ("direct:resilient") fails but has its own DLC. The error
    // is absorbed; the calling route continues processing normally.
    // -----------------------------------------------------------------------
    let subroute_with_handler = RouteBuilder::from("direct:resilient")
        .process_fn(always_fail("subroute: resilient step failed"))
        .error_handler(ErrorHandlerConfig::dead_letter_channel(
            "log:subroute-dlc?showHeaders=true&showBody=true&showCorrelationId=true",
        ))
        .build()?;

    let route5 = RouteBuilder::from("timer:route5?period=2000&repeatCount=1")
        .set_header("example", Value::String("direct-contained".into()))
        .to("direct:resilient")
        .to("log:route5-continued?showHeaders=true&showBody=true&showCorrelationId=true")
        .build()?;

    // -----------------------------------------------------------------------
    // Route 6: Global error handler fallback (no per-route handler)
    //
    // This route has no .error_handler(). It falls back to the global handler
    // set on CamelContext via ctx.set_error_handler().
    // -----------------------------------------------------------------------
    let route6 = RouteBuilder::from("timer:route6?period=2000&repeatCount=1")
        .set_header("example", Value::String("global-fallback".into()))
        .process_fn(always_fail("route6: uses global handler"))
        .build()?;

    // --- Register all routes ---
    ctx.add_route_definition(subroute_no_handler)?;
    ctx.add_route_definition(subroute_with_handler)?;
    ctx.add_route_definition(route1)?;
    ctx.add_route_definition(route2)?;
    ctx.add_route_definition(route3)?;
    ctx.add_route_definition(route4)?;
    ctx.add_route_definition(route5)?;
    ctx.add_route_definition(route6)?;

    ctx.start().await?;

    println!("\n=== Error Handling Example Running ===");
    println!("Watch the logs to see each pattern in action:");
    println!("  Route 1: Basic DLC (immediate failure → log:route1-dlc)");
    println!("  Route 2: Retry with backoff (fails 2x, recovers on 3rd)");
    println!("  Route 3: onException handled_by (→ log:processor-errors)");
    println!("  Route 4: direct: bubble up (subroute error → caller's DLC)");
    println!("  Route 5: direct: contained (subroute DLC absorbs error, caller continues)");
    println!("  Route 6: Global fallback (no per-route handler → log:global-dlc)");
    println!("\nPress Ctrl+C to stop.\n");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    ctx.stop().await?;

    Ok(())
}
