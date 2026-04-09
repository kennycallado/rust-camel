use std::sync::Arc;

use camel_api::body::Body;
use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

/// Routing Slip EIP example — route through a dynamic sequence of endpoints.
///
/// A timer fires every second. A process step determines a routing slip
/// based on a counter: odd messages go through "log:step-a -> log:step-b",
/// even messages go through "log:step-c -> log:step-d".
///
/// Flow:
///   timer -> process (determine slip based on counter)
///         -> routing_slip (evaluates once, routes through endpoint sequence)
#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter_clone = counter.clone();

    let route = RouteBuilder::from("timer:tick?period=1000&repeatCount=10")
        .route_id("routing-slip-demo")
        .process(move |mut exchange: camel_api::Exchange| {
            let c = counter_clone.clone();
            Box::pin(async move {
                let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let slip = if n.is_multiple_of(2) {
                    "log:step-a?showBody=true,log:step-b?showBody=true"
                } else {
                    "log:step-c?showBody=true,log:step-d?showBody=true"
                };
                exchange
                    .input
                    .set_header("slip", Value::String(slip.to_string()));
                exchange.input.body = Body::Text(format!("message #{}", n));
                Ok(exchange)
            })
        })
        .routing_slip(Arc::new(|exchange: &camel_api::Exchange| {
            exchange
                .input
                .header("slip")
                .and_then(|v| v.as_str().map(|s| s.to_string()))
        }))
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("Routing Slip example running.");
    println!("Messages alternate between slip paths A->B and C->D.");
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down routing slip example...");
    ctx.stop().await?;
    println!("Routing slip example stopped cleanly.");

    Ok(())
}
