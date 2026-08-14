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

    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // Rotating destination iterator: a, b, c, a, b, c, ...
    let destinations = Arc::new(Mutex::new(vec!["a", "b", "c"].into_iter().cycle()));
    let dest_clone = destinations.clone();

    // Per-exchange routing state: the dynamic router calls the closure
    // repeatedly on the SAME exchange until it returns None. The log endpoint
    // does not clear the `destination` header, so a plain header read would
    // return the same URI on every iteration and trip the router's infinite
    // loop guard. Track which exchanges have already been routed: first call
    // routes, the second returns None (routing complete). Keys are removed on
    // completion so the set stays bounded.
    let routed: Arc<Mutex<std::collections::HashSet<String>>> =
        Arc::new(Mutex::new(std::collections::HashSet::new()));
    let routed_clone = routed.clone();

    // ANCHOR: dynamic-router-route
    let route = RouteBuilder::from("timer:tick?period=1000&repeatCount=10")
        .route_id("dynamic-router-demo")
        // Set a rotating destination header
        .process(move |mut exchange: camel_api::Exchange| {
            let dests = dest_clone.clone();
            Box::pin(async move {
                let mut d = dests.lock().unwrap(); // allow-unwrap
                let dest = d.next().unwrap(); // allow-unwrap
                exchange
                    .input
                    .set_header("destination", Value::String(dest.to_string()));
                exchange.input.body = Body::Text(format!("routed to {}", dest));
                Ok(exchange)
            })
        })
        // Dynamic router: read header and route to correct endpoint
        .dynamic_router(Arc::new(move |exchange: &camel_api::Exchange| {
            let mut routed = routed_clone.lock().unwrap(); // allow-unwrap
            let key = exchange.correlation_id().to_string();
            if routed.insert(key.clone()) {
                let dest = exchange
                    .input
                    .header("destination")
                    .and_then(|v| v.as_str())
                    .unwrap_or("a");
                Some(format!(
                    "log:routed-{}?showBody=true&showHeaders=true",
                    dest
                ))
            } else {
                // Second call on the same exchange: routing is complete.
                routed.remove(&key);
                None
            }
        }))
        .build()?;
    // ANCHOR_END: dynamic-router-route

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
