use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use camel_api::body::Body;
use camel_api::circuit_breaker::CircuitBreakerConfig;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_core::context::CamelContext;
use camel_http::HttpComponent;
use camel_log::LogComponent;
use camel_timer::TimerComponent;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(HttpComponent::new());

    let failure_count = Arc::new(AtomicU32::new(0));
    let failure_clone = Arc::clone(&failure_count);

    // This example demonstrates circuit breaker pattern:
    // - First 5 calls will fail (simulating service outage)
    // - Circuit opens after 3 consecutive failures
    // - After open_duration (3s), circuit enters half-open state
    // - If probe succeeds, circuit closes and normal operation resumes
    let route = RouteBuilder::from("timer:cb-test?period=1000&repeatCount=15")
        .route_id("circuit-breaker-demo")
        .process(move |mut exchange| {
            let fc = Arc::clone(&failure_clone);
            async move {
                let n = fc.fetch_add(1, Ordering::SeqCst);
                // Fail first 5 calls, then succeed
                if n < 5 {
                    exchange.input.body = Body::Text(format!("call-{} (will fail)", n));
                    exchange.input.set_header(
                        "CamelHttpUrl",
                        Value::String("https://httpbin.org/status/500".into()),
                    );
                } else {
                    exchange.input.body = Body::Text(format!("call-{} (should succeed)", n));
                    exchange.input.set_header(
                        "CamelHttpUrl",
                        Value::String("https://httpbin.org/get".into()),
                    );
                }
                Ok(exchange)
            }
        })
        .to("header:CamelHttpUrl?allowPrivateIps=false")
        .to("log:cb-result?showBody=true&showCorrelationId=true")
        .circuit_breaker(
            CircuitBreakerConfig::new()
                .failure_threshold(3)
                .open_duration(Duration::from_secs(3)),
        )
        .error_handler(ErrorHandlerConfig::dead_letter_channel(
            "log:cb-dlc?showBody=true&showCorrelationId=true",
        ))
        .build()?;

    ctx.add_route_definition(route)?;
    ctx.start().await?;

    println!("Circuit Breaker example running.");
    println!("First 5 calls will fail (HTTP 500), then succeed (HTTP 200).");
    println!("Circuit opens after 3 failures, attempts recovery after 3 seconds.");
    println!("Press Ctrl+C to stop.");

    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;
    Ok(())
}
