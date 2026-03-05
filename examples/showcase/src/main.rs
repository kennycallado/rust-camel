//! # rust-camel Comprehensive Showcase
//!
//! This showcase demonstrates ALL features of rust-camel in one executable:
//!
//! ## EIPs (Enterprise Integration Patterns)
//! - Filter, Splitter, Aggregator, WireTap, Multicast
//! - Content-Based Router, Circuit Breaker, Stop
//!
//! ## Components
//! - timer, log, direct, mock, file, http, https, controlbus
//!
//! ## Features
//! - Correlation IDs, Error Handlers, Timeouts
//! - Sequential Processing, Metrics

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use camel_api::aggregator::AggregatorConfig;
use camel_api::body::Body;
use camel_api::circuit_breaker::CircuitBreakerConfig;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::metrics::MetricsCollector;
use camel_api::multicast::MulticastStrategy;
use camel_api::splitter::{AggregationStrategy, SplitterConfig, split_body_json_array};
use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_controlbus::ControlBusComponent;
use camel_component_direct::DirectComponent;
use camel_component_file::FileComponent;
use camel_component_http::{HttpComponent, HttpsComponent};
use camel_component_log::LogComponent;
use camel_component_mock::MockComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use tracing_subscriber::Layer;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use camel_processor::LogLevel;

// =============================================================================
// Custom Metrics Collector
// =============================================================================

#[derive(Debug)]
struct ShowcaseMetrics {
    exchanges_processed: AtomicU64,
    errors: AtomicU64,
    total_duration_ns: AtomicU64,
}

impl ShowcaseMetrics {
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

impl MetricsCollector for ShowcaseMetrics {
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

    fn set_queue_depth(&self, _route_id: &str, _depth: usize) {}

    fn record_circuit_breaker_change(&self, _route_id: &str, from: &str, to: &str) {
        println!("[METRICS] Circuit breaker: {} -> {}", from, to);
    }
}

// =============================================================================
// Main
// =============================================================================

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    // General layer: all non-tracer logs → stdout
    let general_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_filter(tracing_subscriber::filter::filter_fn(|meta| {
            meta.target() != "camel_tracer"
        }))
        .boxed();

    // Tracer layer: camel_tracer spans → showcase-trace.log (JSON)
    let trace_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("showcase-trace.log")
        .expect("failed to open showcase-trace.log");
    let tracer_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_span_events(FmtSpan::CLOSE)
        .with_writer(std::sync::Mutex::new(trace_file))
        .with_target(true)
        .with_filter(tracing_subscriber::filter::filter_fn(|meta| {
            meta.target() == "camel_tracer"
        }))
        .boxed();

    tracing_subscriber::registry()
        .with(general_layer)
        .with(tracer_layer)
        .init();

    // Setup metrics
    let metrics = Arc::new(ShowcaseMetrics::new());
    let metrics_for_print = Arc::clone(&metrics);

    // Create context with metrics and enable tracing
    let mut ctx = CamelContext::with_metrics(metrics);
    ctx.set_tracing(true); // spans for all 22 routes → showcase-trace.log

    // Register all components
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(DirectComponent::new());
    ctx.register_component(MockComponent::new());
    ctx.register_component(FileComponent::new());
    ctx.register_component(HttpComponent::new());
    ctx.register_component(HttpsComponent::new());
    ctx.register_component(ControlBusComponent::new());

    // Global error handler (fallback for routes without their own)
    ctx.set_error_handler(ErrorHandlerConfig::dead_letter_channel(
        "log:global-dlc?showHeaders=true&showBody=true&showCorrelationId=true",
    ));

    // Setup temp directory for file routes
    let output_dir = std::env::temp_dir().join("rust-camel-showcase");
    std::fs::create_dir_all(&output_dir).ok();
    let file_path = output_dir.to_str().unwrap();

    // =========================================================================
    // CATEGORY 1: EIP BASICS
    // =========================================================================

    println!("Building Category 1: EIP Basics...");

