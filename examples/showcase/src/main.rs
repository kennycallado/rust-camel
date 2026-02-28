use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use camel_api::body::Body;
use camel_api::{CamelError, Value};
use camel_builder::RouteBuilder;
use camel_core::context::CamelContext;
use camel_direct::DirectComponent;
use camel_log::LogComponent;
use camel_timer::TimerComponent;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt::init();

    let mut ctx = CamelContext::new();

    let direct = DirectComponent::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(direct);

    // Route 1: Timer -> process (alternate body) -> set header -> dispatch
    let counter = Arc::new(AtomicU64::new(0));
    let counter_clone = Arc::clone(&counter);

    let route1 = RouteBuilder::from("timer:events?period=1000&repeatCount=8")
        .process(move |mut exchange| {
            let c = Arc::clone(&counter_clone);
            Box::pin(async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n.is_multiple_of(2) {
                    exchange.input.body = Body::Text("typeA".into());
                } else {
                    exchange.input.body = Body::Text("typeB".into());
                }
                Ok(exchange)
            })
        })
        .set_header("source", Value::String("timer".into()))
        .to("direct:dispatcher")
        .build()?;

    // Route 2: Receive -> filter typeA only -> uppercase -> mark processed -> log
    let route2 = RouteBuilder::from("direct:dispatcher")
        .filter(|ex| ex.input.body.as_text() == Some("typeA"))
        .map_body(|body| {
            if let Some(text) = body.as_text() {
                Body::Text(text.to_uppercase())
            } else {
                body
            }
        })
        .set_header("processed", Value::Bool(true))
        .to("log:output?showHeaders=true&showBody=true")
        .build()?;

    ctx.add_route_definition(route1)?;
    ctx.add_route_definition(route2)?;
    ctx.start().await?;

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    ctx.stop().await?;

    Ok(())
}
