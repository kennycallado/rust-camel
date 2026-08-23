//! Bind-governance integration tests for [`WasmSourceConsumer`]
//! (`wasm-source-auth-kernel`, Task 1.4): operator-bind precedence,
//! bind-conflict refusal, and the ADR-0061 per-bind exposure gate.
//!
//! # Prerequisites
//!
//! All tests in this target are `#[ignore]` (ADR-0054): they start real
//! guest fixtures.
//!
//! 1. Webhook guest at
//!    `examples/wasm-source-webhook/guest/target/wasm32-wasip2/debug/wasm_source_webhook_guest.wasm`
//!    ```sh
//!    cd examples/wasm-source-webhook/guest
//!    cargo build --target wasm32-wasip2
//!    ```
//!
//! 2. Conflicting-bind guest at
//!    `crates/components/camel-component-wasm/tests/fixtures/conflicting-bind-guest/target/wasm32-wasip2/debug/conflicting_bind_guest.wasm`
//!    ```sh
//!    cd crates/components/camel-component-wasm/tests/fixtures/conflicting-bind-guest
//!    cargo build --target wasm32-wasip2
//!    ```
//!
//! 3. A free TCP port per test (tests use unique ports to run in parallel).
//!
//! Run with:
//! ```sh
//! cargo test -p camel-component-wasm --test source_bind_gate -- --ignored
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use camel_component_api::Consumer;
use camel_component_api::consumer::{ConsumerContext, ExchangeEnvelope};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use camel_component_wasm::WasmSourceBindAcks;
use camel_component_wasm::config::WasmConfig;
use camel_component_wasm::source_consumer::WasmSourceConsumer;

// ─── Constants ──────────────────────────────────────────────────────────────

/// Pre-built webhook guest wasm (relative to workspace root).
const WEBHOOK_GUEST_WASM_REL: &str =
    "examples/wasm-source-webhook/guest/target/wasm32-wasip2/debug/wasm_source_webhook_guest.wasm";

/// Pre-built conflicting-bind guest wasm (relative to workspace root).
const CONFLICT_GUEST_WASM_REL: &str = "crates/components/camel-component-wasm/tests/fixtures/conflicting-bind-guest/target/wasm32-wasip2/debug/conflicting_bind_guest.wasm";

/// Timeout for the guest to bind its HTTP listener and be ready.
const BIND_WAIT: Duration = Duration::from_secs(5);

/// Timeout for stop() to complete cleanly.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Resolve a pre-built guest wasm path (same two-location scheme as
/// `tests/source_integration.rs`): `$CARGO_TARGET_DIR/wasm32-wasip2/debug/…`
/// first, then the per-guest default location relative to the workspace
/// root. Returns `None` if the file doesn't exist (prerequisite not met).
fn guest_wasm_path(file: &str, default_rel: &str) -> Option<PathBuf> {
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        let path = PathBuf::from(target_dir)
            .join("wasm32-wasip2/debug")
            .join(file);
        if path.exists() {
            return Some(path);
        }
    }

    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = crate_dir
        .parent() // components/
        .and_then(|p| p.parent()) // crates/
        .and_then(|p| p.parent()); // workspace root

    let path = workspace_root?.join(default_rel);
    if path.exists() { Some(path) } else { None }
}

fn require_webhook_guest_wasm() -> PathBuf {
    guest_wasm_path("wasm_source_webhook_guest.wasm", WEBHOOK_GUEST_WASM_REL).expect(
        "Webhook guest wasm not found. Build with:\n\
         cd examples/wasm-source-webhook/guest && cargo build --target wasm32-wasip2",
    )
}

