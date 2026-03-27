use camel_api::body::Body;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_http::HttpComponent;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(HttpComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from("timer:http-poll?period=5000&repeatCount=3")
        .route_id("http-client")
        .process(|mut exchange| {
            Box::pin(async move {
                exchange.input.body = Body::Empty;
                exchange
                    .input
                    .set_header("X-Request-Id", Value::String(uuid_header()));
                Ok(exchange)
            })
        })
        .to("https://httpbin.org/get?source=rust-camel&allowPrivateIps=false")
        .process(|exchange| {
            Box::pin(async move {
                let body_str = match &exchange.input.body {
                    Body::Text(s) => Some(s.as_str()),
                    Body::Bytes(b) => std::str::from_utf8(b).ok(),
                    _ => None,
                };
                if let Some(text) = body_str
                    && let Ok(json) = serde_json::from_str::<serde_json::Value>(text)
                {
                    println!(
                        "Response status: {:?}",
                        exchange.input.header("CamelHttpResponseCode")
                    );
                    if let Some(url) = json.get("url") {
                        println!("Response from: {}", url);
                    }
                }
                Ok(exchange)
            })
        })
        .to("log:http-response?showHeaders=true&showBody=true&showCorrelationId=true")
        .error_handler(
            ErrorHandlerConfig::dead_letter_channel("log:http-dlc?showBody=true")
                .on_exception(|_| true)
                .retry(2)
                .build(),
        )
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("HTTP client example running.");
    println!("Polling https://httpbin.org/get every 5 seconds.");
    println!("Press Ctrl+C to stop.");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    ctx.stop().await?;
    println!("Done.");

    Ok(())
}

fn uuid_header() -> String {
    uuid::Uuid::new_v4().to_string()
}
