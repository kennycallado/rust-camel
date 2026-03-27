use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use camel_api::CamelError;
use camel_api::aggregator::AggregatorConfig;
use camel_api::body::Body;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

/// Aggregator EIP example — order batching by correlation key.
///
/// A timer emits 12 exchanges with `orderId` cycling through A, B, C (4 each).
/// The Aggregator groups them by `orderId` with `completionSize(3)`, so 4
/// batches are emitted — one per unique order ID after 3 items. Each batch body
/// is a JSON array of the 3 individual item bodies.
///
/// Flow:
///   timer (12 fires) -> set orderId header + body
///                     -> aggregate(correlate_by=orderId, size=3)
///                     -> log each completed batch
#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let counter = Arc::new(AtomicU64::new(0));
    let counter_clone = Arc::clone(&counter);

    // Each timer fire sets orderId = "A" | "B" | "C" (cycling) and a body.
    // After 3 exchanges with the same orderId, the aggregator emits a batch.
    let route = RouteBuilder::from("timer:orders?period=200&repeatCount=12")
        .route_id("aggregator-demo")
        .process(move |mut ex: camel_api::Exchange| {
            let c = Arc::clone(&counter_clone);
            Box::pin(async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                let order_id = ["A", "B", "C"][(n % 3) as usize];
                ex.input
                    .headers
                    .insert("orderId".to_string(), serde_json::json!(order_id));
                ex.input.body = Body::Text(format!("order-item-{n}"));
                println!("[timer] #{n} orderId={order_id}");
                Ok(ex)
            })
        })
        .aggregate(
            AggregatorConfig::correlate_by("orderId")
                .complete_when_size(3)
                .max_buckets(100)
                .bucket_ttl(std::time::Duration::from_secs(60))
                .build(),
        )
        // Pending exchanges (Body::Empty, CamelAggregatorPending=true) still flow
        // through the pipeline — log only completed batches.
        .process(|ex: camel_api::Exchange| {
            Box::pin(async move {
                if ex.property("CamelAggregatorPending").is_some() {
                    return Ok(ex);
                }
                let key = ex
                    .property("CamelAggregatedKey")
                    .map(|v: &serde_json::Value| v.to_string())
                    .unwrap_or_default();
                let size = ex
                    .property("CamelAggregatedSize")
                    .map(|v: &serde_json::Value| v.to_string())
                    .unwrap_or_default();
                println!(
                    "[batch] orderId={} size={} body={:?}",
                    key, size, ex.input.body
                );
                Ok(ex)
            })
        })
        .to("log:batch?showBody=true&showCorrelationId=true")
        .error_handler(
            ErrorHandlerConfig::log_only()
                .on_exception(|_| true)
                .retry(1)
                .build(),
        )
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!();
    println!("Aggregator example running.");
    println!("12 timer fires, orderId cycles A/B/C, completionSize=3");
    println!("Expect 4 batches (one per orderId after 12 fires).");
    println!("Press Ctrl+C to stop.");
    println!();

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    ctx.stop().await?;
    println!("Done.");
    Ok(())
}
