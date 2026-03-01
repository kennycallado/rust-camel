use camel_api::CamelError;
use camel_api::body::Body;
use camel_api::multicast::MulticastStrategy;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_core::context::CamelContext;
use camel_log::LogComponent;
use camel_timer::TimerComponent;

/// Multicast EIP example — fan-out to multiple endpoints.
///
/// A timer fires periodically. The same exchange is sent to all three
/// log endpoints concurrently (parallel mode) so each can process it
/// independently. The `CollectAll` aggregation strategy gathers all
/// responses into a JSON array which is logged at the end.
///
/// Flow:
///   timer -> multicast (parallel)
///       -> log:channel-a
///       -> log:channel-b
///       -> log:channel-c
///   (aggregated JSON array) -> log:summary
#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt::init();

    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from("timer:tick?period=2000&repeatCount=3")
        // Give the exchange a meaningful body before multicasting
        .process(|mut exchange: camel_api::Exchange| {
            Box::pin(async move {
                exchange.input.body = Body::Text("hello from multicast".to_string());
                Ok(exchange)
            })
        })
        // Multicast: same message goes to all three log endpoints in parallel.
        // CollectAll aggregation gathers each response body into a JSON array.
        .multicast()
        .parallel(true)
        .aggregation(MulticastStrategy::CollectAll)
        .to("log:channel-a?showBody=true")
        .to("log:channel-b?showBody=true")
        .to("log:channel-c?showBody=true")
        .end_multicast()
        // After multicast: the body is a JSON array of all three responses
        .to("log:summary?showBody=true")
        .build()?;

    ctx.add_route_definition(route)?;
    ctx.start().await?;

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("Shutting down...");
    ctx.stop().await?;
    println!("Done.");

    Ok(())
}
