use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use camel_api::CamelError;
use camel_api::body::Body;
use camel_api::circuit_breaker::CircuitBreakerConfig;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_direct::DirectComponent;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(DirectComponent::new());

    let failure_count = Arc::new(AtomicU32::new(0));
    let failure_clone = Arc::clone(&failure_count);

    // Simulated failing service - fails first 5 calls, then succeeds
    let failing_service = RouteBuilder::from("direct:failing-service")
        .route_id("failing-service")
        .process(move |exchange| {
            let fc = Arc::clone(&failure_clone);
            async move {
                let n = fc.fetch_add(1, Ordering::SeqCst);
                if n < 5 {
                    println!("[SERVICE] Call {} - SIMULATING FAILURE", n);
                    Err(CamelError::ProcessorError(format!(
                        "Service unavailable (call {})",
                        n
                    )))
                } else {
                    println!("[SERVICE] Call {} - SUCCESS", n);
                    Ok(exchange)
                }
            }
        })
        .build()?;

    // Main route with circuit breaker
    let main_route = RouteBuilder::from("timer:cb-test?period=1000&repeatCount=15")
        .route_id("circuit-breaker-demo")
        .process(|mut exchange| async move {
            let n = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            exchange.input.body = Body::Text(format!("request-{}", n));
            Ok(exchange)
        })
        .to("direct:failing-service")
        .circuit_breaker(
            CircuitBreakerConfig::new()
                .failure_threshold(3)
                .open_duration(Duration::from_secs(3)),
        )
        .to("log:cb-success?showBody=true&showCorrelationId=true")
        .error_handler(
            ErrorHandlerConfig::dead_letter_channel(
                "log:cb-fallback?showBody=true&showHeaders=true&showCorrelationId=true",
            )
            .on_exception(|_| true)
            .build(),
        )
        .build()?;

    ctx.add_route_definition(failing_service).await?;
    ctx.add_route_definition(main_route).await?;
    ctx.start().await?;

    println!("Circuit Breaker example running.");
    println!("  - Service fails first 5 calls, then succeeds");
    println!("  - Circuit opens after 3 consecutive failures");
    println!("  - After 3s, circuit enters half-open state");
    println!("  - Watch logs: cb-success (working) vs cb-fallback (circuit open)");
    println!("Press Ctrl+C to stop.");

    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;
    Ok(())
}
