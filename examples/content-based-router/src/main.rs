use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use camel_api::CamelError;
use camel_api::body::Body;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap

    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let counter = Arc::new(AtomicU64::new(0));
    let counter_clone = Arc::clone(&counter);

    // ANCHOR: cbr-route
    let route = RouteBuilder::from("timer:tick?period=1000&repeatCount=6")
        .route_id("content-based-router-demo")
        .process(move |mut exchange: camel_api::Exchange| {
            let c = Arc::clone(&counter_clone);
            Box::pin(async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                let priority = match n % 3 {
                    0 => "high",
                    1 => "medium",
                    _ => "low",
                };
                exchange.input.body = Body::Text(priority.into());
                Ok(exchange)
            })
        })
        .choice()
        .when(|ex| ex.input.body.as_text() == Some("high"))
        .to("log:high-priority?showBody=true&showCorrelationId=true")
        .end_when()
        .when(|ex| ex.input.body.as_text() == Some("medium"))
        .to("log:medium-priority?showBody=true&showCorrelationId=true")
        .end_when()
        .otherwise()
        .to("log:low-priority?showBody=true&showCorrelationId=true")
        .end_otherwise()
        .end_choice()
        .error_handler(
            ErrorHandlerConfig::log_only()
                .on_exception(|_| true)
                .retry(1)
                .build(),
        )
        .build()?;
    // ANCHOR_END: cbr-route

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("Content-based router example running.");
    println!("Routing timer fires by priority (high, medium, low).");
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down content-based router example...");
    ctx.stop().await?;
    println!("Content-based router example stopped cleanly.");

    Ok(())
}
