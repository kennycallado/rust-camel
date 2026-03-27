use camel_api::CamelError;
use camel_api::body::Body;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

/// Load Balancer EIP example — distribute messages across endpoints.
///
/// A timer fires every second. The load balancer distributes each message
/// to one of three log endpoints using RoundRobin strategy. Each endpoint
/// shows which message it received.
///
/// Flow:
///   timer -> load-balance (round-robin)
///       -> log:server-a
///       -> log:server-b
///       -> log:server-c
#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from("timer:tick?period=1000&repeatCount=10")
        .route_id("load-balancer-demo")
        // Give the exchange a meaningful body before load balancing
        .process(|mut exchange: camel_api::Exchange| {
            Box::pin(async move {
                exchange.input.body = Body::Text("hello from load balancer".to_string());
                Ok(exchange)
            })
        })
        // Load Balance: RoundRobin distributes messages across endpoints
        .load_balance()
        .to("log:server-a?showBody=true")
        .to("log:server-b?showBody=true")
        .to("log:server-c?showBody=true")
        .end_load_balance()
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("Load Balancer example running.");
    println!("Distributing messages to server-a, server-b, server-c using RoundRobin.");
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down load balancer example...");
    ctx.stop().await?;
    println!("Load balancer example stopped cleanly.");

    Ok(())
}
