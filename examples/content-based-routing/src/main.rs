use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use camel_api::CamelError;
use camel_api::body::Body;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_core::context::CamelContext;
use camel_log::LogComponent;
use camel_timer::TimerComponent;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt::init();

    let mut ctx = CamelContext::new();

    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let counter = Arc::new(AtomicU64::new(0));
    let counter_clone = Arc::clone(&counter);

    let route = RouteBuilder::from("timer:tick?period=1000&repeatCount=10")
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
            .to("log:filtered?showBody=true")
        .end_filter()
        .build()?;

    ctx.add_route_definition(route)?;
    ctx.start().await?;

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    ctx.stop().await?;

    Ok(())
}
