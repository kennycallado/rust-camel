use std::time::{SystemTime, UNIX_EPOCH};

use camel_api::CamelError;
use camel_api::body::Body;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_http::HttpComponent;
use camel_core::context::CamelContext;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(HttpComponent::new());

    // Route 1: /slow — no DSL override.
    //
    // HttpConsumer declares ConcurrencyModel::Concurrent by default. The
    // runtime spawns a task per exchange with no semaphore limit. Send 8
    // requests simultaneously and all 8 finish in ~200ms total.
    let route_slow = RouteBuilder::from("http://0.0.0.0:8080/slow")
        .route_id("concurrent-auto")
        .process(|mut exchange| async move {
            let started = now_ms();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let finished = now_ms();
            exchange.input.body = Body::Json(serde_json::json!({
                "path": "/slow",
                "mode": "auto-concurrent (HttpConsumer default, no override)",
                "started_at_ms": started,
                "finished_at_ms": finished,
            }));
            Ok(exchange)
        })
        .error_handler(
            ErrorHandlerConfig::log_only()
                .on_exception(|_| true)
                .retry(1)
                .build(),
        )
        .build()?;

    // Route 2: /limited — .concurrent(4) override.
    //
    // A semaphore caps pipeline parallelism at 4 simultaneous exchanges.
    // The 5th request blocks until one of the first 4 finishes. Send 8
    // requests and they complete in two batches: ~400ms total.
    let route_limited = RouteBuilder::from("http://0.0.0.0:8080/limited")
        .route_id("concurrent-bounded")
        .concurrent(4)
        .process(|mut exchange| async move {
            let started = now_ms();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let finished = now_ms();
            exchange.input.body = Body::Json(serde_json::json!({
                "path": "/limited",
                "mode": "concurrent(4) — max 4 in-flight at a time",
                "started_at_ms": started,
                "finished_at_ms": finished,
            }));
            Ok(exchange)
        })
        .error_handler(
            ErrorHandlerConfig::log_only()
                .on_exception(|_| true)
                .retry(1)
                .build(),
        )
        .build()?;

    // Route 3: /sequential — .sequential() override.
    //
    // Overrides the HttpConsumer's default Concurrent model. The runtime
    // processes one exchange at a time — request N+1 waits until request N
    // exits the pipeline. Correct choice when shared mutable state requires
    // strict ordering. Send 8 requests and they complete one-by-one: ~1600ms.
    let route_sequential = RouteBuilder::from("http://0.0.0.0:8080/sequential")
        .route_id("sequential")
        .sequential()
        .process(|mut exchange| async move {
            let started = now_ms();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let finished = now_ms();
            exchange.input.body = Body::Json(serde_json::json!({
                "path": "/sequential",
                "mode": "sequential() override — one request at a time",
                "started_at_ms": started,
                "finished_at_ms": finished,
            }));
            Ok(exchange)
        })
        .error_handler(
            ErrorHandlerConfig::log_only()
                .on_exception(|_| true)
                .retry(1)
                .build(),
        )
        .build()?;

    ctx.add_route_definition(route_slow).await?;
    ctx.add_route_definition(route_limited).await?;
    ctx.add_route_definition(route_sequential).await?;

    ctx.start().await?;

    print_banner();

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down...");
    ctx.stop().await?;
    println!("Done.");

    Ok(())
}

fn print_banner() {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║        rust-camel — Pipeline Concurrency Example             ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();
    println!("Three HTTP routes on port 8080, each with 200ms artificial latency.");
    println!("Compare total wall-clock time to see the difference:");
    println!();
    println!("  GET /slow       — auto-concurrent (HttpConsumer default, no override)");
    println!("  GET /limited    — .concurrent(4)  (semaphore: max 4 in-flight)");
    println!("  GET /sequential — .sequential()   (one request at a time)");
    println!();
    println!("Send 8 parallel requests to each route:");
    println!();
    println!("  # /slow: all 8 finish in ~200ms");
    println!("  for i in $(seq 1 8); do curl -s localhost:8080/slow & done; wait");
    println!();
    println!("  # /limited: 8 requests finish in ~400ms (two batches of 4)");
    println!("  for i in $(seq 1 8); do curl -s localhost:8080/limited & done; wait");
    println!();
    println!("  # /sequential: 8 requests finish in ~1600ms (one at a time)");
    println!("  for i in $(seq 1 8); do curl -s localhost:8080/sequential & done; wait");
    println!();
    println!("Watch started_at_ms / finished_at_ms in each JSON response:");
    println!("  Overlapping timestamps     → concurrent execution");
    println!("  Non-overlapping timestamps → sequential execution");
    println!();
    println!("Press Ctrl+C to stop...");
}