fn require_conflict_guest_wasm() -> PathBuf {
    guest_wasm_path("conflicting_bind_guest.wasm", CONFLICT_GUEST_WASM_REL).expect(
        "Conflicting-bind guest wasm not found. Build with:\n\
         cd crates/components/camel-component-wasm/tests/fixtures/conflicting-bind-guest\n\
         cargo build --target wasm32-wasip2",
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

/// Create a WasmSourceConsumer for the given guest wasm with a short timeout.
fn make_consumer(
    uri: String,
    wasm_path: PathBuf,
    guest_config: Vec<(String, String)>,
) -> WasmSourceConsumer {
    WasmSourceConsumer::new(
        wasm_path,
        uri,
        WasmConfig {
            timeout_secs: 5,
            ..WasmConfig::default()
        },
        guest_config,
        Arc::new(camel_component_api::NoOpComponentContext),
    )
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

/// Runs an async future under a thread-local `fmt` subscriber capturing
/// output into a buffer; returns the future's result plus the captured
/// text. Same make-writer + `set_default` pattern as
/// `crates/camel-core/src/lifecycle/adapters/route_controller_trait_tests.rs`;
/// the guard lives across the await so events emitted inside the future
/// (e.g. the gate's warn) land in the buffer. Safe here because
/// `#[tokio::test]` polls on one thread.
async fn capture_logs<F: Future>(fut: F) -> (F::Output, String) {
    struct CaptureWriter {
        buf: Arc<std::sync::Mutex<Vec<u8>>>,
    }
    impl std::io::Write for CaptureWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.buf.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
        type Writer = CaptureWriter;
        fn make_writer(&'a self) -> Self::Writer {
            CaptureWriter {
                buf: Arc::clone(&self.buf),
            }
        }
    }
    let buf = Arc::new(std::sync::Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt::Subscriber::builder()
        .with_writer(CaptureWriter {
            buf: Arc::clone(&buf),
        })
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let output = fut.await;
    drop(_guard);
    let captured =
        String::from_utf8(buf.lock().unwrap().clone()).expect("captured output must be UTF-8");
    (output, captured)
}

// ─── Tests ──────────────────────────────────────────────────────────────────

/// Operator `bind` equal to the guest-declared bind → one listener, start
/// succeeds.
///
/// Prerequisites: pre-built webhook guest wasm.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn matching_binds_produce_one_listener() {
    let port = free_port().await;
    let bind = format!("127.0.0.1:{port}");
    let uri = format!("wasm:webhook.wasm?bind={bind}&path=/webhook");
    let guest_config = vec![
        ("bind".into(), bind.clone()),
        ("path".into(), "/webhook".into()),
    ];

    let mut consumer = make_consumer(uri, require_webhook_guest_wasm(), guest_config);
    let (ctx, _rx, _cancel) = make_consumer_context("matching-binds", 16);

    consumer
        .start(ctx)
        .await
        .expect("matching operator/guest binds must start");

    // Exactly one listener exists for the resolved bind: connecting once
    // succeeds against it.
    wait_for_bind(port, BIND_WAIT).await;

    tokio::time::timeout(STOP_TIMEOUT, consumer.stop())
        .await
        .expect("stop timed out")
        .expect("stop should succeed");
}

/// Operator `bind` differing from the (independent) guest-declared bind →
/// start() fails naming both addresses, and nothing is bound.
///
/// Prerequisites: pre-built conflicting-bind guest wasm.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn conflicting_binds_fail_before_socket() {
    let port_a = free_port().await;
    let port_b = free_port().await;
    let guest_bind = format!("0.0.0.0:{port_a}");
    let operator_bind = format!("127.0.0.1:{port_b}");
    let uri = format!("wasm:conflict.wasm?bind={operator_bind}");

    // The conflicting-bind guest derives its declared bind ONLY from
    // `conflict_port` (never from `bind`), so the two are independent.
    let guest_config = vec![
        ("bind".into(), operator_bind.clone()),
        ("conflict_port".into(), port_a.to_string()),
    ];

    let mut consumer = make_consumer(uri, require_conflict_guest_wasm(), guest_config);
    let (ctx, _rx, _cancel) = make_consumer_context("conflicting-binds", 16);

    let err = consumer
        .start(ctx)
        .await
        .expect_err("conflicting operator/guest binds must fail start()");
    let msg = err.to_string();
    assert!(
        msg.contains(&guest_bind),
        "error must name the guest bind '{guest_bind}': {msg}"
    );
    assert!(
        msg.contains(&operator_bind),
        "error must name the operator bind '{operator_bind}': {msg}"
    );

    // The refusal happens before the socket is bound: nothing listens on
    // the operator bind.
    assert!(
        TcpStream::connect(("127.0.0.1", port_b)).await.is_err(),
        "nothing must be bound at the operator bind {operator_bind}"
    );
}

/// Guest-declared non-loopback bind without operator `bind` is gated.
///
/// No `?bind=` query param is present; the effective bind comes solely from
/// the guest's declared `0.0.0.0:<portA>` (derived from `conflict_port`).
/// This exercises the no-operator-bind branch feeding the same exposure gate.
///
/// Prerequisites: pre-built conflicting-bind guest wasm.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn guest_only_non_loopback_bind_gated() {
    let port_a = free_port().await;
    let guest_bind = format!("0.0.0.0:{port_a}");
    WasmSourceBindAcks::global().set(HashMap::new());

    // No `bind` query param — operator bind absent.
    let uri = "wasm:conflict.wasm".to_string();
    let guest_config = vec![("conflict_port".into(), port_a.to_string())];

    let mut consumer = make_consumer(uri, require_conflict_guest_wasm(), guest_config);
    let (ctx, _rx, _cancel) = make_consumer_context("guest-only-gate", 16);

    let err = consumer
        .start(ctx)
        .await
        .expect_err("guest-only non-loopback bind must be refused without ack");
    let msg = err.to_string();
    assert!(
        msg.contains(&guest_bind),
        "error must name the bind '{guest_bind}': {msg}"
    );
}

