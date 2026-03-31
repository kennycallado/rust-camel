use std::time::Duration;

use camel_api::{Body, CamelError, RuntimeQuery, RuntimeQueryResult, UnitOfWorkConfig};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_direct::DirectComponent;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::CamelContext;
use tokio::time::sleep;

async fn print_in_flight_count(ctx: &CamelContext, route_id: &str) -> Result<(), CamelError> {
    match ctx
        .runtime()
        .ask(RuntimeQuery::InFlightCount {
            route_id: route_id.to_string(),
        })
        .await?
    {
        RuntimeQueryResult::InFlightCount { route_id, count } => {
            println!("[runtime] in-flight for '{route_id}': {count}");
        }
        RuntimeQueryResult::RouteNotFound { route_id } => {
            println!("[runtime] route not found: {route_id}");
        }
        other => {
            println!("[runtime] unexpected query result: {other:?}");
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(DirectComponent::new());

    let on_complete = RouteBuilder::from("direct:on-complete")
        .route_id("on-complete")
        .process(|mut exchange| async move {
            let body = exchange
                .input
                .body
                .as_text()
                .unwrap_or("<empty>")
                .to_string();
            exchange.input.body = Body::Text(format!("Exchange completed: {body}"));
            Ok(exchange)
        })
        .to("log:uow-complete?showBody=true&showCorrelationId=true")
        .build()?;

    let on_failure = RouteBuilder::from("direct:on-failure")
        .route_id("on-failure")
        .process(|mut exchange| async move {
            exchange.input.body = Body::Text("Exchange failed".to_string());
            Ok(exchange)
        })
        .to("log:uow-failed?showBody=true&showCorrelationId=true")
        .build()?;

    let success_route = RouteBuilder::from("timer:uow-success?delay=0&period=1200&repeatCount=4")
        .route_id("uow-success")
        .process(|mut exchange| async move {
            exchange.input.body = Body::Text("order-123".to_string());
            sleep(Duration::from_millis(450)).await;
            Ok(exchange)
        })
        .to("log:uow-main-success?showBody=true")
        .build()?
        .with_unit_of_work(UnitOfWorkConfig {
            on_complete: Some("direct:on-complete".to_string()),
            on_failure: None,
        });

    let failure_route = RouteBuilder::from("timer:uow-failure?delay=0&period=1800&repeatCount=2")
        .route_id("uow-failure")
        .process(|mut exchange| async move {
            exchange.input.body = Body::Text("will-fail".to_string());
            sleep(Duration::from_millis(300)).await;
            exchange.set_error(CamelError::ProcessorError("simulated failure".to_string()));
            Ok(exchange)
        })
        .build()?
        .with_unit_of_work(UnitOfWorkConfig {
            on_complete: None,
            on_failure: Some("direct:on-failure".to_string()),
        });

    // Hook routes must be registered before the routes that reference them as
    // UoW hook URIs. Hook URIs are resolved to producers when `add_route_definition`
    // is called — the target component (here `direct:`) must already be registered.
    ctx.add_route_definition(on_complete).await?;
    ctx.add_route_definition(on_failure).await?;
    ctx.add_route_definition(success_route).await?;
    ctx.add_route_definition(failure_route).await?;

    ctx.start().await?;

    println!("Exchange UoW example running.");
    println!("  - uow-success triggers on_complete => direct:on-complete");
    println!("  - uow-failure triggers on_failure => direct:on-failure");
    println!("  - runtime query prints in-flight counts for uow-success");
    println!("Press Ctrl+C to stop.");

    for _ in 0..8 {
        print_in_flight_count(&ctx, "uow-success").await?;
        sleep(Duration::from_millis(250)).await;
    }

    // Demonstrate not-found query result variant too.
    print_in_flight_count(&ctx, "missing-route").await?;

    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;
    Ok(())
}
