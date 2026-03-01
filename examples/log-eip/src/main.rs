use camel_api::body::Body;
use camel_api::CamelError;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_core::context::CamelContext;
use camel_log::LogComponent;
use camel_mock::MockComponent;
use camel_processor::LogLevel;
use camel_timer::TimerComponent;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt::init();

    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(MockComponent::new());

    let route = RouteBuilder::from("timer:demo?period=3000&repeatCount=5")
        .log("=== Starting processing cycle ===")
        .process(|mut ex| async move {
            ex.input.body = Body::Json(serde_json::json!({
                "order_id": 12345,
                "items": ["widget", "gadget"],
                "total": 99.99
            }));
            ex.input.set_header("source", camel_api::Value::String("timer".into()));
            ex.input.set_header("priority", camel_api::Value::String("high".into()));
            Ok(ex)
        })
        .log("Order data prepared")
        .to("log:exchange-full?showBody=true&showHeaders=true&multiline=true")
        .process(|mut ex| async move {
            if let Some(priority) = ex.input.header("priority").and_then(|v| v.as_str()) {
                if priority == "high" {
                    ex.input.set_header("CamelLogPriority", camel_api::Value::Bool(true));
                }
            }
            Ok(ex)
        })
        .log_level(LogLevel::Debug, "Processing high priority order")
        .to("log:after-processing?showBody=true")
        .log("=== Processing cycle complete ===")
        .to("mock:result")
        .build()?;

    ctx.add_route_definition(route)?;
    ctx.start().await?;

    println!("╔════════════════════════════════════════════════════════════╗");
    println!("║         Log EIP vs Log Component Demo                      ║");
    println!("╚════════════════════════════════════════════════════════════╝");
    println!();
    println!("This example demonstrates the difference between:");
    println!();
    println!("  Log EIP (.log())            Log Component (.to(\"log:...\"))");
    println!("  ─────────────────────       ─────────────────────────────────");
    println!("  Simple human messages       Full exchange inspection");
    println!("  Just logs a string          Shows body, headers, properties");
    println!("  Lightweight                 Configurable output");
    println!("  For progress/status         For debugging");
    println!();
    println!("Watch the output below:");
    println!("  - Messages like '=== Starting...' are from .log()");
    println!("  - Detailed Exchange[...] blocks are from .to(\"log:\")");
    println!();
    println!("Running for 5 cycles (3s each)...");
    println!("─────────────────────────────────────────────────────────────");

    tokio::time::sleep(tokio::time::Duration::from_secs(16)).await;

    println!();
    println!("Demo complete. Stopping...");

    ctx.stop().await?;
    Ok(())
}
