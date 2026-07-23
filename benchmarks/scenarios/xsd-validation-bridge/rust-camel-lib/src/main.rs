//! T4b XSD validation bridge fixture — rust-camel-lib (Pair A, bd Task 4).
//!
//! Implements the spec §4.6 / T4b brief route shape:
//!   from(timer:bench?period=10&repeatCount=10000)
//!     .set_body(constant <1KB XML payload>)
//!     .process(set BENCH_START = Instant::now())
//!     .to(validator:<shared/schema.xsd>)
//!     .process(emit BENCH_LATENCY <id> <duration_ns> to tmpfs file)
//!
//! `camel-validator` delegates XSD validation to `bridges/xml` via
//! gRPC mTLS (Xerces-J 2.12.2). The Apache Camel standalone + Quarkus
//! fixtures run Xerces-J in-process; the per-call overhead this cell
//! measures is the bridge tax for XSD validation.
//!
//! Marker + per-tick patterns mirror the T4a rust-camel-lib fixture;
//! see that file's docstring for the full spec rationale.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use camel_api::CamelError;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_timer::TimerComponent;
use camel_component_validator::ValidatorComponent;
use camel_core::context::CamelContext;
use camel_processor::LogLevel;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;

/// Exchange extension key under which the pre-`.to()` `Instant` is stored.
/// Extension (not property) because `Instant` is not serializable.
const BENCH_START: &str = "BenchStart";

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    // Install rustls CryptoProvider explicitly (camel-bridge / rcgen
    // use rustls for ephemeral mTLS to the bridge subprocess).
    let _ = rustls::crypto::ring::default_provider().install_default();

    tracing_subscriber::fmt().with_target(false).init();

    let payload_path = std::env::var("BENCH_PAYLOAD")
        .unwrap_or_else(|_| "../shared/bench-payload.xml".to_string());
    let schema_path =
        std::env::var("BENCH_SCHEMA").unwrap_or_else(|_| "../shared/schema.xsd".to_string());
    let latency_file = std::env::var("BENCH_LATENCY_FILE")
        .unwrap_or_else(|_| "/tmp/v3-protocol-b-t4b-rust-camel-lib.log".to_string());

    let payload = tokio::fs::read_to_string(&payload_path)
        .await
        .map_err(|e| {
            CamelError::EndpointCreationFailed(format!(
                "failed to read bench payload {payload_path}: {e}"
            ))
        })?;

    if let Some(parent) = std::path::Path::new(&latency_file).parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    _ = tokio::fs::File::create(&latency_file).await;

    if std::env::var("CAMEL_XML_BRIDGE_BINARY_PATH").is_err() {
        let wrapper = std::env::var("BENCH_BRIDGE_WRAPPER")
            .unwrap_or_else(|_| "../shared/bridge-wrapper.sh".to_string());
        // SAFETY: process startup, single-threaded init.
        unsafe { std::env::set_var("CAMEL_XML_BRIDGE_BINARY_PATH", wrapper) };
    }

    let mut ctx = CamelContext::builder().build().await?;
    ctx.register_component(TimerComponent::new());
    ctx.register_component(ValidatorComponent::new());

    let tick_counter = Arc::new(AtomicU64::new(0));
    let latency_file_arc = Arc::new(latency_file.clone());

    let rc_for_route = Arc::clone(&tick_counter);
    let lf_for_route = Arc::clone(&latency_file_arc);
    let schema_uri = format!("validator:{schema_path}");
    let route = RouteBuilder::from("timer:bench?period=10&repeatCount=10000")
        .route_id("bench-xsd")
        .set_body(payload)
        // Bracket the bridge gRPC dispatch (inside `.to(validator:...)`)
        // with Instant t_start. Extension-stored because Instant is not
        // serializable. Read back in the post-.to() processor.
        .process(move |mut exchange| async move {
            exchange.set_extension(BENCH_START, Arc::new(Instant::now()));
            Ok(exchange)
        })
        .to(schema_uri.as_str())
        .process(move |exchange| {
            let rc = Arc::clone(&rc_for_route);
            let lf = Arc::clone(&lf_for_route);
            async move {
                let id = rc.fetch_add(1, Ordering::Relaxed) + 1;
                let duration_ns = exchange
                    .get_extension::<Instant>(BENCH_START)
                    .expect("BENCH_START must be set before .to() step")
                    .elapsed()
                    .as_nanos() as u64;
                let line = format!("BENCH_LATENCY {id} {duration_ns}\n");
                if let Ok(mut f) = OpenOptions::new().append(true).open(lf.as_str()).await {
                    let _ = f.write_all(line.as_bytes()).await;
                }
                Ok(exchange)
            }
        })
        .log("BENCH_XSD_TICK", LogLevel::Info)
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    let unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    println!("BENCH_ROUTE_READY {unix_ms}");

    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;
    Ok(())
}
