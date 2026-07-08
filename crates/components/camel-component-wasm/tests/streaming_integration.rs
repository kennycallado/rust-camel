//! End-to-end integration tests for the WASM streaming-body host bridge.
//!
//! These tests drive the streaming byte-counter guest
//! (`examples/wasm-streaming-plugin/fixtures/streaming-plugin.wasm`) through
//! [`WasmRuntime::process_streaming_exchange`] — the full
//! `WasmProducer::call` → `process_streaming_exchange` → `BoxStreamProducer`
//! → `StreamReader` pipeline — exercising the host↔guest `stream<u8>`
//! contract: basic throughput, large-stream round-trip, mid-stream
//! error propagation, the max-bytes cap, cancellation, the no-progress
//! watchdog, and progressing-stream watchdog survival.
//!
//! Run with: `cargo test -p camel-component-wasm --test streaming_integration -- --nocapture`

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use camel_api::{Body, CamelError, Exchange, Message, StreamBody, StreamMetadata};
use camel_component_wasm::StateStore;
use camel_component_wasm::WasmConfig;
use camel_component_wasm::WasmError;
use camel_component_wasm::bindings::camel::plugin::types::{WasmBody, WasmExchange};
use camel_component_wasm::runtime::WasmRuntime;
use camel_core::Registry;
use futures::stream::{self, BoxStream, StreamExt};
use tokio_util::sync::CancellationToken;

/// Path to the prebuilt streaming byte-counter guest fixture.
fn streaming_plugin_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples/wasm-streaming-plugin/fixtures/streaming-plugin.wasm")
}

const MB: usize = 1024 * 1024;
/// Default watchdog window for tests that should never time out.
const GENEROUS_TIMEOUT: Duration = Duration::from_secs(60);
/// Default per-stream byte cap large enough to admit any test payload.
const GENEROUS_MAX_BYTES: u64 = 64 * MB as u64;

/// Wrap a byte stream in an `Exchange` whose input body is `Body::Stream`.
fn stream_exchange(stream: BoxStream<'static, Result<Bytes, CamelError>>) -> Exchange {
    let body = Body::Stream(StreamBody {
        stream: Arc::new(tokio::sync::Mutex::new(Some(stream))),
        metadata: StreamMetadata::default(),
    });
    Exchange::new(Message::new(body))
}

/// Run a streaming exchange through the guest and return the WIT result.
async fn process_streaming(
    runtime: &WasmRuntime,
    exchange: Exchange,
    cancel: CancellationToken,
    max_bytes: u64,
    no_progress_timeout: Duration,
) -> Result<WasmExchange, WasmError> {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    let state_store = StateStore::new();
    let properties: HashMap<String, serde_json::Value> = HashMap::new();
    let sem = Arc::new(tokio::sync::Semaphore::new(1));
    let pending_permit = sem.try_acquire_owned().expect("semaphore just created"); // allow-unwrap
    runtime
        .process_streaming_exchange(
            registry,
            properties,
            state_store,
            exchange,
            pending_permit,
            cancel,
            max_bytes,
            no_progress_timeout,
        )
        .await
        .map(|sr| sr.exchange)
}