    // --- Route 1: Filter EIP ---
    let filter_counter = Arc::new(AtomicU64::new(0));
    let filter_counter_clone = Arc::clone(&filter_counter);
    let route1 = RouteBuilder::from("timer:eip-filter?period=2000&repeatCount=4")
        .route_id("eip-filter")
        .process(move |mut exchange| {
            let c = Arc::clone(&filter_counter_clone);
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                let msg_type = if n.is_multiple_of(2) {
                    "important"
                } else {
                    "normal"
                };
                exchange.input.body = Body::Text(format!("[{msg_type}] message #{n}"));
                exchange
                    .input
                    .set_header("type", Value::String(msg_type.to_string()));
                Ok(exchange)
            }
        })
        .filter(|ex| ex.input.header("type") == Some(&Value::String("important".into())))
        .to("log:eip-filter-inside?showBody=true&showHeaders=true&showCorrelationId=true")
        .end_filter()
        .to("log:eip-filter-after?showBody=true&showCorrelationId=true")
        .build()?;

    // --- Route 2: Splitter EIP ---
    let route2 = RouteBuilder::from("timer:eip-splitter?period=3000&repeatCount=3")
        .route_id("eip-splitter")
        .process(|mut exchange| async move {
            exchange.input.body = Body::Json(serde_json::json!([
                {"id": 1, "item": "widget", "qty": 5},
                {"id": 2, "item": "gadget", "qty": 2},
                {"id": 3, "item": "gizmo", "qty": 10},
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
        .to("log:eip-split-fragment?showBody=true&showCorrelationId=true")
        .end_split()
        .to("log:eip-split-aggregated?showBody=true&showCorrelationId=true")
        .build()?;

    // --- Route 3: Aggregator EIP ---
    let agg_counter = Arc::new(AtomicU64::new(0));
    let agg_counter_clone = Arc::clone(&agg_counter);
    let route3 = RouteBuilder::from("timer:eip-aggregator?period=500&repeatCount=9")
        .route_id("eip-aggregator")
        .process(move |mut exchange| {
            let n = agg_counter_clone.fetch_add(1, Ordering::SeqCst);
            let order_id = ["A", "B", "C"][(n % 3) as usize];
            async move {
                exchange
                    .input
                    .set_header("orderId", Value::String(order_id.to_string()));
                exchange.input.body = Body::Text(format!("item-{n}"));
                Ok(exchange)
            }
        })
        .aggregate(
            AggregatorConfig::correlate_by("orderId")
                .complete_when_size(3)
                .max_buckets(100)
                .bucket_ttl(Duration::from_secs(60))
                .build(),
        )
        .process(|exchange| async move {
            if exchange.property("CamelAggregatorPending").is_none() {
                let key = exchange
                    .property("CamelAggregatedKey")
                    .map(|v: &serde_json::Value| v.to_string())
                    .unwrap_or_default();
                println!("[AGGREGATOR] Completed batch: orderId={}", key);
            }
            Ok(exchange)
        })
        .to("log:eip-aggregator-batch?showBody=true&showCorrelationId=true")
        .build()?;

    // --- Route 4: WireTap EIP ---
    let route4 = RouteBuilder::from("timer:eip-wiretap?period=3000&repeatCount=3")
        .route_id("eip-wiretap")
        .process(|mut exchange| async move {
            exchange.input.body = Body::Text("Main flow message".to_string());
            Ok(exchange)
        })
        .wire_tap("log:eip-wiretap-tap?showBody=true&showCorrelationId=true")
        .to("log:eip-wiretap-main?showBody=true&showCorrelationId=true")
        .build()?;

    // --- Route 5: Multicast EIP ---
    let route5 = RouteBuilder::from("timer:eip-multicast?period=4000&repeatCount=2")
        .route_id("eip-multicast")
        .process(|mut exchange| async move {
            exchange.input.body = Body::Text("broadcast".to_string());
            Ok(exchange)
        })
        .multicast()
        .parallel(true)
        .timeout(Duration::from_secs(5))
        .aggregation(MulticastStrategy::CollectAll)
        .to("log:eip-mc-a?showBody=true&showCorrelationId=true")
        .to("log:eip-mc-b?showBody=true&showCorrelationId=true")
        .to("log:eip-mc-c?showBody=true&showCorrelationId=true")
        .end_multicast()
        .to("log:eip-mc-summary?showBody=true&showCorrelationId=true")
        .build()?;

    // =========================================================================
    // CATEGORY 2: ROUTING
    // =========================================================================

    println!("Building Category 2: Routing...");

    // --- Route 6: Content-Based Router ---
    let cbr_counter = Arc::new(AtomicU64::new(0));
    let cbr_counter_clone = Arc::clone(&cbr_counter);
    let route6 = RouteBuilder::from("timer:eip-cbr?period=2000&repeatCount=6")
        .route_id("eip-content-based-router")
        .process(move |mut exchange| {
            let n = cbr_counter_clone.fetch_add(1, Ordering::SeqCst);
            let region = ["US", "EU", "APAC"][(n % 3) as usize];
            async move {
                exchange
                    .input
                    .set_header("region", Value::String(region.to_string()));
                exchange.input.body = Body::Text(format!("order-{n} for {region}"));
                Ok(exchange)
            }
        })
        .to("direct:region-router")
        .build()?;

    // --- Subroute: Region Router (Content-Based) ---
    let route6b = RouteBuilder::from("direct:region-router")
        .route_id("sub-region-router")
        .filter(|ex| ex.input.header("region") == Some(&Value::String("US".into())))
        .to("log:cbr-us?showBody=true&showCorrelationId=true")
        .to("mock:us-orders")
        .end_filter()
        .filter(|ex| ex.input.header("region") == Some(&Value::String("EU".into())))
        .to("log:cbr-eu?showBody=true&showCorrelationId=true")
        .to("mock:eu-orders")
        .end_filter()
        .filter(|ex| ex.input.header("region") == Some(&Value::String("APAC".into())))
        .to("log:cbr-apac?showBody=true&showCorrelationId=true")
        .to("mock:apac-orders")
        .end_filter()
        .build()?;

    // --- Route 7: Stop EIP ---
    let stop_counter = Arc::new(AtomicU64::new(0));
    let stop_counter_clone = Arc::clone(&stop_counter);
    let route7 = RouteBuilder::from("timer:eip-stop?period=2000&repeatCount=4")
        .route_id("eip-stop")
        .process(move |mut exchange| {
            let n = stop_counter_clone.fetch_add(1, Ordering::SeqCst);
            let label = if n.is_multiple_of(2) {
                "halt-me"
            } else {
                "pass-me"
            };
            async move {
                exchange.input.body = Body::Text(label.to_string());
                Ok(exchange)
            }
        })
        .filter(|ex| ex.input.body.as_text() == Some("halt-me"))
        .to("log:eip-stop-inside?showBody=true&showCorrelationId=true")
        .stop()
        .end_filter()
        .to("log:eip-stop-after?showBody=true&showCorrelationId=true")
        .build()?;

    // --- Route 8: Recipient List (dynamic routing via filters) ---
    let dyn_counter = Arc::new(AtomicU64::new(0));
    let dyn_counter_clone = Arc::clone(&dyn_counter);
    let route8 = RouteBuilder::from("timer:eip-recipient?period=3000&repeatCount=3")
        .route_id("eip-recipient-list")
        .process(move |mut exchange| {
            let n = dyn_counter_clone.fetch_add(1, Ordering::SeqCst);
            let dest = if n.is_multiple_of(2) { "alpha" } else { "beta" };
            async move {
                exchange
                    .input
                    .set_header("destination", Value::String(dest.to_string()));
                exchange.input.body = Body::Text(format!("dynamic-{n}"));
                println!("[RECIPIENT] Routing to: {}", dest);
                Ok(exchange)
            }
        })
        // Content-based recipient selection using filters
        .filter(|ex| ex.input.header("destination") == Some(&Value::String("alpha".into())))
        .to("direct:alpha")
        .end_filter()
        .filter(|ex| ex.input.header("destination") == Some(&Value::String("beta".into())))
        .to("direct:beta")
        .end_filter()
        .to("log:eip-recipient-done?showBody=true&showCorrelationId=true")
        .build()?;

    let route8a = RouteBuilder::from("direct:alpha")
        .route_id("sub-recipient-alpha")
        .process(|mut exchange| async move {
            exchange.input.body = Body::Text(format!(
                "alpha: {}",
                exchange.input.body.as_text().unwrap_or("")
            ));
            Ok(exchange)
        })
        .to("log:eip-recipient-alpha?showBody=true&showCorrelationId=true")
        .build()?;

    let route8b = RouteBuilder::from("direct:beta")
        .route_id("sub-recipient-beta")
        .process(|mut exchange| async move {
            exchange.input.body = Body::Text(format!(
                "beta: {}",
                exchange.input.body.as_text().unwrap_or("")
            ));
            Ok(exchange)
        })
        .to("log:eip-recipient-beta?showBody=true&showCorrelationId=true")
        .build()?;

    // =========================================================================
    // CATEGORY 3: RESILIENCE
    // =========================================================================

    println!("Building Category 3: Resilience...");

    // --- Route 9: Circuit Breaker ---
    // Note: Demonstrates circuit breaker pattern by calling a failing endpoint
    // then recovering. Uses static URL since header: endpoint is not available.
    let route9 = RouteBuilder::from("timer:resilience-cb?period=1000&repeatCount=3")
        .route_id("resilience-circuit-breaker")
        .process(|mut exchange| async move {
            exchange.input.body = Body::Text("circuit breaker test".to_string());
            println!("[CB] Testing circuit breaker with HTTP call...");
            Ok(exchange)
        })
        .to("https://httpbin.org/get?source=rust-camel-showcase&connectTimeout=3000&responseTimeout=5000")
        .to("log:resilience-cb-result?showBody=true&showCorrelationId=true")
        .circuit_breaker(
            CircuitBreakerConfig::new()
                .failure_threshold(2)
                .open_duration(Duration::from_secs(2)),
        )
        .error_handler(ErrorHandlerConfig::dead_letter_channel(
            "log:resilience-cb-dlc?showBody=true&showCorrelationId=true",
        ))
        .build()?;

    // --- Route 10: Error Handler with Retry ---
    let retry_counter = Arc::new(AtomicU32::new(0));
    let retry_counter_clone = Arc::clone(&retry_counter);
    let route10 = RouteBuilder::from("timer:resilience-retry?period=5000&repeatCount=2")
        .route_id("resilience-retry")
        .process(move |mut exchange| {
            let c = Arc::clone(&retry_counter_clone);
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    println!("[RETRY] Attempt {} - will fail", n + 1);
                    return Err(CamelError::ProcessorError("transient error".into()));
                }
                println!("[RETRY] Attempt {} - success!", n + 1);
                exchange.input.body = Body::Text("recovered after retry".to_string());
                Ok(exchange)
            }
        })
        .to("log:resilience-retry-success?showBody=true&showCorrelationId=true")
        .error_handler(
            ErrorHandlerConfig::dead_letter_channel(
                "log:resilience-retry-dlc?showCorrelationId=true",
            )
            .on_exception(|_| true)
            .retry(3)
            .with_backoff(Duration::from_millis(100), 2.0, Duration::from_secs(1))
            .build(),
        )
        .build()?;

    // --- Route 11: Dead Letter Channel ---
    let route11 = RouteBuilder::from("timer:resilience-dlc?period=5000&repeatCount=2")
        .route_id("resilience-dlc")
        .set_header("example", Value::String("dead-letter-channel".into()))
        .process(|_exchange| async move {
            Err(CamelError::ProcessorError(
                "permanent failure - goes to DLC".into(),
            ))
        })
        .error_handler(ErrorHandlerConfig::dead_letter_channel(
            "log:resilience-dlc-sink?showHeaders=true&showBody=true&showCorrelationId=true",
        ))
        .build()?;

    // =========================================================================
    // CATEGORY 4: COMPONENTS
    // =========================================================================

    println!("Building Category 4: Components...");

    // --- Route 12: Timer + Log (basic) ---
    let route12 = RouteBuilder::from("timer:comp-timer?period=3000&repeatCount=3")
        .route_id("comp-timer-log")
        .log("Timer fired - processing exchange", LogLevel::Info)
        .process(|mut exchange| async move {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            exchange.input.body = Body::Json(serde_json::json!({
                "timestamp": ts,
                "message": "timer event"
            }));
            Ok(exchange)
        })
        .to("log:comp-timer-output?showBody=true&showCorrelationId=true")
        .build()?;

    // --- Route 13: Direct (route linking) ---
    let route13 = RouteBuilder::from("timer:comp-direct?period=4000&repeatCount=2")
        .route_id("comp-direct-caller")
        .process(|mut exchange| async move {
            exchange.input.body = Body::Text("calling subroute".to_string());
            Ok(exchange)
        })
        .to("direct:sub-service")
        .to("log:comp-direct-result?showBody=true&showCorrelationId=true")
        .build()?;

    let route13b = RouteBuilder::from("direct:sub-service")
        .route_id("comp-direct-subroute")
        .process(|mut exchange| async move {
            let orig = exchange.input.body.as_text().unwrap_or("");
            exchange.input.body = Body::Text(format!("{} -> processed by subroute", orig));
            Ok(exchange)
        })
        .build()?;

    // --- Route 14: File (write) ---
    let route14 = RouteBuilder::from("timer:comp-file?period=4000&repeatCount=2")
        .route_id("comp-file-write")
        .process(|mut exchange| async move {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            exchange.input.body = Body::Text(format!("[{ts}] Showcase event logged\n"));
            Ok(exchange)
        })
        .set_header("CamelFileName", Value::String("showcase.log".into()))
        .to(format!("file:{file_path}?fileExist=Append"))
        .to("log:comp-file-written?showBody=true&showCorrelationId=true")
        .build()?;

    // --- Route 15: HTTP Client ---
    let route15 = RouteBuilder::from("timer:comp-http?period=8000&repeatCount=2")
        .route_id("comp-http-client")
        .log("Making HTTP request", LogLevel::Info)
        .to("https://httpbin.org/get?source=rust-camel-showcase&connectTimeout=5000&responseTimeout=10000")
        .to("log:comp-http-response?showHeaders=true&showBody=true&showCorrelationId=true")
        .build()?;

    // --- Route 16: Mock Component ---
    let route16 = RouteBuilder::from("timer:comp-mock?period=3000&repeatCount=3")
        .route_id("comp-mock")
        .process(|mut exchange| async move {
            exchange.input.body = Body::Json(serde_json::json!({"test": "data"}));
            Ok(exchange)
        })
        .to("mock:showcase-result")
        .to("log:comp-mock-sent?showBody=true&showCorrelationId=true")
        .build()?;

    // --- Route 17: ControlBus ---
    let route17_target = RouteBuilder::from("timer:comp-cbus-target?period=500")
        .route_id("comp-cbus-target")
        .auto_startup(true)
        .process(|mut exchange| async move {
            exchange.input.body = Body::Text("ControlBus target running".to_string());
            Ok(exchange)
        })
        .to("log:comp-cbus-target-log?showBody=true&showCorrelationId=true")
        .build()?;

    let route17_suspend = RouteBuilder::from("timer:comp-cbus-suspend?delay=3000&repeatCount=1")
        .route_id("comp-cbus-suspend")
        .process(|mut exchange| async move {
            println!("[CONTROLBUS] Suspending target route...");
            exchange
                .input
                .set_header("CamelRouteId", Value::String("comp-cbus-target".into()));
            Ok(exchange)
        })
        .to("controlbus:route?action=suspend")
        .to("log:comp-cbus-suspend-done?showBody=true")
        .build()?;

    let route17_resume = RouteBuilder::from("timer:comp-cbus-resume?delay=6000&repeatCount=1")
        .route_id("comp-cbus-resume")
        .process(|mut exchange| async move {
            println!("[CONTROLBUS] Resuming target route...");
            exchange
                .input
                .set_header("CamelRouteId", Value::String("comp-cbus-target".into()));
            Ok(exchange)
        })
        .to("controlbus:route?action=resume")
        .to("log:comp-cbus-resume-done?showBody=true")
        .build()?;

    // =========================================================================
    // CATEGORY 5: ADVANCED
    // =========================================================================

    println!("Building Category 5: Advanced...");

    // --- Route 18: Sequential Processing ---
    let seq_counter = Arc::new(std::sync::Mutex::new(0u64));
    let seq_counter_clone = Arc::clone(&seq_counter);
    let route18 = RouteBuilder::from("http://0.0.0.0:8081/showcase/counter")
        .route_id("adv-sequential")
        .sequential()
        .process(move |mut exchange| {
            let counter = Arc::clone(&seq_counter_clone);
            async move {
                let mut n = counter.lock().unwrap();
                *n += 1;
                exchange.input.body = Body::Json(serde_json::json!({
                    "count": *n,
                    "correlation_id": exchange.correlation_id
                }));
                Ok(exchange)
            }
        })
        .build()?;

    // --- Route 19: Correlation ID Tracing ---
    let corr_counter = Arc::new(AtomicU64::new(0));
    let corr_counter_clone = Arc::clone(&corr_counter);
    let route19 = RouteBuilder::from("timer:adv-correlation?period=3000&repeatCount=3")
        .route_id("adv-correlation-ids")
        .process(move |mut exchange| {
            let n = corr_counter_clone.fetch_add(1, Ordering::SeqCst);
            async move {
                exchange.input.body = Body::Text(format!("trace-{n}"));
                exchange
                    .input
                    .set_header("X-Trace-Id", Value::String(format!("trace-{n}")));
                println!(
                    "[CORRELATION] Starting trace: {} (cid={})",
                    n, exchange.correlation_id
                );
                Ok(exchange)
            }
        })
        .to("direct:trace-step-1")
        .to("direct:trace-step-2")
        .to("log:adv-correlation-done?showBody=true&showHeaders=true&showCorrelationId=true")
        .build()?;

    let route19a = RouteBuilder::from("direct:trace-step-1")
        .route_id("sub-trace-step-1")
        .process(|exchange| async move {
            println!("[CORRELATION] Step 1 - cid={}", exchange.correlation_id);
            Ok(exchange)
        })
        .build()?;

    let route19b = RouteBuilder::from("direct:trace-step-2")
        .route_id("sub-trace-step-2")
        .process(|exchange| async move {
            println!("[CORRELATION] Step 2 - cid={}", exchange.correlation_id);
            Ok(exchange)
        })
        .build()?;

    // --- Route 20: Metrics Collection ---
    let route20 = RouteBuilder::from("timer:adv-metrics?period=1000&repeatCount=5")
        .route_id("adv-metrics")
        .process(|mut exchange| {
            let start = Instant::now();
            async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                exchange.input.body = Body::Json(serde_json::json!({
                    "message": "metrics demo",
                    "elapsed_us": start.elapsed().as_micros(),
                }));
                Ok(exchange)
            }
        })
        .to("log:adv-metrics-output?showBody=true&showCorrelationId=true")
        .build()?;

    // --- Route 21: Timeout Handling ---
    let route21 = RouteBuilder::from("timer:adv-timeout?period=5000&repeatCount=2")
        .route_id("adv-timeout")
        .process(|mut exchange| async move {
            exchange.input.body = Body::Text("timeout test".to_string());
            Ok(exchange)
        })
        .multicast()
        .timeout(Duration::from_millis(100))
        .aggregation(MulticastStrategy::CollectAll)
        .to("direct:slow-endpoint")
        .end_multicast()
        .to("log:adv-timeout-result?showBody=true&showCorrelationId=true")
        .error_handler(ErrorHandlerConfig::dead_letter_channel(
            "log:adv-timeout-dlc?showBody=true&showCorrelationId=true",
        ))
        .build()?;

    let route21b = RouteBuilder::from("direct:slow-endpoint")
        .route_id("sub-slow-endpoint")
        .process(|exchange| async move {
            println!("[TIMEOUT] Slow endpoint starting (will exceed timeout)...");
            tokio::time::sleep(Duration::from_millis(500)).await;
            println!("[TIMEOUT] Slow endpoint done (too late!)");
            Ok(exchange)
        })
        .build()?;

    // --- Route 22: Log EIP (inline logging) ---
    let route22 = RouteBuilder::from("timer:adv-log-eip?period=4000&repeatCount=2")
        .route_id("adv-log-eip")
        .log("=== Starting Log EIP Demo ===", LogLevel::Warn)
        .process(|mut exchange| async move {
            exchange.input.body = Body::Json(serde_json::json!({
                "order_id": 12345,
                "items": ["widget", "gadget"],
                "total": 99.99
            }));
            exchange.input.set_header("source", Value::String("timer".into()));
            Ok(exchange)
        })
        .log("Order data prepared", LogLevel::Info)
        .to("log:adv-log-eip-full?showBody=true&showHeaders=true&multiline=true&showCorrelationId=true")
        .log("=== Processing complete ===", LogLevel::Info)
        .to("mock:log-eip-result")
        .build()?;

    // =========================================================================
    // Register All Routes
    // =========================================================================

    println!("Registering routes...");

    // Category 1: EIP Basics
    ctx.add_route_definition(route1)?;
    ctx.add_route_definition(route2)?;
    ctx.add_route_definition(route3)?;
    ctx.add_route_definition(route4)?;
    ctx.add_route_definition(route5)?;

    // Category 2: Routing
    ctx.add_route_definition(route6)?;
    ctx.add_route_definition(route6b)?;
    ctx.add_route_definition(route7)?;
    ctx.add_route_definition(route8)?;
    ctx.add_route_definition(route8a)?;
    ctx.add_route_definition(route8b)?;

    // Category 3: Resilience
    ctx.add_route_definition(route9)?;
    ctx.add_route_definition(route10)?;
    ctx.add_route_definition(route11)?;

    // Category 4: Components
    ctx.add_route_definition(route12)?;
    ctx.add_route_definition(route13)?;
    ctx.add_route_definition(route13b)?;
    ctx.add_route_definition(route14)?;
    ctx.add_route_definition(route15)?;
    ctx.add_route_definition(route16)?;
    ctx.add_route_definition(route17_target)?;
    ctx.add_route_definition(route17_suspend)?;
    ctx.add_route_definition(route17_resume)?;

    // Category 5: Advanced
    ctx.add_route_definition(route18)?;
    ctx.add_route_definition(route19)?;
    ctx.add_route_definition(route19a)?;
    ctx.add_route_definition(route19b)?;
    ctx.add_route_definition(route20)?;
    ctx.add_route_definition(route21)?;
    ctx.add_route_definition(route21b)?;
    ctx.add_route_definition(route22)?;

    // Start the context
    ctx.start().await?;

    // Print banner
    print_banner(file_path);

    // Wait for shutdown
    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down gracefully (30s timeout)...");
    ctx.stop().await?;

    // Print final metrics
    metrics_for_print.print_stats();
    println!("Shutdown complete.");

    Ok(())
}

fn print_banner(file_path: &str) {
    println!();
    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║           rust-camel COMPREHENSIVE SHOWCASE (22 routes)              ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝");    println!();
    println!("CATEGORY 1: EIP BASICS");
    println!("  1. eip-filter           - Filter EIP (important vs normal)");
    println!("  2. eip-splitter         - Splitter EIP (parallel JSON array)");
    println!("  3. eip-aggregator       - Aggregator EIP (correlate by orderId)");
    println!("  4. eip-wiretap          - WireTap EIP (fire-and-forget tap)");
    println!("  5. eip-multicast        - Multicast EIP (parallel fan-out)");
    println!();
    println!("CATEGORY 2: ROUTING");
    println!("  6. eip-content-based-router - Content-Based Router (US/EU/APAC)");
    println!("  7. eip-stop             - Stop EIP (halt matching exchanges)");
    println!("  8. eip-recipient-list   - Recipient List (filter-based dynamic routing)");
    println!();
    println!("CATEGORY 3: RESILIENCE");
    println!("  9. resilience-circuit-breaker - Circuit Breaker (fail-fast)");
    println!(" 10. resilience-retry     - Retry with exponential backoff");
    println!(" 11. resilience-dlc       - Dead Letter Channel");
    println!();
    println!("CATEGORY 4: COMPONENTS");
    println!(" 12. comp-timer-log       - Timer + Log (basic)");
    println!(" 13. comp-direct          - Direct (route linking)");
    println!(" 14. comp-file-write      - File (append to log)");
    println!(" 15. comp-http-client     - HTTP Client (https://httpbin.org)");
    println!(" 16. comp-mock            - Mock Component (testing)");
    println!(" 17. comp-cbus-*          - ControlBus (suspend/resume routes)");
    println!();
    println!("CATEGORY 5: ADVANCED");
    println!(" 18. adv-sequential       - Sequential Processing (HTTP counter)");
    println!(" 19. adv-correlation-ids  - Correlation ID Tracing");
    println!(" 20. adv-metrics          - Metrics Collection");
    println!(" 21. adv-timeout          - Timeout Handling");
    println!(" 22. adv-log-eip          - Log EIP (inline logging)");
    println!();
    println!("COMPONENTS: timer, log, direct, mock, file, http, https, controlbus");
    println!("EIPS: filter, splitter, aggregator, wiretap, multicast, cbr, stop");
    println!("FEATURES: correlation-ids, error-handlers, timeouts, sequential, metrics, tracer");
    println!();
    println!("File output:  {}/showcase.log", file_path);
    println!("Trace output: showcase-trace.log (JSON spans for all 22 routes)");
    println!("HTTP endpoint: http://0.0.0.0:8081/showcase/counter");
    println!();
    println!("────────────────────────────────────────────────────────────────────────");
    println!("Press Ctrl+C to stop...");
    println!();
}
