//! T4a XSLT bridge fixture — rust-camel-lib (Pair A partner, bd Task 4).
//!
//! Implements the spec §4.6 / T4a brief route shape:
//!   from(timer:bench?period=10ms&repeatCount=10000)
//!     .set_body(constant <1KB XML payload>)
//!     .process(set BENCH_START = Instant::now())
//!     .to(xslt:<shared/identity-transform.xsl>)
//!     .process(emit BENCH_LATENCY <id> <duration_ns> to tmpfs file)
//!
//! `camel-xslt` delegates the transform to `bridges/xml` via gRPC
//! mTLS — this is the bridge tax surface the T4a cell measures. The
//! Apache Camel standalone + Quarkus fixtures run Saxon-HE 12.5
//! in-process; the per-call overhead (TLS record + HTTP/2 framing +
//! protobuf serialize + Java execute + serialize back) is what this
//! cell quantifies.
//!
//! # Bridge binary + PID file
//!
//! The fixture points `CAMEL_XML_BRIDGE_BINARY_PATH` at the shared
//! `bridge-wrapper.sh` script, which writes its own PID (= the
//! bridge's PID after exec) to `V3_BRIDGE_PID_FILE` before exec'ing
//! the real bridge binary. The harness reads this file at round
//! boundaries per spec §4.11 to detect bridge restarts (a PID change
//! mid-round invalidates the round).
//!
//! # Marker mechanism
//!
//! Per spec §4.6 + Task 3 rc-w1u9 contract: the marker
//! `BENCH_ROUTE_READY <unix_ms>` is emitted immediately after
//! `ctx.start().await?` returns — which under the rc-w1u9 handshake
//! means every registered consumer has completed its start phase.
//! For the timer consumer (no listener to bind), start completion
//! means the timer scheduler is armed and the first tick is
//! scheduled. The bridge itself is spawned lazily on the FIRST tick
//! (not at context start) — the harness's bridge readiness probe
//! (first warmup tick must produce output within 30s per spec §4.6)
//! covers this; the M1 clock stops on the probe, not on the marker.
//!
//! # No self-instrumentation
//!
//! Timing is captured externally by the harness's `time -v` wrapper.
//! The only in-band signals are BENCH_ROUTE_READY (1× startup) and
//! BENCH_LATENCY (per-tick records for the M2 Protocol B harness).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use camel_api::CamelError;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_processor::LogLevel;
use camel_xslt::XsltComponent;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;

/// Exchange extension key under which the pre-`.to()` `Instant` is stored.
/// Extension (not property) because `Instant` is not serializable.
const BENCH_START: &str = "BenchStart";

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    // Install rustls CryptoProvider explicitly before any TLS code
    // runs (camel-bridge spawns the xml-bridge subprocess with mTLS;
    // rustls 0.23 needs a provider installed before the first TLS
    // operation). Idempotent — the `_` discard handles re-install.
    let _ = rustls::crypto::ring::default_provider().install_default();

    tracing_subscriber::fmt().with_target(false).init();

    // Resolve shared asset paths from env vars. Defaults match the
    // smoke test's CWD-relative layout (scenario dir).
    let payload_path = std::env::var("BENCH_PAYLOAD")
        .unwrap_or_else(|_| "../shared/bench-payload.xml".to_string());
    let stylesheet_path = std::env::var("BENCH_STYLESHEET")
        .unwrap_or_else(|_| "../shared/identity-transform.xsl".to_string());
    let latency_file = std::env::var("BENCH_LATENCY_FILE")
        .unwrap_or_else(|_| "/tmp/v3-protocol-b-t4a-rust-camel-lib.log".to_string());

    // Pre-load payload into a constant string so the timer route
    // doesn't pay file I/O on every tick. The bytes are byte-identical
    // to what the Apache Camel fixtures read from the same file.
    let payload = tokio::fs::read_to_string(&payload_path)
        .await
        .map_err(|e| {
            CamelError::EndpointCreationFailed(format!(
                "failed to read bench payload {payload_path}: {e}"
            ))
        })?;

    // Pre-create / truncate the latency file so the first append is
    // fast (no inode allocation on the hot path).
    if let Some(parent) = std::path::Path::new(&latency_file).parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    _ = tokio::fs::File::create(&latency_file).await;

    // Wire up the bridge wrapper: point CAMEL_XML_BRIDGE_BINARY_PATH
    // at the shared bridge-wrapper.sh so the bridge PID is recorded
    // once at spawn (spec §4.11). The wrapper exec's the real binary
    // (path from CAMEL_XML_BRIDGE_REAL_BINARY), so the recorded PID
    // IS the bridge's PID for its whole lifetime.
    //
    // NOTE: camel-bridge spawns the binary lazily on the first xslt:
    // invocation, not at context start. The wrapper is invoked then.
    // If V3_BRIDGE_PID_FILE is unset, the wrapper exec's the bridge
    // with no PID write (best-effort; harness tolerates absence per
    // Task 2 concern #5).
    if std::env::var("CAMEL_XML_BRIDGE_BINARY_PATH").is_err() {
        let wrapper = std::env::var("BENCH_BRIDGE_WRAPPER")
            .unwrap_or_else(|_| "../shared/bridge-wrapper.sh".to_string());
        // SAFETY: this runs at process startup before any async task or
        // thread is spawned. No concurrent access to the environment is
        // possible at this point (single-threaded init path).
        unsafe { std::env::set_var("CAMEL_XML_BRIDGE_BINARY_PATH", wrapper) };
    }

    let mut ctx = CamelContext::builder().build().await?;
    ctx.register_component(TimerComponent::new());
    ctx.register_component(XsltComponent::default());

    let tick_counter = Arc::new(AtomicU64::new(0));
    let latency_file_arc = Arc::new(latency_file.clone());

    // Build the T4a route programmatically (Pair A — no YAML/DSL parsing).
    // The to("xslt:...") step spawns the bridge on first invocation
    // (lazy); subsequent invocations reuse the warm gRPC channel.
    let rc_for_route = Arc::clone(&tick_counter);
    let lf_for_route = Arc::clone(&latency_file_arc);
    let stylesheet_uri = format!("xslt:{stylesheet_path}");
    let route = RouteBuilder::from("timer:bench?period=10&repeatCount=10000")
        .route_id("bench-xslt")
        .set_body(payload)
        // Bracket the bridge gRPC dispatch (inside `.to(xslt:...)`) with
        // Instant t_start. Stored via exchange extension because Instant
        // is not serializable (properties only hold serializable Value).
        // Read back in the post-.to() processor to compute duration_ns.
        .process(move |mut exchange| async move {
            exchange.set_extension(BENCH_START, Arc::new(Instant::now()));
            Ok(exchange)
        })
        .to(stylesheet_uri.as_str())
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
                // Append to tmpfs: tokio async write keeps the route's
                // executor responsive (no blocking syscall). Tmpfs writes
                // are sub-microsecond so this perturbs the p99 minimally.
                if let Ok(mut f) = OpenOptions::new().append(true).open(lf.as_str()).await {
                    let _ = f.write_all(line.as_bytes()).await;
                }
                Ok(exchange)
            }
        })
        .log("BENCH_XSLT_TICK", LogLevel::Info)
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    // Marker emission: by the rc-w1u9 contract, when control reaches
    // this line every registered consumer has started. For the timer
    // consumer, that means the scheduler is armed. The bridge itself
    // spawns lazily on first tick; the harness's bridge readiness probe
    // (first warmup tick within 30s per spec §4.6) covers that gap.
    let unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    println!("BENCH_ROUTE_READY {unix_ms}");

    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;
    Ok(())
}
