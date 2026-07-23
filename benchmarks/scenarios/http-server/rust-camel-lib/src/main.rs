//! T3 HTTP server fixture — rust-camel-lib (Pair A partner, bd Task 3).
//!
//! Implements the canonical minimal T3 route shape:
//!   `from("http://0.0.0.0:8080/bench")` → `set_body("pong")` → 200.
//!
//! # Marker mechanism (HARD entrance criterion from Task 1 review)
//!
//! Per spec §4.10: "M1 marker is listener-bound AND accepting
//! connections (NOT on first request)". The Task 1 review explicitly
//! upgraded this from a soft caveat to a hard T3 entrance criterion
//! (see Task 1 report "Review fixes" → "Important #3 — Marker not
//! coupled to listener bind"). The spike 1A used a sibling timer as a
//! throwaway; the production T3 fixture must NOT inherit that shape.
//!
//! This fixture's marker is **listener-bound by construction** via the
//! rc-w1u9 production handshake (`ConsumerStartupMode::Explicit`):
//! 1. `HttpConsumer::startup_mode()` returns `Explicit`
//!    (crates/components/camel-http/src/lib.rs:1569).
//! 2. `HttpConsumer::start()` performs `TcpListener::bind` + axum
//!    spawn + route registration, THEN calls `ctx.mark_ready()`
//!    (camel-http/src/lib.rs:1289). Any bind failure makes `start()`
//!    return `Err`, which `spawn_consumer_task` propagates via
//!    `startup_for_task.mark_failed(...)` (consumer_management.rs:98).
//! 3. `CamelContext::start()` awaits the `StartupReceiver` for every
//!    Explicit consumer; it returns `Ok` ONLY after `mark_ready()` has
//!    fired (listener genuinely accepting) and returns `Err` on
//!    `mark_failed()` (bind error surfaced as a proper startup error).
//! 4. Therefore the line `println!("BENCH_ROUTE_READY ...")` below,
//!    emitted immediately after `ctx.start().await?`, is 1:1 coupled
//!    to "the listener is bound and accepting" — no TCP probe, no
//!    sibling timer, no orphan-port false-positive. The previous
//!    TCP-probe approach (which could succeed against a stale orphan)
//!    has been deleted; this fixture no longer polls the port.
//!
//! # Per-request work
//!
//! The route is the canonical minimal `setBody("pong")` only — no
//! per-request counter, no process step, no log emission. The harness
//! measures cold-start + RSS, not request latency; per-request
//! observability belongs to the loadgen, not the fixture.

use std::time::{SystemTime, UNIX_EPOCH};

use camel_api::CamelError;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_http::HttpComponent;
use camel_core::context::CamelContext;

const LISTEN_PORT: u16 = 8080;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    // Initialize the tracing subscriber first so every subsequent log
    // line (including our own BENCH_ROUTE_READY line) lands on stdout.
    tracing_subscriber::fmt().with_target(false).init();

    // 1. Construct the CamelContext via the public builder.
    let mut ctx = CamelContext::builder().build().await?;

    // 2. Register the HTTP component. The route uses `from("http:...")`
    //    so the http scheme must be resolvable in the registry.
    ctx.register_component(HttpComponent::new());

    // 3. Build the T3 route programmatically (Pair A — no YAML/DSL
    //    parsing). Canonical minimal shape: `from(http).set_body(pong)`.
    //    The `.set_body("pong")` form is a literal string step —
    //    equivalent to the spec's `respond(200, body=pong)`.
    //    camel-http's reply finaliser (camel-http/CONTEXT.md "Accepted
    //    — reply body") maps Body::Text → text/plain; charset=utf-8 +
    //    200 status (default per ADR-0024). No per-request counter,
    //    no process step, no log emission.
    //
    //    No `httpMethod` URI parameter — the consumer accepts any
    //    method (including POST, which is what the spec requires for
    //    the body-bearing `/bench` endpoint).
    let route = RouteBuilder::from(format!("http://0.0.0.0:{LISTEN_PORT}/bench").as_str())
        .route_id("bench-http")
        .set_body("pong")
        .build()?;

    // 4. Register the route and start the context. `ctx.start()`
    //    blocks until all registered routes' consumers have started;
    //    for the HTTP consumer, "started" means `TcpListener::bind`
    //    succeeded, the axum accept loop was spawned, and the route's
    //    path/REST endpoint was registered — see the file-level
    //    docstring for the rc-w1u9 handshake chain. Any bind failure
    //    (e.g. EADDRINUSE because an orphan holds 8080) surfaces as
    //    `ctx.start()` returning `Err`, which `?` propagates as a
    //    process exit (no marker emitted → harness sees a hard
    //    failure, which is the correct outcome).
    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    // 5. Marker emission — listener-bound, NOT sibling-timer, NOT
    //    TCP probe. By the rc-w1u9 contract (see docstring), when
    //    control reaches this line the HTTP listener IS bound and
    //    accepting connections on `0.0.0.0:{LISTEN_PORT}`. We emit
    //    `BENCH_ROUTE_READY <unix_ms>` immediately so the harness's
    //    M1 clock stops at the earliest moment the listener is
    //    genuinely ready. No background probe task, no port polling.
    let unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    println!("BENCH_ROUTE_READY {unix_ms}");

    // 6. Keep the CamelContext alive until killed by the benchmark
    //    harness. Mirrors v1 / T2 rust-camel-lib behavior — the
    //    marker has fired, the route is servicing requests, and
    //    the harness terminates the process externally after
    //    measurement completes.
    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;
    Ok(())
}
