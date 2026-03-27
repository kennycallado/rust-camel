use std::sync::{Arc, Mutex};

use camel_api::body::Body;
use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

/// Dynamic Router EIP example — route based on runtime header values.
///
/// A timer fires every second. A process step sets a `destination` header
/// that cycles through "a", "b", "c". The dynamic router reads this header
/// at runtime and routes to the appropriate log endpoint.
///
/// Flow:
///   timer -> process (set rotating destination header)
///         -> dynamic_router (reads header, routes to log:routed-{dest})
///
/// Output rotates between:
///   - log:routed-a
///   - log:routed-b
///   - log:routed-c
#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // Rotating destination iterator: a, b, c, a, b, c, ...
    let destinations = Arc::new(Mutex::new(vec!["a", "b", "c"].into_iter().cycle()));
    let dest_clone = destinations.clone();

    let route = RouteBuilder::from("timer:tick?period=1000&repeatCount=10")
        .route_id("dynamic-router-demo")
        // Set a rotating destination header
        .process(move |mut exchange: camel_api::Exchange| {
            let dests = dest_clone.clone();
            Box::pin(async move {
                let mut d = dests.lock().unwrap();
                let dest = d.next().unwrap();
                exchange
                    .input
                    .set_header("destination", Value::String(dest.to_string()));
                exchange.input.body = Body::Text(format!("routed to {}", dest));
                Ok(exchange)
            })
        })
        // Dynamic router: read header and route to correct endpoint
        .dynamic_router(Arc::new(|exchange: &camel_api::Exchange| {
            let dest = exchange
                .input
                .header("destination")
                .and_then(|v| v.as_str())
                .unwrap_or("a");
            Some(format!(
                "log:routed-{}?showBody=true&showHeaders=true",
                dest
            ))
        }))
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("Dynamic Router example running.");
    println!("Messages will rotate between routed-a, routed-b, routed-c.");
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down dynamic router example...");
    ctx.stop().await?;
    println!("Dynamic router example stopped cleanly.");

    Ok(())
}
