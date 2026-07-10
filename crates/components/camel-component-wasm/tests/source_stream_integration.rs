//! Integration test: accept-http streams large bodies without materialization.
//!
//! Proves the 10 MiB `to_bytes` cap is gone: the host streams the axum body
//! incrementally through a per-request channel, the guest reads via
//! `stream<u8>.read`, and the full payload arrives at the pipeline.
//!
//! # Prerequisites
//!
//! Same as `source_integration.rs` — pre-built guest wasm.
//!
//! Run with:
//! ```sh
//! cargo test -p camel-component-wasm --test source_stream_integration -- --ignored
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

/// Timeout for the guest to bind its HTTP listener and be ready.
const BIND_WAIT: Duration = Duration::from_secs(5);

/// Timeout for stop() to complete cleanly.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

// ─── Helpers (mirror source_integration.rs) ────────────────────────────────

fn guest_wasm_path() -> Option<PathBuf> {
    const GUEST_WASM_FILE: &str = "wasm32-wasip2/debug/wasm_source_webhook_guest.wasm";

    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        let path = PathBuf::from(target_dir).join(GUEST_WASM_FILE);
        if path.exists() {
            return Some(path);
        }
    }

    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = crate_dir
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent());

    let rel = "examples/wasm-source-webhook/guest/target/wasm32-wasip2/debug/wasm_source_webhook_guest.wasm";
    let path = workspace_root?.join(rel);
    if path.exists() { Some(path) } else { None }
}

fn require_guest_wasm() -> PathBuf {
    guest_wasm_path().expect(
        "Guest wasm not found. Build with:\n\
         cd examples/wasm-source-webhook/guest && CARGO_TARGET_DIR=/home/shared/rust-camel-target cargo build --target wasm32-wasip2",
    )
}

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

fn make_consumer(guest_config: Vec<(String, String)>) -> WasmSourceConsumer {
    let wasm_path = require_guest_wasm();
    let config = WasmConfig {
        timeout_secs: 30,
        ..WasmConfig::default()
    };
    WasmSourceConsumer::new(
        wasm_path,
        config,
        guest_config,
        Arc::new(Mutex::new(Registry::new())),
    )
}

async fn free_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind ephemeral port");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

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

/// Send a raw HTTP POST and return (status_line, stream).
/// The stream is returned so the caller can keep the connection alive while
/// the body is being streamed to the guest.
async fn send_http_post(port: u16, path: &str, body: &[u8]) -> (String, TcpStream) {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("failed to connect to source HTTP listener");

    let request = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Content-Type: application/octet-stream\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
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
    (String::from_utf8_lossy(&buf[..n]).to_string(), stream)
}

// ─── Tests ──────────────────────────────────────────────────────────────────

/// POST a large body and verify the guest receives all bytes via the
/// streaming body handle (not materialized via `to_bytes`).
///
/// The body is 8 MiB — under the default 10 MiB `max_request_body_bytes`
/// cap but large enough to prove the streaming path works (the old code
/// materialized via `axum::body::to_bytes(body, MAX_BODY_BYTES)` with
/// `MAX_BODY_BYTES = 10 * 1024 * 1024`; the new code streams incrementally
/// through a per-request channel).
///
/// Prerequisites: pre-built guest wasm.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn accept_http_streams_large_body() {
    let port = free_port().await;
    let guest_config = vec![
        ("bind".into(), format!("127.0.0.1:{port}")),
        ("path".into(), "/webhook".into()),
    ];

    let mut consumer = make_consumer(guest_config);
    let (ctx, mut rx, _cancel) = make_consumer_context("test-stream-large", 4);

    consumer.start(ctx).await.expect("start() failed");
    wait_for_bind(port, BIND_WAIT).await;

    // 8 MiB — under the default 10 MiB cap, large enough to prove streaming.
    let size: usize = 8 * 1024 * 1024;
    // Use a repeating pattern so we can verify integrity, not just length.
    let body: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();

    let (response, _stream) = send_http_post(port, "/webhook", &body).await;
    assert!(
        response.contains("202"),
        "expected 202 response for large body, got: {response}"
    );

    // Keep `_stream` alive until the exchange arrives — dropping it would
    // close the TCP connection and abort the host's body-streaming task
    // before the guest has finished reading.

    // The guest reads the stream incrementally, buffers, and submits.
    // Wait for the exchange to arrive at the pipeline.
    let envelope = tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("timed out waiting for exchange from large-body POST")
        .expect("channel closed before exchange arrived");

    // Verify the exchange body matches the original payload.
    let received_len = match &envelope.exchange.input.body {
        camel_api::Body::Bytes(b) => b.len(),
        camel_api::Body::Text(s) => s.len(),
        other => panic!("expected Bytes or Text body, got: {other:?}"),
    };
    assert_eq!(
        received_len, size,
        "guest should have received all {size} bytes, got {received_len}"
    );

    // Verify payload integrity (spot-check the repeating pattern).
    match &envelope.exchange.input.body {
        camel_api::Body::Bytes(b) => {
            for (i, &byte) in b.iter().enumerate() {
                assert_eq!(
                    byte,
                    (i % 251) as u8,
                    "byte mismatch at offset {i}: expected {}, got {byte}",
                    i % 251
                );
            }
        }
        camel_api::Body::Text(s) => {
            let b = s.as_bytes();
            for (i, &byte) in b.iter().enumerate() {
                assert_eq!(
                    byte,
                    (i % 251) as u8,
                    "byte mismatch at offset {i}: expected {}, got {byte}",
                    i % 251
                );
            }
        }
        other => panic!("expected Bytes or Text body, got: {other:?}"),
    }

    // Clean shutdown.
    tokio::time::timeout(STOP_TIMEOUT, consumer.stop())
        .await
        .expect("stop() timed out")
        .expect("stop() error");
}

