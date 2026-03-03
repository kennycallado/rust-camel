use camel_api::body::Body;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::new();

    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from("timer:transform?period=1000&repeatCount=3")
        .route_id("transform-pipeline")
        .process(|mut exchange| async move {
            exchange.input.body = Body::Text("hello world".to_string());
            Ok(exchange)
        })
        .map_body(|body: Body| {
            if let Some(text) = body.as_text() {
                Body::Text(text.to_uppercase())
            } else {
                body
            }
        })
        .set_header("transformed", Value::Bool(true))
        .to("log:info?showHeaders=true&showBody=true&showCorrelationId=true")
        .error_handler(
            ErrorHandlerConfig::log_only()
                .on_exception(|_| true)
                .retry(1)
                .build(),
        )
        .build()?;

    ctx.add_route_definition(route)?;
    ctx.start().await?;

    println!("Transform pipeline running. Press Ctrl+C to stop.");

    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;
    Ok(())
}
