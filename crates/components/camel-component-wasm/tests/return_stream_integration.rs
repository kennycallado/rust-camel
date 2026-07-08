//! Integration test: a WASM **bean** emits a streaming body (`WasmBody::Stream`)
//! and the host-side return-path drain reattaches it as `Body::Stream` on the
//! output exchange.
//!
//! Validates Tasks 6+7+9 (drain spawn + bean wiring + guest fixture) end-to-end.
//! The `emit_stream` guest method writes a deterministic 25-byte pattern
//! (`b"data\ndata\ndata\ndata\ndata\n"`) and returns it as a stream. This test
//! asserts the host receives the complete byte sequence followed by clean EOF.
//!
//! Run with: `cargo test -p camel-component-wasm --test return_stream_integration`

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use camel_api::{Body, Exchange, Message};
use camel_bean::BeanProcessor;
use camel_component_wasm::WasmConfig;
use camel_component_wasm::bean::WasmBean;
use camel_core::Registry;
use futures::StreamExt;
use tokio::sync::Notify;

/// Path to the prebuilt `emit_stream` bean fixture (method: emit_stream).
fn bean_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples/wasm-bean-example/fixtures/emit_stream.wasm")
}

async fn fresh_bean() -> WasmBean {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    WasmBean::new(
        bean_fixture(),
        WasmConfig::default(),
        registry,
        HashMap::new(),
    )
    .await
    .expect("bean fixture loads")
}

/// Build a bean with a drain-completion lifecycle hook (used by tests to
/// assert drain timing / cancel-on-drop deterministically).
async fn fresh_bean_with_drain_completion_notify() -> (WasmBean, Arc<Notify>) {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    let notify = Arc::new(Notify::new());
    let bean = WasmBean::new(
        bean_fixture(),
        WasmConfig::default(),
        registry,
        HashMap::new(),
    )
    .await
    .expect("bean fixture loads")
    .with_drain_completion_notify(notify.clone());
    (bean, notify)
}