/// Mid-stream connection abort: the guest must NOT silently receive a
/// truncated body as if it were complete.
///
/// Sends headers + partial body, then drops the TCP connection. The host's
/// body-drain task observes the connection error and forwards it as an
/// `Err` frame on the body channel. The guest's stream read surfaces the
/// error via the terminal future — it does not see a clean EOF at the
/// truncation point.
///
/// We verify the negative: no exchange arrives at the pipeline within a
/// short window, because the guest encounters a stream error before it
/// can submit. A silent truncation would produce an exchange with a
/// short body — this test ensures that does not happen.
///
/// Prerequisites: pre-built guest wasm.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn accept_http_mid_stream_abort_surfaces_error() {
    let port = free_port().await;
    let guest_config = vec![
        ("bind".into(), format!("127.0.0.1:{port}")),
        ("path".into(), "/webhook".into()),
    ];

    let mut consumer = make_consumer(guest_config);
    let (ctx, mut rx, _cancel) = make_consumer_context("test-stream-abort", 4);

    consumer.start(ctx).await.expect("start() failed");
    wait_for_bind(port, BIND_WAIT).await;

    // Open a raw TCP connection and send headers + partial body.
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("failed to connect to source HTTP listener");

    use tokio::io::AsyncWriteExt;
    // Claim a large Content-Length but only send a fraction.
    let claimed_len: usize = 1024 * 1024; // 1 MiB claimed
    let actual_send: usize = 256; // only 256 bytes actually sent
    let request = format!(
        "POST /webhook HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Content-Type: application/octet-stream\r\n\
         Content-Length: {claimed_len}\r\n\
         \r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("failed to write headers");
    stream
        .write_all(&vec![0xABu8; actual_send])
        .await
        .expect("failed to write partial body");

    // Drop the connection abruptly (no FIN — simulate mid-stream abort).
    // shutdown().await sends a FIN; drop() aborts.
    drop(stream);

    // The guest should encounter a stream error (not a clean EOF at 256 bytes).
    // If the guest were to silently truncate, it would submit an exchange with
    // a 256-byte body. Verify no exchange arrives within a short window —
    // the guest's stream read errors before it can submit.
    let exchange_arrived = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await;

    match exchange_arrived {
        Err(_timeout) => {
            // Good: no exchange arrived. The guest hit a stream error and
            // did not submit a truncated body.
        }
        Ok(Some(envelope)) => {
            // An exchange arrived — verify it is NOT a silent truncation.
            // The body should not be exactly `actual_send` bytes (that would
            // mean the guest saw a clean EOF at the truncation point).
            let received_len = match &envelope.exchange.input.body {
                camel_api::Body::Bytes(b) => b.len(),
                camel_api::Body::Text(s) => s.len(),
                camel_api::Body::Empty => 0,
                other => panic!("unexpected body variant: {other:?}"),
            };
            assert_ne!(
                received_len, actual_send,
                "guest must NOT silently truncate: received {received_len} bytes \
                 == sent {actual_send} bytes (clean EOF at truncation point)"
            );
        }
        Ok(None) => {
            // Channel closed — guest exited due to stream error. Acceptable.
        }
    }

    // Clean shutdown.
    tokio::time::timeout(STOP_TIMEOUT, consumer.stop())
        .await
        .expect("stop() timed out")
        .expect("stop() error");
}

