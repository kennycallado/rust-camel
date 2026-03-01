use camel_api::CamelError;
use camel_api::body::Body;
use camel_api::splitter::{AggregationStrategy, SplitterConfig, split_body_lines};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_core::context::CamelContext;
use camel_log::LogComponent;
use camel_timer::TimerComponent;

/// Splitter EIP example — batch CSV processor.
///
/// A timer fires periodically with a CSV-like body. The splitter breaks it
/// into one exchange per line, each fragment is transformed into JSON, and
/// the aggregated result (CollectAll) is logged at the end.
///
/// Flow:
///   timer -> set body (CSV) -> split by lines
///       -> transform line to JSON -> log per-fragment
///   -> log aggregated result
#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt::init();

    let mut ctx = CamelContext::new();

    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from("timer:batch?period=2000&repeatCount=3")
        // Simulate incoming CSV data
        .process(|mut exchange: camel_api::Exchange| {
            Box::pin(async move {
                exchange.input.body = Body::Text("alice,100\nbob,200\ncharlie,300".to_string());
                Ok(exchange)
            })
        })
        // Split by lines, aggregate all fragment bodies into a JSON array
        .split(SplitterConfig::new(split_body_lines()).aggregation(AggregationStrategy::CollectAll))
        // Transform each CSV line into a JSON object
        .map_body(|body: Body| {
            let text = body.as_text().unwrap_or("");
            let parts: Vec<&str> = text.splitn(2, ',').collect();
            let (name, amount) = match parts.as_slice() {
                [n, a] => (*n, a.parse::<u64>().unwrap_or(0)),
                _ => (text, 0),
            };
            Body::Json(serde_json::json!({
                "name": name,
                "amount": amount,
            }))
        })
        .to("log:fragment?showBody=true")
        .end_split()
        // After split: aggregated JSON array of all fragments
        .to("log:aggregated?showBody=true")
        .build()?;

    ctx.add_route_definition(route)?;
    ctx.start().await?;

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("Shutting down...");
    ctx.stop().await?;
    println!("Done.");

    Ok(())
}