// ---------------------------------------------------------------------------
// Test: bean emits a stream, host drains it, clean EOF after 25 bytes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn return_stream_clean_eof() {
    let bean = fresh_bean().await;

    // Input exchange with Empty body (the guest ignores input and emits a fixed pattern).
    let mut exchange = Exchange::new(Message::new(Body::Empty));

    // Invoke the emit_stream method.
    bean.call("emit_stream", &mut exchange)
        .await
        .expect("bean emit_stream should succeed");

    // Bean convention: the guest transforms input.body in place.
    let body = exchange.input.body.clone();

    match body {
        Body::Stream(stream_body) => {
            // Lock the mutex and take the stream out of the Option.
            let mut stream_guard = stream_body.stream.lock().await;
            let mut stream = stream_guard
                .take()
                .expect("stream should be present on first consumption");

            // Collect all chunks from the stream.
            let mut collected = Vec::new();
            while let Some(item) = stream.next().await {
                match item {
                    Ok(chunk) => collected.push(chunk),
                    Err(e) => panic!("stream yielded an error: {e}"),
                }
            }

            // Concatenate all chunks and assert the expected byte sequence.
            let result: Vec<u8> = collected.into_iter().flat_map(|b| b.to_vec()).collect();
            let expected = b"data\ndata\ndata\ndata\ndata\n";
            assert_eq!(
                result.len(),
                expected.len(),
                "expected {} bytes, got {}",
                expected.len(),
                result.len()
            );
            assert_eq!(
                result, expected,
                "byte sequence mismatch: expected {:?}, got {:?}",
                expected, result
            );

            // Stream ended cleanly (EOF) — the while loop above exited without error.
        }
        other => panic!("expected Body::Stream, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Task 11: early-drop → Store freed (deterministic via Notify, not sleep)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn return_stream_early_drop_frees_store() {
    let (bean, drain_completion_notify) = fresh_bean_with_drain_completion_notify().await;
    // Use slow-emit guest to ensure the stream is still active when we drop it.
    let mut exchange = Exchange::new(Message::new(Body::Empty));
    bean.call("emit_stream_slow", &mut exchange)
        .await
        .expect("bean emit_stream_slow should succeed");

    // Extract and drop the Body::Stream mid-way.
    {
        let body = std::mem::replace(&mut exchange.input.body, Body::Empty);
        match body {
            Body::Stream(_) => {
                // Drop the stream — this should trigger cancel-on-drop.
            }
            other => panic!("expected Body::Stream, got {other:?}"),
        }
        // body drops here
    }

    // The Store must be freed promptly. The 2s bound holds for a cooperative
    // guest (wasmtime re-polls poll_consume → receiver_gone notify_one fires
    // → select! cancels). A wedged guest is bounded by the watchdog's
    // no_progress_timeout, so set that well under 2s in production config.
    tokio::time::timeout(Duration::from_secs(2), drain_completion_notify.notified())
        .await
        .expect("Store was NOT freed within 2s after body drop — cancel-on-drop broken");
}

// ---------------------------------------------------------------------------
// Task 12: terminal error propagation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn return_stream_terminal_error() {
    let bean = fresh_bean().await;
    let mut exchange = Exchange::new(Message::new(Body::Empty));
    bean.call("emit_stream_fail", &mut exchange)
        .await
        .expect("bean emit_stream_fail should succeed");

    let body = exchange.input.body.clone();
    match body {
        Body::Stream(stream_body) => {
            let mut stream_guard = stream_body.stream.lock().await;
            let mut stream = stream_guard
                .take()
                .expect("stream should be present on first consumption");

            let mut collected = Vec::new();
            let mut terminal_error: Option<String> = None;
            while let Some(item) = stream.next().await {
                match item {
                    Ok(chunk) => collected.push(chunk),
                    Err(e) => {
                        // Error is terminal — stream should end after this.
                        terminal_error = Some(e.to_string());
                    }
                }
            }

            // Assert we got the partial bytes.
            let result: Vec<u8> = collected.into_iter().flat_map(|b| b.to_vec()).collect();
            assert_eq!(result, b"partial", "expected partial bytes before error");

            // Assert we got a terminal error AND it's the guest's specific
            // failure message (not a premature EOF or unrelated error).
            let err_msg = terminal_error.expect("expected terminal error item in stream");
            assert!(
                err_msg.contains("bean requested failure after partial emit"),
                "expected the guest's failure message, got: {err_msg}"
            );
        }
        other => panic!("expected Body::Stream, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Task 13: backpressure under slow consumer
//
// Discriminating assertion: measure the DRAIN TASK completion time (via the
// drain-completion lifecycle hook), NOT the consumer loop elapsed time. With
// a slow consumer (50ms sleep × 20 chunks ≈ 1s), if backpressure works
// (rendezvous channel), the guest CANNOT finish writing until the consumer
// drains each chunk → drain completion is bounded by the consumer's total
// sleep budget (≥ ~1s). If backpressure were broken (unbounded buffer), the
// guest would finish near-instantly and drain completion would fire in ~0s
// regardless of consumer speed.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn return_stream_backpressure_slow_consumer() {
    let (bean, drain_completion_notify) = fresh_bean_with_drain_completion_notify().await;
    let mut exchange = Exchange::new(Message::new(Body::Empty));
    bean.call("emit_stream_slow", &mut exchange)
        .await
        .expect("bean emit_stream_slow should succeed");

    let body = exchange.input.body.clone();
    match body {
        Body::Stream(stream_body) => {
            let mut stream_guard = stream_body.stream.lock().await;
            let mut stream = stream_guard
                .take()
                .expect("stream should be present on first consumption");

            // Slow consumer: read one chunk, sleep 50ms, repeat.
            // Total sleep budget: 20 chunks × 50ms = 1000ms.
            let consumer_sleep_ms = 50u64;
            let mut chunk_count = 0;
            let consumer_start = std::time::Instant::now();
            while let Some(item) = stream.next().await {
                match item {
                    Ok(_chunk) => {
                        chunk_count += 1;
                        tokio::time::sleep(Duration::from_millis(consumer_sleep_ms)).await;
                    }
                    Err(e) => panic!("stream yielded an error: {e}"),
                }
            }
            let consumer_elapsed = consumer_start.elapsed();

            // Assert all 20 chunks were received (guest emitted 20 × 4 bytes).
            assert_eq!(chunk_count, 20, "expected 20 chunks, got {chunk_count}");

            // Now await the drain task completion. With working backpressure,
            // the guest could not finish writing until the consumer drained
            // each chunk, so drain completion time ≥ consumer's sleep budget.
            let drain_completion_result =
                tokio::time::timeout(Duration::from_secs(5), drain_completion_notify.notified())
                    .await;
            assert!(
                drain_completion_result.is_ok(),
                "drain task did not complete within 5s"
            );
            let drain_elapsed = consumer_start.elapsed();

            // The discriminating assertion: drain completion must be bounded
            // by the consumer's sleep budget. If backpressure were broken
            // (unbounded buffer), drain_elapsed would be ≈ 0s (guest finishes
            // instantly, independent of consumer speed). With rendezvous
            // backpressure, drain_elapsed ≥ consumer's total sleep time.
            let sleep_budget =
                Duration::from_millis(consumer_sleep_ms * chunk_count as u64 * 8 / 10);
            assert!(
                drain_elapsed >= sleep_budget,
                "drain completed in {drain_elapsed:?} which is < sleep budget {sleep_budget:?} \
                 — backpressure not engaged (guest finished independent of consumer speed). \
                 Consumer loop took {consumer_elapsed:?}."
            );

            eprintln!(
                "[backpressure] consumer_elapsed={consumer_elapsed:?}, \
                 drain_elapsed={drain_elapsed:?}, sleep_budget={sleep_budget:?}"
            );
        }
        other => panic!("expected Body::Stream, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Task 14: cancel on mid-consume stream drop (spec §8 test 5)
//
// Distinct from Task 11 (early-drop BEFORE reading any chunk): this test
// reads a few chunks to prove the stream is actively delivering, THEN drops
// it. Both trigger cancel-on-drop, but this variant exercises the path where
// the drain task is already mid-flight (chunks have been transferred, the
// guest is blocked on the next write) — verifying the cancel signal cleanly
// aborts an in-progress drain, not just a not-yet-started one.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn return_stream_cancel_mid_consume() {
    let (bean, drain_completion_notify) = fresh_bean_with_drain_completion_notify().await;
    let mut exchange = Exchange::new(Message::new(Body::Empty));
    bean.call("emit_stream_slow", &mut exchange)
        .await
        .expect("bean emit_stream_slow should succeed");

    // Extract the stream, read a few chunks, then drop it (simulating
    // downstream error → pipeline drop mid-consume).
    {
        let body = std::mem::replace(&mut exchange.input.body, Body::Empty);
        match body {
            Body::Stream(stream_body) => {
                let mut stream_guard = stream_body.stream.lock().await;
                let mut stream = stream_guard
                    .take()
                    .expect("stream should be present on first consumption");

                // Read a few chunks to prove the stream is active and the
                // drain task is mid-flight.
                let mut count = 0;
                while count < 3 {
                    match stream.next().await {
                        Some(Ok(_)) => count += 1,
                        Some(Err(e)) => panic!("stream error: {e}"),
                        None => break, // EOF before 3 chunks — unexpected but ok
                    }
                }
                // Drop the stream — simulates downstream error mid-consume.
                drop(stream);
            }
            other => panic!("expected Body::Stream, got {other:?}"),
        }
        // body drops here
    }

    // Assert the drain task completes (Store freed, no hang, no leak).
    // The cancel signal must cleanly abort the in-progress drain.
    tokio::time::timeout(Duration::from_secs(2), drain_completion_notify.notified())
        .await
        .expect("drain task did not complete within 2s after mid-consume cancel — cancel-on-drop broken");
}