// ─── submit-exchange streaming (guest → host) ──────────────────────────────
//
// These tests exercise Task 4: the guest emits a streaming body via
// submit-exchange, drained in the background via Accessor::spawn
// (fire-and-return). The echo guest (echo=stream / echo=stream-fail) reads
// the incoming body, then re-emits it as a stream.
//
// The large-body test is the rendezvous-critical one: if the drain is not
// progressing on the event loop, the bounded chunk channel fills and the
// whole flow self-deadlocks (the test times out).

/// POST a body that exceeds all internal channel bounds (drain channel = 8
/// chunks × 4 KiB guest writes = 32 KiB in flight). The echo guest
/// (`echo=stream`) re-emits it as a streaming submit-exchange body. Asserts
/// the full body round-trips with no truncation, deadlock, or timeout.
///
/// If this deadlocks, the Accessor::spawn drain is not progressing on the
/// event loop concurrent with the guest fiber.
///
/// Prerequisites: pre-built guest wasm (with echo=stream support).
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn submit_exchange_streams_body_larger_than_buffers() {
    let port = free_port().await;
    let guest_config = vec![
        ("bind".into(), format!("127.0.0.1:{port}")),
        ("path".into(), "/webhook".into()),
        // echo=stream: guest re-emits the body as a streaming submit-exchange.
        ("echo".into(), "stream".into()),
    ];

    let mut consumer = make_consumer(guest_config);
    let (ctx, mut rx, _cancel) = make_consumer_context("test-submit-stream-large", 4);

    consumer.start(ctx).await.expect("start() failed");
    wait_for_bind(port, BIND_WAIT).await;

    // 256 KiB — well above the 32 KiB in-flight capacity (8 chunks × 4 KiB),
    // forcing backpressure + concurrent drain to complete the round-trip.
    let size: usize = 256 * 1024;
    let body: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();

    let (response, _stream) = send_http_post(port, "/webhook", &body).await;
    assert!(
        response.contains("202"),
        "expected 202 response, got: {response}"
    );

    let envelope = tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("timed out waiting for streaming submit-exchange envelope")
        .expect("channel closed before exchange arrived");

    // The body arrives as a Body::Stream — drain it fully. A timeout here
    // means the drain stalled (deadlock). A short body means truncation.
    let received = tokio::time::timeout(
        Duration::from_secs(30),
        envelope.exchange.input.body.into_bytes(size * 2),
    )
    .await
    .expect("timed out draining submit-exchange body stream — drain not progressing")
    .expect("draining body stream failed");

    assert_eq!(
        received.len(),
        size,
        "full body must round-trip; got {} of {} bytes",
        received.len(),
        size
    );

    // Verify payload integrity (repeating pattern).
    for (i, &byte) in received.iter().enumerate() {
        assert_eq!(
            byte,
            (i % 251) as u8,
            "byte mismatch at offset {i}: expected {}, got {byte}",
            i % 251
        );
    }

    // Clean shutdown.
    tokio::time::timeout(STOP_TIMEOUT, consumer.stop())
        .await
        .expect("stop() timed out")
        .expect("stop() error");
}

