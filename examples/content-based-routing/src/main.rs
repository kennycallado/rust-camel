use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use camel_api::CamelError;
use camel_api::body::Body;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_core::context::CamelContext;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::new();

    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let counter = Arc::new(AtomicU64::new(0));
    let counter_clone = Arc::clone(&counter);

    let route = RouteBuilder::from("timer:tick?period=1000&repeatCount=10")
        .route_id("content-based-routing-demo")
        .process(move |mut exchange: camel_api::Exchange| {
            let c = Arc::clone(&counter_clone);
            Box::pin(async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n.is_multiple_of(2) {
                    exchange.input.body = Body::Text("important".into());
                } else {
                    exchange.input.body = Body::Text("routine".into());
                }
                Ok(exchange)
            })
        })
        .filter(|ex| ex.input.body.as_text() == Some("important"))
        .to("log:filtered?showBody=true&showCorrelationId=true")
        .end_filter()
        .error_handler(
            ErrorHandlerConfig::log_only()
                .on_exception(|_| true)
                .retry(1)
                .build(),
        )
        .build()?;

    ctx.add_route_definition(route)?;
    ctx.start().await?;

    println!("Content-based routing example running.");
    println!("Filtering for 'important' messages (every other message).");
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down content-based routing example...");
    ctx.stop().await?;
    println!("Content-based routing example stopped cleanly.");

    Ok(())
}
