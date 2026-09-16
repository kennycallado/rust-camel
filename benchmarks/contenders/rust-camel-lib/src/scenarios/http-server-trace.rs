//! Smoke-trace variant of the http-server route — compiled ONLY when
//! the `bench-trace` feature is enabled (e_opus ruling D1, 2026-09-16;
//! bd rc-h42s6).
//!
//! The DEFAULT build contains none of this code: the measured fixture
//! is minimal-bare (`scenarios/http-server.rs`) and the harness
//! M1–M4 builds NEVER enable `bench-trace`. Prove the boundary with
//! `benchmarks/harness/checks/trace-absent.sh` — the default binary
//! must not contain the `BENCH_HTTP_REQUEST` string; the feature
//! build must (that FAIL result is the check working as intended).
//!
//! Shape: `from("http://0.0.0.0:8080/bench")` →
//! `log("BENCH_HTTP_REQUEST received")` → `process(id++)` →
//! `set_body("pong")` → 200, emitting `BENCH_HTTP_REQUEST received`
//! and a 1-based `BENCH_HTTP_REQUEST id=<n>` line per request.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use camel_api::CamelError;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_http::HttpComponent;
use camel_core::context::CamelContext;
use camel_processor::LogLevel;

const LISTEN_PORT: u16 = 8080;

/// Verbatim port of the per-scenario fixture's `main()`.
pub fn run() -> i32 {
    match main_async() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("http-server-trace: {e:?}");
            1
        }
    }
}

#[tokio::main]
async fn main_async() -> Result<(), CamelError> {
    // Initialize the tracing subscriber first so every subsequent log
    // line (including our own BENCH_ROUTE_READY line) lands on stdout.
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::builder().build().await?;
    ctx.register_component(HttpComponent::new());

    // Trace shape: the minimal-bare route plus the smoke-trace steps
    // (received + 1-based id counter, relaxed AtomicU64).
    let request_counter = Arc::new(AtomicU64::new(0));
    let counter_for_route = Arc::clone(&request_counter);
    let route = RouteBuilder::from(format!("http://0.0.0.0:{LISTEN_PORT}/bench").as_str())
        .route_id("bench-http-trace")
        .log("BENCH_HTTP_REQUEST received", LogLevel::Info)
        .process(move |exchange| {
            let counter = Arc::clone(&counter_for_route);
            async move {
                let id = counter.fetch_add(1, Ordering::Relaxed) + 1;
                tracing::info!("BENCH_HTTP_REQUEST id={id}");
                Ok(exchange)
            }
        })
        .set_body("pong")
        .build()?;

    ctx.add_route_definition(route).await?;

    // ADR-0061 bind-exposure gate: 0.0.0.0:8080 is non-loopback and the
    // route is Public — acknowledge explicitly.
    let acks = camel_core::route_controller::BindExposureAcks::new(
        [("0.0.0.0:8080".to_string(), true)].into_iter().collect(),
    );
    ctx.set_bind_exposure_acks(acks).await;

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
