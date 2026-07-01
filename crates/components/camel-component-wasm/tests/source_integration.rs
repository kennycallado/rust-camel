//! Integration tests for [`WasmSourceConsumer`].
//!
//! # Prerequisites
//!
//! All tests in this module are `#[ignore]` by default because they require:
//!
//! 1. A pre-built guest `.wasm` at:
//!    `examples/wasm-source-webhook/guest/target/wasm32-wasip2/debug/wasm_source_webhook_guest.wasm`
//!
//!    Build with:
//!    ```sh
//!    cd examples/wasm-source-webhook/guest
//!    cargo build --target wasm32-wasip2
//!    ```
//!
//! 2. A free TCP port per test (tests use unique ports to run in parallel).
//!
//! Run with:
//! ```sh
//! cargo test -p camel-component-wasm --test source_integration -- --ignored
//! ```

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use camel_component_api::Consumer;
use camel_component_api::consumer::{ConsumerContext, ExchangeEnvelope};
use camel_core::Registry;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use camel_component_wasm::config::WasmConfig;
use camel_component_wasm::source_consumer::WasmSourceConsumer;

// ─── Constants ──────────────────────────────────────────────────────────────

/// Path to the pre-built guest wasm (relative to workspace root).
const GUEST_WASM_REL: &str =
    "examples/wasm-source-webhook/guest/target/wasm32-wasip2/debug/wasm_source_webhook_guest.wasm";

/// Timeout for the guest to bind its HTTP listener and be ready.
const BIND_WAIT: Duration = Duration::from_secs(5);

/// Timeout for stop() to complete cleanly.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Resolve the guest wasm path.
/// Returns `None` if the file doesn't exist (prerequisite not met).
///
/// Checks two locations, in order:
/// 1. `$CARGO_TARGET_DIR/wasm32-wasip2/debug/…` — used when the workspace
///    redirects all build output to a shared target dir (CI and this repo's
///    dev setup both do). The guest's own `guest/target/` never fills in that
///    case, so relying on it alone would make the test silently skip.
/// 2. `examples/wasm-source-webhook/guest/target/wasm32-wasip2/debug/…` —
///    the default location when no shared target dir is configured.
fn guest_wasm_path() -> Option<PathBuf> {
    const GUEST_WASM_FILE: &str = "wasm32-wasip2/debug/wasm_source_webhook_guest.wasm";

    // 1. Shared target dir (CARGO_TARGET_DIR).
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        let path = PathBuf::from(target_dir).join(GUEST_WASM_FILE);
        if path.exists() {
            return Some(path);
        }
    }

    // 2. Default per-crate target dir, relative to the workspace root.
    // Cargo sets CARGO_MANIFEST_DIR to the crate root; the workspace root is
    // two levels up from crates/components/camel-component-wasm.
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = crate_dir
        .parent() // components/
        .and_then(|p| p.parent()) // crates/
        .and_then(|p| p.parent()); // workspace root

    let path = workspace_root?.join(GUEST_WASM_REL);
    if path.exists() { Some(path) } else { None }
}

/// Skip the test if the guest wasm is not built.
fn require_guest_wasm() -> PathBuf {
    guest_wasm_path().expect(
        "Guest wasm not found. Build with:\n\
         cd examples/wasm-source-webhook/guest && cargo build --target wasm32-wasip2",
    )
}

/// Create a ConsumerContext backed by a bounded channel.
/// Returns (context, receiver, cancel_token).
fn make_consumer_context(
    route_id: &str,
    capacity: usize,
) -> (
    ConsumerContext,
    mpsc::Receiver<ExchangeEnvelope>,
    CancellationToken,
) {
    let (tx, rx) = mpsc::channel(capacity);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), route_id.to_string());
    (ctx, rx, cancel)
}

/// Create a WasmSourceConsumer with the given guest config and a short timeout.
fn make_consumer(guest_config: Vec<(String, String)>) -> WasmSourceConsumer {
    let wasm_path = require_guest_wasm();
    let config = WasmConfig {
        timeout_secs: 5,
        ..WasmConfig::default()
    };
    WasmSourceConsumer::new(
        wasm_path,
        config,
        guest_config,
        Arc::new(Mutex::new(Registry::new())),
    )
}

/// Send a raw HTTP POST request over TCP and return the response status line.
///
/// Uses raw TCP because `reqwest` is not in dev-dependencies and we want
/// minimal dependencies for integration tests.
async fn send_http_post(port: u16, path: &str, body: &[u8]) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("failed to connect to source HTTP listener");

    let request = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         \r\n",
        body.len()
    );

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    stream
        .write_all(request.as_bytes())
        .await
        .expect("failed to write request headers");
    stream
        .write_all(body)
        .await
        .expect("failed to write request body");

    let mut buf = vec![0u8; 1024];
    let n = stream
        .read(&mut buf)
        .await
        .expect("failed to read response");
    String::from_utf8_lossy(&buf[..n]).to_string()
}

