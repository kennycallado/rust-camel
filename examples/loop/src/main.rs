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

    // Count-mode loop: runs the sub-pipeline 3 times per exchange.
    // Each iteration appends "!" to the body. After 3 iterations: "hello!!!"
    let loop_route = RouteBuilder::from("timer:tick?period=3000&repeatCount=3")
        .route_id("loop-count-demo")
        .set_body("hello")
        .loop_count(3)
        .process(|mut ex: camel_api::Exchange| async move {
            let body = ex.input.body.as_text().unwrap_or("").to_string();
            ex.input.body = camel_api::body::Body::Text(format!("{body}!"));
            Ok(ex)
        })
        .end_loop()
        .to("log:loop-result?level=info&showBody=true")
        .build()?;

    ctx.add_route_definition(loop_route).await?;
    ctx.start().await?;

    println!("Loop example running.");
    println!("Route: timer -> set_body(hello) -> loop(3x append '!') -> log:loop-result");
    println!("Expected output body: hello!!!");
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down loop example...");
    ctx.stop().await?;
    println!("Loop example stopped cleanly.");

    Ok(())
}
