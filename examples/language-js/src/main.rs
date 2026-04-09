//! # language-js example
//!
//! Demonstrates using `camel-language-js` from a route `.script("js", ...)` step.

use std::time::Duration;

use camel_api::CamelError;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from("timer:tick?period=1000&repeatCount=3")
        .route_id("language-js-demo")
        .script(
            "js",
            r#"
            camel.headers.set("greeting", "Hello from JS");
            camel.headers.set("processedBy", "language-js-example");
            camel.body = "JS was here";
            "done";
            "#,
        )
        .to("log:js-output?showBody=true&showHeaders=true")
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("JS Language example running for ~3 ticks...");

    tokio::time::sleep(Duration::from_millis(3500)).await;

    ctx.stop().await?;
    println!("Stopped.");

    Ok(())
}