/// Wait until the given TCP port accepts connections, or panic after timeout.
async fn wait_for_bind(port: u16, timeout: Duration) {
    let start = std::time::Instant::now();
    loop {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        if start.elapsed() > timeout {
            panic!("port {port} did not bind within {timeout:?}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Allocate a unique port by binding to port 0 and reading the assigned port.
async fn free_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind ephemeral port");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

// ─── Tests ──────────────────────────────────────────────────────────────────

/// Verify that start → stop completes cleanly without panics or hangs.
///
/// Prerequisites: pre-built guest wasm.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn test_source_lifecycle_start_stop() {
    let port = free_port().await;
    let guest_config = vec![
        ("bind".into(), format!("127.0.0.1:{port}")),
        ("path".into(), "/webhook".into()),
    ];

    let mut consumer = make_consumer(guest_config);
    let (ctx, _rx, _cancel) = make_consumer_context("test-lifecycle", 16);

    // Start should succeed (engine init, component load, HTTP bind).
    consumer.start(ctx).await.expect("start() failed");

    // Wait for the HTTP listener to bind.
    wait_for_bind(port, BIND_WAIT).await;

    // Stop should complete within timeout — no panic, no hang.
    let stop_result = tokio::time::timeout(STOP_TIMEOUT, consumer.stop()).await;
    assert!(
        stop_result.is_ok(),
        "stop() did not complete within {STOP_TIMEOUT:?}"
    );
    stop_result.unwrap().expect("stop() returned error");
}

/// End-to-end: start consumer, send HTTP POST, verify exchange arrives in pipeline.
///
/// Prerequisites: pre-built guest wasm.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn test_source_webhook_e2e() {
    let port = free_port().await;
    let guest_config = vec![
        ("bind".into(), format!("127.0.0.1:{port}")),
        ("path".into(), "/webhook".into()),
    ];

    let mut consumer = make_consumer(guest_config);
    let (ctx, mut rx, _cancel) = make_consumer_context("test-e2e", 16);

    consumer.start(ctx).await.expect("start() failed");
    wait_for_bind(port, BIND_WAIT).await;

    // Send an HTTP POST with a JSON body.
    let body = b"{\"event\":\"test\"}";
    let response = send_http_post(port, "/webhook", body).await;

    // The listener should respond 202 Accepted.
    assert!(
        response.contains("202"),
        "expected 202 response, got: {response}"
    );

    // An exchange should arrive on the pipeline receiver.
    let envelope = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out waiting for exchange")
        .expect("channel closed before exchange arrived");

    // Verify the exchange carries the HTTP body.
    // The guest converts HTTP body to Body::Bytes, so check both Text and Bytes variants.
    let body_contains_event = match &envelope.exchange.input.body {
        camel_api::Body::Text(s) => s.contains("event"),
        camel_api::Body::Bytes(b) => String::from_utf8_lossy(b).contains("event"),
        _ => false,
    };
    assert!(
        body_contains_event,
        "exchange body should contain the POST payload, got: {:?}",
        envelope.exchange.input.body
    );

    // Verify HTTP metadata in properties.
    assert!(
        envelope
            .exchange
            .properties
            .contains_key("camel.http.method"),
        "exchange should have camel.http.method property"
    );

    // Clean shutdown.
    tokio::time::timeout(STOP_TIMEOUT, consumer.stop())
        .await
        .expect("stop() timed out")
        .expect("stop() error");
}

/// Start consumer, send no requests, call stop(). Guest is blocked in accept-http.
/// Verify clean exit — cancel token triggers epoch interruption, guest exits.
///
/// Prerequisites: pre-built guest wasm.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn test_source_cancellation_channel_close() {
    let port = free_port().await;
    let guest_config = vec![
        ("bind".into(), format!("127.0.0.1:{port}")),
        ("path".into(), "/webhook".into()),
    ];

    let mut consumer = make_consumer(guest_config);
    let (ctx, _rx, _cancel) = make_consumer_context("test-cancel", 16);

    consumer.start(ctx).await.expect("start() failed");
    wait_for_bind(port, BIND_WAIT).await;

    // Don't send any requests — guest is blocked in accept_http().
    // Stop should cancel the token, increment epoch, and the guest should exit.
    let stop_result = tokio::time::timeout(STOP_TIMEOUT, consumer.stop()).await;
    assert!(
        stop_result.is_ok(),
        "stop() did not complete within {STOP_TIMEOUT:?} — guest may be stuck in accept-http"
    );
    stop_result.unwrap().expect("stop() returned error");
}

/// Pipeline delays each exchange by 500ms. Send a burst of requests.
///
/// Backpressure chain (all capacity 1): axum handler `.send().await` →
/// request channel → guest `accept-http`/`submit-exchange` → exchange channel
/// → pipeline bridge → ConsumerContext channel. There are a few one-slot
/// buffers in that chain, so the FIRST few requests are absorbed without the
/// client waiting; once every slot is occupied by the slow (500ms) pipeline,
/// further inbound HTTP `.send()`s park until the pipeline drains.
///
/// Sending a burst larger than the total buffer depth therefore forces the
/// tail of the burst to observe client-visible backpressure. This answers
/// spike Q1 (backpressure) with a real measurement.
///
/// Prerequisites: pre-built guest wasm.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn test_source_backpressure_sequential() {
    let port = free_port().await;
    let guest_config = vec![
        ("bind".into(), format!("127.0.0.1:{port}")),
        ("path".into(), "/webhook".into()),
    ];

    let mut consumer = make_consumer(guest_config);
    // Use capacity=1 to match EXCHANGE_CHANNEL_CAPACITY — strict backpressure.
    let (ctx, mut rx, _cancel) = make_consumer_context("test-backpressure", 1);

    consumer.start(ctx).await.expect("start() failed");
    wait_for_bind(port, BIND_WAIT).await;

    // Spawn a pipeline consumer that delays 500ms per exchange.
    let pipeline_handle = tokio::spawn(async move {
        let mut received = Vec::new();
        while let Some(envelope) = rx.recv().await {
            received.push(envelope.exchange);
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        received
    });

    // Send a burst of 6 requests — larger than the ~3-slot buffer depth so the
    // tail must wait for the 500ms-per-exchange pipeline to drain.
    const BURST: usize = 6;
    let send_start = std::time::Instant::now();
    for i in 0..BURST {
        let body = format!("{{\"seq\":{i}}}");
        let _resp = send_http_post(port, "/webhook", body.as_bytes()).await;
    }
    let send_elapsed = send_start.elapsed();

    // With a 6-request burst against a 500ms drain and ~3 buffer slots, at
    // least ~2 requests must block on the pipeline (≥1s). Assert a conservative
    // ≥800ms; without backpressure the whole burst would finish in <100ms.
    assert!(
        send_elapsed >= Duration::from_millis(800),
        "expected backpressure delay (>=800ms), but sends completed in {send_elapsed:?}"
    );

    // Stop consumer and collect pipeline results.
    tokio::time::timeout(STOP_TIMEOUT, consumer.stop())
        .await
        .expect("stop() timed out")
        .expect("stop() error");

    // Drop the consumer's exchange_tx to close the bridge → pipeline rx closes.
    // The pipeline_handle should finish.
    let exchanges = tokio::time::timeout(Duration::from_secs(5), pipeline_handle)
        .await
        .expect("pipeline task timed out")
        .expect("pipeline task panicked");

    assert!(
        !exchanges.is_empty(),
        "expected at least 1 exchange to reach pipeline, got {}",
        exchanges.len()
    );
}

/// Crash recovery: the guest traps inside `run()`.
///
/// The webhook guest traps deliberately when `configure()` receives
/// `crash=run` (see `examples/wasm-source-webhook/guest/src/lib.rs`). This
/// answers spike Q3: crash/lifecycle semantics across the WASM boundary.
///
/// Verifies that:
/// 1. The guest trap is surfaced as an `Err` from the run task
///    (retrieved via `background_task_handle()`, which the Runtime owns).
/// 2. `stop()` completes cleanly and is idempotent after a trap.
///
/// Prerequisites: pre-built guest wasm.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn test_source_crash_recovery() {
    let port = free_port().await;
    let guest_config = vec![
        ("bind".into(), format!("127.0.0.1:{port}")),
        ("path".into(), "/webhook".into()),
        ("crash".into(), "run".into()),
    ];

    let mut consumer = make_consumer(guest_config);
    let (ctx, _rx, _cancel) = make_consumer_context("test-crash", 16);

    // start() calls configure() (succeeds) and spawns run() (which traps).
    consumer.start(ctx).await.expect("start() should succeed");

    // The Runtime owns the run task via background_task_handle(); take it and
    // confirm the trap propagates as an error rather than a silent exit.
    let run_task = consumer
        .background_task_handle()
        .expect("run task handle should be present after start()");

    let outcome = tokio::time::timeout(Duration::from_secs(5), run_task)
        .await
        .expect("run task did not finish after guest trap");
    let join_result = outcome.expect("run task should not have been cancelled");
    assert!(
        join_result.is_err(),
        "guest trap must surface as an Err from the run task, got: {join_result:?}"
    );

    // stop() must remain clean and idempotent even though the guest already
    // crashed (the host does NOT guarantee stop() on crash — but calling it
    // must not panic or hang).
    tokio::time::timeout(STOP_TIMEOUT, consumer.stop())
        .await
        .expect("stop() timed out after crash")
        .expect("stop() returned error after crash");
}
