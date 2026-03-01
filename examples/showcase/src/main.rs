use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use camel_api::body::Body;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::splitter::{AggregationStrategy, SplitterConfig, split_body_json_array};
use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_core::context::CamelContext;
use camel_direct::DirectComponent;
use camel_file::FileComponent;
use camel_http::{HttpComponent, HttpsComponent};
use camel_log::LogComponent;
use camel_timer::TimerComponent;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt::init();

    let mut ctx = CamelContext::new();

    let direct = DirectComponent::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(direct);
    ctx.register_component(FileComponent::new());
    ctx.register_component(HttpComponent::new());
    ctx.register_component(HttpsComponent::new());

    ctx.set_error_handler(ErrorHandlerConfig::dead_letter_channel(
        "log:global-dlc?showHeaders=true&showBody=true",
    ));

    // Route 1: Timer -> process (alternate body) -> set header -> dispatch
    let counter = Arc::new(AtomicU64::new(0));
    let counter_clone = Arc::clone(&counter);

    let route1 = RouteBuilder::from("timer:events?period=1000&repeatCount=8")
        .process(move |mut exchange| {
            let c = Arc::clone(&counter_clone);
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n.is_multiple_of(2) {
                    exchange.input.body = Body::Text("typeA".into());
                } else {
                    exchange.input.body = Body::Text("typeB".into());
                }
                Ok(exchange)
            }
        })
        .set_header("source", Value::String("timer".into()))
        .to("direct:dispatcher")
        .build()?;

    // Route 2: Receive -> filter typeA only -> uppercase -> mark processed -> log
    let route2 = RouteBuilder::from("direct:dispatcher")
        .filter(|ex| ex.input.body.as_text() == Some("typeA"))
        .map_body(|body| {
            if let Some(text) = body.as_text() {
                Body::Text(text.to_uppercase())
            } else {
                body
            }
        })
        .set_header("processed", Value::Bool(true))
        .to("log:output?showHeaders=true&showBody=true")
        .end_filter()
        .error_handler(
            ErrorHandlerConfig::dead_letter_channel(
                "log:route2-dlc?showHeaders=true&showBody=true",
            )
            .on_exception(|e| matches!(e, CamelError::ProcessorError(_)))
            .retry(2)
            .with_backoff(Duration::from_millis(100), 2.0, Duration::from_secs(1))
            .build(),
        )
        .build()?;

    // Route 3: Timer -> set JSON array body -> parallel split -> enrich each -> log aggregated
    let route3 = RouteBuilder::from("timer:orders?period=3000&repeatCount=3")
        .process(|mut exchange| async move {
            exchange.input.body = Body::Json(serde_json::json!([
                {"id": 1, "item": "widget", "qty": 5},
                {"id": 2, "item": "gadget", "qty": 2},
                {"id": 3, "item": "gizmo",  "qty": 10},
            ]));
            Ok(exchange)
        })
        .split(
            SplitterConfig::new(split_body_json_array())
                .aggregation(AggregationStrategy::CollectAll)
                .parallel(true)
                .parallel_limit(2),
        )
        .map_body(|body| {
            if let Body::Json(mut v) = body {
                v["processed"] = serde_json::json!(true);
                Body::Json(v)
            } else {
                body
            }
        })
        .to("log:order-fragment?showBody=true")
        .end_split()
        .to("log:orders-aggregated?showBody=true")
        .build()?;

    // Route 4 (File): Timer -> append events to file
    let output_dir = std::env::temp_dir().join("rust-camel-showcase");
    std::fs::create_dir_all(&output_dir).ok();
    let file_path = output_dir.to_str().unwrap();

    let route4 = RouteBuilder::from("timer:file-writer?period=2000&repeatCount=5")
        .process(|mut exchange| async move {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            exchange.input.body = Body::Text(format!("[{timestamp}] Event logged\n"));
            Ok(exchange)
        })
        .set_header("CamelFileName", Value::String("events.log".into()))
        .to(format!("file:{file_path}?fileExist=Append"))
        .to("log:file-written?showBody=true")
        .build()?;

    // Route 5 (HTTP): Timer -> HTTP GET -> log response
    let route5 = RouteBuilder::from("timer:http-poll?period=5000&repeatCount=3")
        .to("https://httpbin.org/get?source=rust-camel")
        .to("log:http-response?showHeaders=true&showBody=true")
        .build()?;

    // Route 6 (WireTap): Timer -> wire_tap (monitoring) -> log (main)
    // Demonstrates fire-and-forget: tap receives a copy while original continues
    let route6 = RouteBuilder::from("timer:monitor?period=4000&repeatCount=3")
        .process(|mut exchange| async move {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            exchange.input.body = Body::Text(format!("[{timestamp}] Monitoring event"));
            Ok(exchange)
        })
        .wire_tap("log:tap-monitor?showBody=true")
        .to("log:tap-main?showBody=true")
        .build()?;

    // Route 7 (Filter): Timer -> filter "important" -> log inside -> log after (all)
    // Demonstrates Apache Camel Filter semantics:
    //   - log:filter-inside  → only "important" messages (inside filter scope)
    //   - log:filter-after   → ALL messages (filter does NOT stop the outer pipeline)
    let filter_counter = Arc::new(AtomicU64::new(0));
    let filter_counter_clone = Arc::clone(&filter_counter);

    let route7 = RouteBuilder::from("timer:filter-demo?period=2000&repeatCount=6")
        .process(move |mut exchange| {
            let n = filter_counter_clone.fetch_add(1, Ordering::SeqCst);
            let msg_type = if n.is_multiple_of(2) { "important" } else { "normal" };
            exchange.input.body = Body::Text(format!("[{msg_type}] message"));
            exchange.input.set_header("type", Value::String(msg_type.to_string()));
            async move { Ok(exchange) }
        })
        .filter(|ex| ex.input.header("type") == Some(&Value::String("important".into())))
        .to("log:filter-inside?showBody=true&showHeaders=true")
        .end_filter()
        .to("log:filter-after?showBody=true&showHeaders=true") // all messages
        .build()?;

    ctx.add_route_definition(route1)?;
    ctx.add_route_definition(route2)?;
    ctx.add_route_definition(route3)?;
    ctx.add_route_definition(route4)?;
    ctx.add_route_definition(route5)?;
    ctx.add_route_definition(route6)?;
    ctx.add_route_definition(route7)?;
    ctx.start().await?;

    println!("Showcase running. Routes:");
    println!("  - timer:events (1s)       -> direct:dispatcher -> filter(typeA) -> log");
    println!("  - timer:orders (3s)       -> split -> log");
    println!("  - timer:file-writer (2s)  -> file:{file_path}/events.log");
    println!("  - timer:http-poll (5s)    -> https:httpbin.org/get -> log");
    println!("  - timer:monitor (4s)      -> wire_tap -> log");
    println!("  - timer:filter-demo (2s)  -> filter(important) -> log inside + log after(all)");
    println!();
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("Shutting down gracefully (30s timeout)...");
    ctx.stop().await?;
    println!("Shutdown complete.");

    Ok(())
}
