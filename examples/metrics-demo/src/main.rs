use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use camel_api::CamelError;
use camel_api::body::Body;
use camel_api::metrics::MetricsCollector;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

/// A simple console-based metrics collector that prints stats on shutdown.
#[derive(Debug)]
struct ConsoleMetrics {
    exchanges_processed: AtomicU64,
    errors: AtomicU64,
    total_duration_ns: AtomicU64,
}

impl ConsoleMetrics {
    fn new() -> Self {
        Self {
            exchanges_processed: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            total_duration_ns: AtomicU64::new(0),
        }
    }

    fn print_stats(&self) {
        let processed = self.exchanges_processed.load(Ordering::Relaxed);
        let errors = self.errors.load(Ordering::Relaxed);
        let total_ns = self.total_duration_ns.load(Ordering::Relaxed);
        let avg_ms = if processed > 0 {
            (total_ns / processed) as f64 / 1_000_000.0
        } else {
            0.0
        };

        println!(
            "[METRICS] exchanges={} errors={} avg_duration_ms={:.2}",
            processed, errors, avg_ms
        );
    }
}

impl MetricsCollector for ConsoleMetrics {
    fn record_exchange_duration(&self, _route_id: &str, duration: Duration) {
        self.exchanges_processed.fetch_add(1, Ordering::Relaxed);
        self.total_duration_ns
            .fetch_add(duration.as_nanos() as u64, Ordering::Relaxed);
    }

    fn increment_errors(&self, _route_id: &str, _error_type: &str) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    fn increment_exchanges(&self, _route_id: &str) {
        self.exchanges_processed.fetch_add(1, Ordering::Relaxed);
    }

    fn set_queue_depth(&self, _route_id: &str, _depth: usize) {
        // Not tracked in this simple example
    }

    fn record_circuit_breaker_change(&self, _route_id: &str, from: &str, to: &str) {
        println!("[METRICS] Circuit breaker: {} -> {}", from, to);
    }
}

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let metrics = Arc::new(ConsoleMetrics::new());
    let metrics_for_print = Arc::clone(&metrics);

    // Create context with custom metrics collector
    let mut ctx = CamelContext::builder().metrics(metrics).build().await?;

    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from("timer:metrics?period=500&repeatCount=10")
        .route_id("metrics-demo")
        .process(|mut exchange| {
            let start = Instant::now();
            async move {
                // Simulate some work
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

                exchange.input.body = Body::Json(serde_json::json!({
                    "message": "processed",
                    "elapsed_us": start.elapsed().as_micros(),
                }));
                Ok(exchange)
            }
        })
        .to("log:output?showBody=true&showCorrelationId=true")
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("Metrics demo running.");
    println!("Custom ConsoleMetrics collecting stats...");
    println!("Press Ctrl+C to stop and see final metrics.");

    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;

    metrics_for_print.print_stats();
    println!("Done.");

    Ok(())
}
