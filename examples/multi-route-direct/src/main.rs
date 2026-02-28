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

    // Route 1: Timer -> set header -> send to direct:pipeline
    let route1 = RouteBuilder::from("timer:tick?period=1000&repeatCount=5")
        .set_header("source", Value::String("timer".into()))
        .to("direct:pipeline")
        .build()?;

    // Route 2: Receive from direct:pipeline -> uppercase body -> log
    let route2 = RouteBuilder::from("direct:pipeline")
        .map_body(|body| {
            if let Some(text) = body.as_text() {
                Body::Text(text.to_uppercase())
            } else {
                body
            }
        })
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