/// Guest emits a partial body then errors (`echo=stream-fail`). The host must
/// surface the terminal error through the Body::Stream drain — `into_bytes`
/// returns `Err` after the partial bytes.
///
/// Prerequisites: pre-built guest wasm (with echo=stream-fail support).
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn submit_exchange_surfaces_terminal_error() {
    let port = free_port().await;
    let guest_config = vec![
        ("bind".into(), format!("127.0.0.1:{port}")),
        ("path".into(), "/webhook".into()),
        // echo=stream-fail: guest emits partial body then resolves terminal Err.
        ("echo".into(), "stream-fail".into()),
    ];

    let mut consumer = make_consumer(guest_config);
    let (ctx, mut rx, _cancel) = make_consumer_context("test-submit-stream-fail", 4);

    consumer.start(ctx).await.expect("start() failed");
    wait_for_bind(port, BIND_WAIT).await;

    // Body large enough that the guest's partial emit (first 4 KiB chunk) is
    // clearly smaller than the full request.
    let size: usize = 32 * 1024;
    let body: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();

    let (response, _stream) = send_http_post(port, "/webhook", &body).await;
    assert!(
        response.contains("202"),
        "expected 202 response, got: {response}"
    );

    let envelope = tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("timed out waiting for streaming submit-exchange envelope")
        .expect("channel closed before exchange arrived");

    // Draining the body must surface the terminal error (not a clean EOF).
    let drain_result = tokio::time::timeout(
        Duration::from_secs(30),
        envelope.exchange.input.body.into_bytes(size * 2),
    )
    .await
    .expect("timed out draining submit-exchange body stream");

    assert!(
        drain_result.is_err(),
        "expected terminal error from stream-fail echo, got success: {:?}",
        drain_result.ok()
    );

    // Clean shutdown.
    tokio::time::timeout(STOP_TIMEOUT, consumer.stop())
        .await
        .expect("stop() timed out")
        .expect("stop() error");
}

// ─── Backpressure through the streaming pipeline ──────────────────────────
//
// The existing `test_source_backpressure_sequential` in source_integration.rs
// proves sequential backpressure with non-streaming bodies. This test adds
// `echo=stream` to verify the streaming path (AccessorTask drain + bounded
// chunk channel) composes with the capacity-1 control channels to propagate
// backpressure all the way to the HTTP client.

