use camel_api::body::Body;
use camel_api::{CamelError, Value};
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

    let route = RouteBuilder::from("timer:tick?period=1000&repeatCount=5")
        .set_header("timestamp", Value::String("processed".into()))
        .set_header("version", Value::Number(1.into()))
        .map_body(|body: Body| {
            let original = body.as_text().unwrap_or("empty").to_string();
            Body::Json(serde_json::json!({
                "original": original,
                "transformed": true,
            }))
        })
        .to("log:output?showHeaders=true&showBody=true")
        .build()?;

    ctx.add_route_definition(route)?;
    ctx.start().await?;

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    ctx.stop().await?;

    Ok(())
}
