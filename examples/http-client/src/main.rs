use camel_api::{body::Body, CamelError};
use camel_builder::RouteBuilder;
use camel_core::context::CamelContext;
use camel_http::HttpComponent;
use camel_log::LogComponent;
use camel_timer::TimerComponent;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt::init();

    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(HttpComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from("timer:http-poll?period=10000")
        .process(|mut exchange| {
            Box::pin(async move {
                exchange.input.body = Body::Text("{}".into());
                Ok(exchange)
            })
        })
        .set_header("Content-Type", camel_api::Value::String("application/json".into()))
        .to("https://httpbin.org/post?httpMethod=POST")
        .process(|exchange| {
            Box::pin(async move {
                if let Body::Text(ref body) = exchange.input.body {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
                        println!("Response status: {:?}", exchange.input.header("CamelHttpResponseCode"));
                        if let Some(url) = json.get("url") {
                            println!("Posted to: {}", url);
                        }
                    }
                }
                Ok(exchange)
            })
        })
        .to("log:http-response?showHeaders=true&showBody=true")
        .build()?;

    ctx.add_route_definition(route)?;
    ctx.start().await?;

    println!("HTTP client example running.");
    println!("  Endpoint: https://httpbin.org/post");
    println!("  Method: POST");
    println!("  Period: 10 seconds");
    println!();
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down...");
    ctx.stop().await?;
    println!("Done.");

    Ok(())
}