/// Extract the byte count from a `"streamed {n} bytes"` output body.
fn extract_count(result: &WasmExchange) -> u64 {
    let out = result.output.as_ref().expect("guest must populate output");
    match &out.body {
        WasmBody::Text(s) => s
            .strip_prefix("streamed ")
            .and_then(|s| s.strip_suffix(" bytes"))
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or_else(|| panic!("unexpected output body text: {s}")),
        other => panic!("expected Text output body, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Test 1: basic streaming end-to-end
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_basic_one_mb_round_trip() {
    let runtime = WasmRuntime::new(&streaming_plugin_path(), WasmConfig::default())
        .await
        .expect("runtime loads streaming plugin");

    let bytes = Bytes::from(vec![b'x'; MB]);
    let stream = stream::iter(vec![Ok::<_, CamelError>(bytes)]).boxed();
    let exchange = stream_exchange(stream);

    let result = process_streaming(
        &runtime,
        exchange,
        CancellationToken::new(),
        GENEROUS_MAX_BYTES,
        GENEROUS_TIMEOUT,
    )
    .await
    .expect("streaming exchange should succeed");

    assert_eq!(extract_count(&result), MB as u64);
    // Confirm the exact contract string the guest emits.
    let out = result.output.as_ref().unwrap();
    assert!(
        matches!(&out.body, WasmBody::Text(s) if s == "streamed 1048576 bytes"),
        "guest output contract mismatch: {:?}",
        out.body
    );
}

// ---------------------------------------------------------------------------
// Test 2: empty stream (immediate EOF)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_empty_stream_round_trip() {
    let runtime = WasmRuntime::new(&streaming_plugin_path(), WasmConfig::default())
        .await
        .expect("runtime loads streaming plugin");

    let stream = stream::iter(vec![] as Vec<Result<Bytes, CamelError>>).boxed();
    let exchange = stream_exchange(stream);

    let result = process_streaming(
        &runtime,
        exchange,
        CancellationToken::new(),
        GENEROUS_MAX_BYTES,
        GENEROUS_TIMEOUT,
    )
    .await
    .expect("empty streaming exchange should succeed");

    assert_eq!(extract_count(&result), 0);
    let out = result.output.as_ref().unwrap();
    assert!(
        matches!(&out.body, WasmBody::Text(s) if s == "streamed 0 bytes"),
        "guest output contract mismatch: {:?}",
        out.body
    );
}

// ---------------------------------------------------------------------------
// Test 3: >10 MB stream — large-stream round-trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_large_round_trip() {
    let runtime = WasmRuntime::new(&streaming_plugin_path(), WasmConfig::default())
        .await
        .expect("runtime loads streaming plugin");

    // Large stream round-trip. The streaming-plugin guest reads in 4 KiB
    // chunks (see `examples/wasm-streaming-plugin/guest/src/lib.rs`), counting bytes
    // without retaining them. This test verifies correctness of large streams; a
    // dedicated memory harness would be needed to assert RSS bounds.
    let chunk = Bytes::from(vec![b'y'; MB]);
    let chunks: Vec<Result<Bytes, CamelError>> = (0..20).map(|_| Ok(chunk.clone())).collect();
    let stream = stream::iter(chunks).boxed();
    let exchange = stream_exchange(stream);

    let result = process_streaming(
        &runtime,
        exchange,
        CancellationToken::new(),
        GENEROUS_MAX_BYTES,
        GENEROUS_TIMEOUT,
    )
    .await
    .expect("large streaming exchange should succeed");

    let expected = 20 * MB as u64;
    assert_eq!(extract_count(&result), expected);
    let out = result.output.as_ref().unwrap();
    assert!(
        matches!(&out.body, WasmBody::Text(s) if s == "streamed 20971520 bytes"),
        "guest output contract mismatch: {:?}",
        out.body
    );
}

// ---------------------------------------------------------------------------
// Test 4: mid-stream error propagation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_mid_stream_error_propagates() {
    let runtime = WasmRuntime::new(&streaming_plugin_path(), WasmConfig::default())
        .await
        .expect("runtime loads streaming plugin");

    // Ship one good chunk, then surface a producer-side error. The host
    // producer funnels the error through the terminal future (not via a
    // stream trap), so the guest observes it via `handle.terminal.await?`
    // and returns a `WasmError` that the host maps to `GuestPanic`.
    let chunks: Vec<Result<Bytes, CamelError>> = vec![
        Ok(Bytes::from_static(b"good-bytes")),
        Err(CamelError::ProcessorError(
            "deliberate-stream-failure".into(),
        )),
    ];
    let stream = stream::iter(chunks).boxed();
    let exchange = stream_exchange(stream);

    let err = process_streaming(
        &runtime,
        exchange,
        CancellationToken::new(),
        GENEROUS_MAX_BYTES,
        GENEROUS_TIMEOUT,
    )
    .await
    .expect_err("mid-stream error must surface as WasmError");

    match err {
        WasmError::GuestPanic(msg) => {
            assert!(
                msg.contains("deliberate-stream-failure"),
                "error message must carry the producer error, got: {msg}"
            );
        }
        other => panic!("expected GuestPanic for mid-stream error, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Test 5: max-bytes limit cuts the stream off
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_max_bytes_limit_enforced() {
    let runtime = WasmRuntime::new(&streaming_plugin_path(), WasmConfig::default())
        .await
        .expect("runtime loads streaming plugin");

    // A 64 KB payload against a 1 KB cap: the producer detects overflow on
    // the first chunk (written > max_bytes) and ends the stream with an
    // overflow terminal, which the guest surfaces as a WasmError.
    let bytes = Bytes::from(vec![b'z'; 64 * 1024]);
    let stream = stream::iter(vec![Ok::<_, CamelError>(bytes)]).boxed();
    let exchange = stream_exchange(stream);

    let err = process_streaming(
        &runtime,
        exchange,
        CancellationToken::new(),
        1024,
        GENEROUS_TIMEOUT,
    )
    .await
    .expect_err("stream exceeding max_bytes must error");

    match err {
        WasmError::GuestPanic(msg) => {
            assert!(
                msg.contains("max-bytes"),
                "error must mention the max-bytes cap, got: {msg}"
            );
            assert!(
                msg.contains("1024"),
                "error must mention the cap value, got: {msg}"
            );
        }
        other => panic!("expected GuestPanic for max-bytes overflow, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Test 6: cancellation mid-stream
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_cancellation_cuts_stream_short() {
    let runtime = WasmRuntime::new(&streaming_plugin_path(), WasmConfig::default())
        .await
        .expect("runtime loads streaming plugin");

    // A slow stream: 64 KiB chunks every 20 ms, 200 chunks ≈ 12.8 MB over
    // ~4 s. We cancel the token ~120 ms in, after a handful of chunks have
    // shipped. The producer sees `cancel.is_cancelled()` and returns
    // `StreamResult::Dropped` with a *clean* terminal (None), so the guest
    // completes normally with a partial count rather than erroring.
    let chunk = Bytes::from(vec![b'q'; 64 * 1024]);
    let stream = stream::unfold(0u32, move |i| {
        let chunk = chunk.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if i < 200 {
                Some((Ok::<_, CamelError>(chunk), i + 1))
            } else {
                None
            }
        }
    })
    .boxed();
    let total: u64 = 200 * 64 * 1024;
    let exchange = stream_exchange(stream);

    let cancel = CancellationToken::new();
    let cancel_for_fut = cancel.clone();
    let proc = process_streaming(
        &runtime,
        exchange,
        cancel_for_fut,
        GENEROUS_MAX_BYTES,
        GENEROUS_TIMEOUT,
    );
    tokio::pin!(proc);

    // Let a few chunks ship, then trip cancellation.
    tokio::select! {
        biased;
        _ = tokio::time::sleep(Duration::from_millis(120)) => {
            cancel.cancel();
        }
        r = &mut proc => {
            panic!("stream finished before cancellation window ({r:?})");
        }
    }

    let result = (&mut proc)
        .await
        .expect("cancellation should yield a clean partial result, not an error");

    let n = extract_count(&result);
    assert!(
        n < total,
        "stream must be cut short by cancellation ({n} >= total {total})"
    );
    println!("cancellation: counted {n} of {total} bytes before cancel");
}

// ---------------------------------------------------------------------------
// Test 7: progressing stream survives tight watchdog
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_progressing_survives_tight_watchdog() {
    let runtime = WasmRuntime::new(&streaming_plugin_path(), WasmConfig::default())
        .await
        .expect("runtime loads streaming plugin");

    // Stream: 1024 bytes every 30ms for ~2s (~67 chunks, ~68 KB total)
    // no_progress_timeout: 100ms (tight — would fire if progress doesn't reset)
    let chunk = Bytes::from(vec![b'w'; 1024]);
    let stream = stream::unfold(0u32, move |i| {
        let chunk = chunk.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            if i < 67 {
                Some((Ok::<_, CamelError>(chunk), i + 1))
            } else {
                None
            }
        }
    })
    .boxed();
    let exchange = stream_exchange(stream);

    let result = process_streaming(
        &runtime,
        exchange,
        CancellationToken::new(),
        GENEROUS_MAX_BYTES,
        Duration::from_millis(100),
    )
    .await
    .expect("progressing stream must not trigger watchdog");

    let expected = 67 * 1024;
    assert_eq!(extract_count(&result), expected);
    let out = result.output.as_ref().unwrap();
    assert!(
        matches!(&out.body, WasmBody::Text(s) if s == "streamed 68608 bytes"),
        "guest output contract mismatch: {:?}",
        out.body
    );
}

// ---------------------------------------------------------------------------
// Test 8: no-progress watchdog fires on a stalled stream
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_no_progress_watchdog_fires() {
    let runtime = WasmRuntime::new(&streaming_plugin_path(), WasmConfig::default())
        .await
        .expect("runtime loads streaming plugin");

    // A stream that never yields — the producer's `poll_produce` stays
    // `Pending`, so `progress_notify` never fires and the guest is stuck in
    // `reader.read(..).await`. A short no-progress window trips the watchdog.
    let stream = stream::pending::<Result<Bytes, CamelError>>().boxed();
    let exchange = stream_exchange(stream);

    // Safety net: if the watchdog fails to fire, fail the test rather than
    // hang the suite.
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        process_streaming(
            &runtime,
            exchange,
            CancellationToken::new(),
            GENEROUS_MAX_BYTES,
            Duration::from_millis(100),
        ),
    )
    .await;

    let err = result
        .expect("watchdog must fire well within the 10s safety net")
        .expect_err("stalled stream must time out");

    match err {
        WasmError::GuestPanic(msg) => {
            assert!(
                msg.contains("no-progress") || msg.contains("stalled"),
                "error must be the no-progress watchdog, got: {msg}"
            );
        }
        other => panic!("expected GuestPanic watchdog timeout, got {other:?}"),
    }
}