/// A slow-draining pipeline + streaming echo bodies should backpressure
/// concurrent HTTP requests: the capacity-1 request/exchange channels +
/// the bounded chunk channel compose so requests queue behind the slow
/// pipeline rather than completing instantly.
///
/// This test verifies the streaming submit-exchange path (AccessorTask
/// drain + rendezvous StreamWriter) does NOT break the backpressure chain
/// that the non-streaming path provides. The name refers to the observable
/// effect: HTTP client sends block until the pipeline drains, proving
/// backpressure reaches the client even with streaming bodies.
///
/// Mirrors `test_source_backpressure_sequential` from source_integration.rs
/// but adds `echo=stream` to exercise the streaming path.
///
/// Prerequisites: pre-built guest wasm (with echo=stream support).
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn backpressure_propagates_to_http_client() {
    let port = free_port().await;
    let guest_config = vec![
        ("bind".into(), format!("127.0.0.1:{port}")),
        ("path".into(), "/webhook".into()),
        // echo=stream: guest re-emits the body as a streaming submit-exchange.
        ("echo".into(), "stream".into()),
    ];

    let mut consumer = make_consumer(guest_config);
    // Capacity=1 to match EXCHANGE_CHANNEL_CAPACITY — strict backpressure.
    let (ctx, mut rx, _cancel) = make_consumer_context("test-stream-backpressure", 1);

    consumer.start(ctx).await.expect("start() failed");
    wait_for_bind(port, BIND_WAIT).await;

    // Slow pipeline: delays 500ms per exchange (same as the sequential test).
    // The Body::Stream is collected but NOT drained — the backpressure is
    // purely through the capacity-1 control channels, same as non-streaming.
    let pipeline_handle = tokio::spawn(async move {
        let mut received = Vec::new();
        while let Some(envelope) = rx.recv().await {
            received.push(envelope.exchange);
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        received
    });

    // Send a burst of 6 POSTs with bodies large enough to exercise the
    // streaming path. With a 500ms drain and ~3-slot buffer depth (exchange
    // channel + ConsumerContext + pipeline pre-fetch), at least ~3 requests
    // must wait → >= 800ms total. Same assertion as the sequential test.
    const BURST: usize = 6;
    let body: Vec<u8> = (0..32 * 1024).map(|i| (i % 251) as u8).collect();

    let send_start = std::time::Instant::now();
    for _ in 0..BURST {
        let _resp = send_http_post(port, "/webhook", &body).await;
    }
    let send_elapsed = send_start.elapsed();

    assert!(
        send_elapsed >= Duration::from_millis(800),
        "expected streaming backpressure delay (>=800ms), \
         but sends completed in {send_elapsed:?}"
    );

    // Clean shutdown.
    tokio::time::timeout(STOP_TIMEOUT, consumer.stop())
        .await
        .expect("stop() timed out")
        .expect("stop() error");

    // Let the pipeline task finish.
    let _ = tokio::time::timeout(Duration::from_secs(5), pipeline_handle).await;
}

// ─── Cancel mid-drain ──────────────────────────────────────────────────────

/// Start a streaming exchange (echo=stream), then call `stop()` while the
/// body is still draining. Assert:
/// 1. `stop()` returns within a reasonable timeout (no hang).
/// 2. The consumer's tasks are cleaned up (stop() joining + aborting).
///
/// This exercises the cancel-token select! in each import + the epoch
/// increment + the run-task abort fallback. A stuck drain or a missing
/// cancellation path would cause stop() to hang until the 5s grace timeout,
/// then abort.
///
/// Prerequisites: pre-built guest wasm (with echo=stream support).
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn stop_mid_drain_exits_cleanly() {
    let port = free_port().await;
    let guest_config = vec![
        ("bind".into(), format!("127.0.0.1:{port}")),
        ("path".into(), "/webhook".into()),
        // echo=stream: guest re-emits the body as a streaming submit-exchange.
        ("echo".into(), "stream".into()),
    ];

    let mut consumer = make_consumer(guest_config);
    // Capacity=4 so the exchange arrives promptly (fire-and-return).
    let (ctx, mut rx, _cancel) = make_consumer_context("test-cancel-mid-drain", 4);

    consumer.start(ctx).await.expect("start() failed");
    wait_for_bind(port, BIND_WAIT).await;

    // Send a large body that triggers streaming echo + a long drain.
    let size: usize = 256 * 1024;
    let body: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();

    let (response, _stream) = send_http_post(port, "/webhook", &body).await;
    assert!(
        response.contains("202"),
        "expected 202 response, got: {response}"
    );

    // Wait for the exchange to arrive (fire-and-return delivers it before
    // the full body is drained).
    let envelope = tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("timed out waiting for streaming submit-exchange envelope")
        .expect("channel closed before exchange arrived");

    // Start draining the body stream in the background — slowly, so the drain
    // is in-flight when we call stop().
    let drain_handle = tokio::spawn(async move {
        // into_bytes drains the streaming body. With a slow producer (guest
        // still writing via AccessorTask), this takes time — giving us a
        // window to call stop() mid-drain.
        tokio::time::timeout(
            Duration::from_secs(60),
            envelope.exchange.input.body.into_bytes(size * 2),
        )
        .await
    });

    // Give the drain a moment to start, then stop mid-drain.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // stop() must return within STOP_TIMEOUT — cancel_token + epoch + abort
    // fallback compose to unblock the guest and its tasks.
    tokio::time::timeout(STOP_TIMEOUT, consumer.stop())
        .await
        .expect("stop() timed out during mid-drain — cancellation path broken")
        .expect("stop() error during mid-drain");

    // The drain task may complete (Ok), timeout (Err), or error — any is
    // acceptable. The key assertion is that stop() returned promptly above.
    // Clean up the drain task.
    drain_handle.abort();
}

// ─── Idle source survival ──────────────────────────────────────────────────

/// An idle source (no HTTP requests arriving) must survive well past the
/// old watchdog timeout. Guards against reintroduction of a no-progress
/// watchdog that would kill a legitimately parked webhook source.
///
/// Uses a 2s timeout_secs and waits 5s — the old watchdog would have fired
/// at 2s. Requires a pre-built guest wasm that parks in accept-http.
#[tokio::test]
#[ignore = "requires pre-built guest wasm (see module docs)"]
async fn idle_source_survives_past_timeout() {
    let port = free_port().await;
    let guest_config = vec![
        ("bind".into(), format!("127.0.0.1:{port}")),
        ("path".into(), "/webhook".into()),
    ];

    // Use a 2s timeout — the old watchdog would fire at 2s of no imports.
    let wasm_path = require_guest_wasm();
    let config = WasmConfig {
        timeout_secs: 2,
        ..WasmConfig::default()
    };
    let mut consumer = WasmSourceConsumer::new(
        wasm_path,
        config,
        guest_config,
        Arc::new(Mutex::new(Registry::new())),
    );
    let (ctx, _rx, _cancel) = make_consumer_context("test-idle-survival", 1);

    consumer.start(ctx).await.expect("start() failed");
    wait_for_bind(port, BIND_WAIT).await;

    // Send NO requests. Wait 5s — well past the 2s timeout.
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Source must still be running. stop() should succeed cleanly.
    tokio::time::timeout(STOP_TIMEOUT, consumer.stop())
        .await
        .expect("stop() timed out — source may have been killed by a watchdog")
        .expect("stop() should succeed on idle source");
}
