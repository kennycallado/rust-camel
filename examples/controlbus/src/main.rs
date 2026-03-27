use camel_api::body::Body;
use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_controlbus::ControlBusComponent;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(ControlBusComponent::new());

    // Target route that will be controlled
    let target_route = RouteBuilder::from("timer:target?period=500")
        .route_id("target-route")
        .auto_startup(true)
        .process(|mut exchange| async move {
            exchange.input.body = Body::Text("Target route is running".to_string());
            Ok(exchange)
        })
        .to("log:target?showBody=true&showCorrelationId=true")
        .build()?;

    // Route that suspends the target after 5 seconds
    let suspend_route = RouteBuilder::from("timer:suspend?delay=5000&repeatCount=1")
        .route_id("suspend-controller")
        .process(|mut exchange| async move {
            println!("[CONTROL] Suspending target-route...");
            exchange
                .input
                .set_header("CamelRouteId", Value::String("target-route".into()));
            Ok(exchange)
        })
        .to("controlbus:route?action=suspend")
        .to("log:control?showBody=true")
        .build()?;

    // Route that resumes the target after 10 seconds
    let resume_route = RouteBuilder::from("timer:resume?delay=10000&repeatCount=1")
        .route_id("resume-controller")
        .process(|mut exchange| async move {
            println!("[CONTROL] Resuming target-route...");
            exchange
                .input
                .set_header("CamelRouteId", Value::String("target-route".into()));
            Ok(exchange)
        })
        .to("controlbus:route?action=resume")
        .to("log:control?showBody=true")
        .build()?;

    ctx.add_route_definition(target_route).await?;
    ctx.add_route_definition(suspend_route).await?;
    ctx.add_route_definition(resume_route).await?;
    ctx.start().await?;

    println!("ControlBus example running.");
    println!("  - target-route: runs every 500ms");
    println!("  - At 5s: suspend-controller suspends target-route");
    println!("  - At 10s: resume-controller resumes target-route");
    println!("Press Ctrl+C to stop.");

    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;
    Ok(())
}
