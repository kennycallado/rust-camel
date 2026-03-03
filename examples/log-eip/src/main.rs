use camel_api::body::Body;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::CamelError;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_mock::MockComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_processor::LogLevel;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(MockComponent::new());

    let route = RouteBuilder::from("timer:demo?period=3000&repeatCount=5")
        .route_id("log-eip-demo")
        .log("=== Starting processing cycle ===", LogLevel::Warn)
        .process(|mut ex| async move {
            ex.input.body = Body::Json(serde_json::json!({
                "order_id": 12345,
                "items": ["widget", "gadget"],
                "total": 99.99
            }));
            ex.input
                .set_header("source", camel_api::Value::String("timer".into()));
            ex.input
                .set_header("priority", camel_api::Value::String("high".into()));
            Ok(ex)
        })
        .log("Order data prepared", LogLevel::Info)
        .log("Checking order priority", LogLevel::Error)
        .to("log:exchange-full?showBody=true&showHeaders=true&multiline=true&showCorrelationId=true")
        .process(|mut ex| async move {
            if let Some(priority) = ex.input.header("priority").and_then(|v| v.as_str()) {
                if priority == "high" {
                    ex.input
                        .set_header("CamelLogPriority", camel_api::Value::Bool(true));
                }
            }
            Ok(ex)
        })
        .log("Processing high priority order", LogLevel::Info)
        .to("log:after-processing?showBody=true&showCorrelationId=true")
        .log("Route execution complete", LogLevel::Trace)
        .log("=== Processing cycle complete ===", LogLevel::Info)
        .to("mock:result")
        .error_handler(
            ErrorHandlerConfig::log_only()
                .on_exception(|_| true)
                .retry(1)
                .build(),
        )
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
    println!("Press Ctrl+C to stop.");

    tokio::signal::ctrl_c().await.ok();

    println!();
    println!("Demo complete. Stopping...");

    ctx.stop().await?;
    Ok(())
}
