//! Example: EIP Content Enricher — augment a message with data from a
//! resource endpoint.
//!
//! The `enrich` step calls a producer endpoint (`direct:enrich-data`) and
//! merges the response body into the original exchange using the default
//! `UseEnrichedBody` strategy (original headers/properties preserved).
//!
//! A second route consumes from `direct:enrich-data` and supplies the
//! enrichment payload — the canonical Camel two-route enrich pattern.

use camel_api::CamelError;
use camel_api::body::Body;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_direct::DirectComponent;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(DirectComponent::new());

    // Enrichment source: returns a fixed body when called as a producer.
    let enrichment_source = RouteBuilder::from("direct:enrich-data")
        .route_id("enrichment-source")
        .process(|mut exchange: camel_api::Exchange| {
            Box::pin(async move {
                exchange.input.body = Body::Text("enriched-value".into());
                Ok(exchange)
            })
        })
        .build()?;
    ctx.add_route_definition(enrichment_source).await?;

    // ANCHOR: content-enricher-route
    let route = RouteBuilder::from("timer:tick?period=1000&repeatCount=3")
        .route_id("content-enricher-demo")
        .enrich("direct:enrich-data")
        .to("log:enriched?showBody=true&showCorrelationId=true")
        .build()?;
    // ANCHOR_END: content-enricher-route

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("Content enricher example running.");
    println!("Each timer tick calls direct:enrich-data and replaces the body.");
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down content enricher example...");
    ctx.stop().await?;
    println!("Content enricher example stopped cleanly.");

    Ok(())
}
