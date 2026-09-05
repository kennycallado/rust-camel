//! Integration tests for staged-listener consumption by
//! [`WasmSourceConsumer`] (wasm-bound-address, Task WASM-1).
//!
//! The tests stage a real socket via `common::stage_wasm_source_listener`
//! and prove the consumer takes (or deterministically refuses) the parked
//! socket at its bind site:
//!
//! - served end-to-end: the staged socket is the one serving the webhook;
//!   a fresh bind would have hit `EADDRINUSE` because the helper holds
//!   the socket until consumption.
//! - unstaged port-zero config: binds normally through the `None` arm.
//! - same port staged under `0.0.0.0`, route claims `127.0.0.1`: start
//!   fails deterministically with the staged-conflict error.
//!
//! # Prerequisites
//!
//! All tests in this module are `#[ignore]` by default because they
//! require a pre-built guest `.wasm` at:
//! `examples/wasm-source-webhook/guest/target/wasm32-wasip2/debug/wasm_source_webhook_guest.wasm`
//!
//! Build with:
//! ```sh
//! cd examples/wasm-source-webhook/guest
//! cargo build --target wasm32-wasip2
//! ```
//!
//! Run with:
//! ```sh
//! cargo test -p camel-component-wasm --test staged_listener_source -- --ignored
//! ```

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use camel_component_api::Consumer;
use camel_component_api::consumer::{ConsumerContext, ExchangeEnvelope};
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

/// Resolve the guest wasm path (same two-location scheme as
/// `tests/source_integration.rs`).
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

/// Create a WasmSourceConsumer with the given guest config and a short
/// timeout. The endpoint URI mirrors the config entries so the consumer's
/// naming key matches the `wasm:...?bind=...&path=...` URI the factory
/// would build.
fn make_consumer(guest_config: Vec<(String, String)>) -> WasmSourceConsumer {
    let wasm_path = require_guest_wasm();
    let config = WasmConfig {
        timeout_secs: 5,
        ..WasmConfig::default()
    };
    let query = guest_config
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    let uri = format!("wasm:guest.wasm?{query}");
    WasmSourceConsumer::new(
        wasm_path,
        uri,
        config,
        guest_config,
        Arc::new(camel_component_api::NoOpComponentContext),
    )
}

/// Send a raw HTTP POST request over TCP and return the response status
/// line (raw TCP keeps dev-dependencies minimal, as in
/// `tests/source_integration.rs`).
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

// ─── Tests ──────────────────────────────────────────────────────────────────

/// End-to-end: the staged socket is consumed at the bind site and serves
/// the webhook exchange. A fresh bind would have failed with `EADDRINUSE`
/// because the helper holds the staged socket until consumption — a served
/// webhook proves the staged socket was the one consumed.
///
/// Prerequisites: pre-built guest wasm.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn staged_listener_served_end_to_end() {
    let port = common::stage_wasm_source_listener("127.0.0.1").await;
    let guest_config = vec![
        ("bind".into(), format!("127.0.0.1:{port}")),
        ("path".into(), "/webhook".into()),
    ];

    let mut consumer = make_consumer(guest_config);
    let (ctx, mut rx, _cancel) = make_consumer_context("staged-served", 16);

    // start() must take the staged socket; binding fresh would collide
    // with the parked socket the helper still logically owns.
    consumer
        .start(ctx)
        .await
        .expect("staged-listener consumption must start the route");
    wait_for_bind(port, BIND_WAIT).await;

    let body = b"{\"event\":\"staged\"}";
    let response = send_http_post(port, "/webhook", body).await;
    assert!(
        response.contains("202"),
        "expected 202 response from the staged socket, got: {response}"
    );

    let envelope = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out waiting for exchange")
        .expect("channel closed before exchange arrived");
    assert!(
        envelope
            .exchange
            .properties
            .contains_key("camel.http.method"),
        "exchange should have camel.http.method property"
    );

    tokio::time::timeout(STOP_TIMEOUT, consumer.stop())
        .await
        .expect("stop() timed out")
        .expect("stop() error");
}

/// No staged entry → the consumer binds `127.0.0.1:0` itself through the
/// `None` arm. Port zero goes straight into the config: no helper, no
/// port discovery, no bind-read-drop probe.
///
/// Prerequisites: pre-built guest wasm.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn unstaged_bind_starts() {
    let guest_config = vec![
        ("bind".into(), "127.0.0.1:0".into()),
        ("path".into(), "/webhook".into()),
    ];

    let mut consumer = make_consumer(guest_config);
    let (ctx, _rx, _cancel) = make_consumer_context("unstaged-bind", 16);

    consumer
        .start(ctx)
        .await
        .expect("unstaged port-zero bind must start through the None arm");

    tokio::time::timeout(STOP_TIMEOUT, consumer.stop())
        .await
        .expect("stop timed out")
        .expect("stop should succeed");
}

/// Same port staged under `0.0.0.0`, route claims `127.0.0.1`: start()
/// fails deterministically with the staged-conflict error, never a
/// bind-race flake.
///
/// Prerequisites: pre-built guest wasm.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn wrong_host_staged_start_fails_deterministically() {
    let port = common::stage_wasm_source_listener("0.0.0.0").await;

    let bind = format!("127.0.0.1:{port}");
    let guest_config = vec![("bind".into(), bind), ("path".into(), "/webhook".into())];

    let mut consumer = make_consumer(guest_config);
    let (ctx, _rx, _cancel) = make_consumer_context("wrong-host-staged", 16);

    let err = consumer
        .start(ctx)
        .await
        .expect_err("same-port different-host staged start must fail");
    let msg = err.to_string();
    assert!(
        msg.contains(&format!(
            "staged listener conflict on port {port}: staged under host 0.0.0.0, requested 127.0.0.1"
        )),
        "error must state the staged conflict exactly: {msg}"
    );
}