/// Non-loopback Public bind (no security wiring): refused without ack,
/// starts with ack — and the acknowledged start emits a warn naming the
/// bind and its exposed Public-route count.
///
/// One sequential test: the ack store is a single process-global map, so
/// the ack-mutating phases must not run in parallel with each other.
///
/// Prerequisites: pre-built webhook guest wasm.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn non_loopback_public_gate_with_and_without_ack() {
    // Phase 1 — no ack: start() fails naming the bind.
    let port_a = free_port().await;
    let bind_a = format!("0.0.0.0:{port_a}");
    WasmSourceBindAcks::global().set(HashMap::new());

    let uri = format!("wasm:webhook.wasm?bind={bind_a}");
    let guest_config = vec![("bind".into(), bind_a.clone())];
    let mut consumer = make_consumer(uri, require_webhook_guest_wasm(), guest_config);
    let (ctx, _rx, _cancel) = make_consumer_context("gate-phase-1", 16);

    let err = consumer
        .start(ctx)
        .await
        .expect_err("non-loopback Public bind must be refused without ack");
    assert!(
        err.to_string().contains(&bind_a),
        "error must name the bind '{bind_a}': {err}"
    );

    // Phase 2 — ack for a fresh bind: start() succeeds and warns.
    let port_b = free_port().await;
    let bind_b = format!("0.0.0.0:{port_b}");
    let mut acks = HashMap::new();
    acks.insert(bind_b.clone(), true);
    WasmSourceBindAcks::global().set(acks);

    let uri = format!("wasm:webhook.wasm?bind={bind_b}");
    let guest_config = vec![("bind".into(), bind_b.clone())];
    let mut consumer = make_consumer(uri, require_webhook_guest_wasm(), guest_config);
    let (ctx, _rx, _cancel) = make_consumer_context("gate-phase-2", 16);

    let (start_result, captured) = capture_logs(consumer.start(ctx)).await;
    start_result.expect("acknowledged non-loopback Public bind must start");
    assert!(
        captured.contains(&bind_b),
        "warn must name the bind '{bind_b}': {captured}"
    );
    assert!(
        captured.contains("public_routes=1"),
        "warn must state the exposed Public-route count: {captured}"
    );

    tokio::time::timeout(STOP_TIMEOUT, consumer.stop())
        .await
        .expect("stop timed out")
        .expect("stop should succeed");
}

/// Loopback Public bind: no ack needed, start() succeeds.
///
/// The ack map is deliberately NOT reset here: no test in this target ever
/// acknowledges a loopback bind, so `acknowledged("127.0.0.1:<port>")` is
/// false under every interleaving — and mutating the global map from this
/// test would race the sequential phases of
/// `non_loopback_public_gate_with_and_without_ack` under the parallel test
/// harness.
///
/// Prerequisites: pre-built webhook guest wasm.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn loopback_public_needs_no_ack() {
    let port = free_port().await;
    let bind = format!("127.0.0.1:{port}");
    let uri = format!("wasm:webhook.wasm?bind={bind}");
    let guest_config = vec![("bind".into(), bind)];

    let mut consumer = make_consumer(uri, require_webhook_guest_wasm(), guest_config);
    let (ctx, _rx, _cancel) = make_consumer_context("loopback-public", 16);

    consumer
        .start(ctx)
        .await
        .expect("loopback Public bind must start without ack");
    wait_for_bind(port, BIND_WAIT).await;

    tokio::time::timeout(STOP_TIMEOUT, consumer.stop())
        .await
        .expect("stop timed out")
        .expect("stop should succeed");
}
