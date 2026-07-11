//! Integration test: a WASM **bean** receives a streaming body
//! (`Body::Stream`) and processes it without materialising the whole payload.
//!
//! Closes the rc-2lge gap: commit 0580c7ae wired the `stream<u8>` host bridge
//! only to the producer/plugin path (`process_streaming_exchange`). The bean
//! path (`WasmBean::call`) called `serde_bridge::exchange_to_wasm` outside
//! `run_concurrent`, which rejected `Body::Stream` before the guest ever ran.
//! This test drives the bean `count` method (reads a `stream<u8>` in 4 KiB
//! chunks, tallies bytes) end-to-end through
//! [`camel_component_wasm::bean::WasmBean::call`].
//!
//! Run with: `cargo test -p camel-component-wasm --test bean_streaming_integration -- --nocapture`

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use camel_api::{Body, CamelError, Exchange, Message, StreamBody, StreamMetadata};
use camel_bean::BeanProcessor;
use camel_component_wasm::WasmConfig;
use camel_component_wasm::bean::WasmBean;
use camel_core::Registry;
use futures::stream::{self, BoxStream, StreamExt};

const MB: usize = 1024 * 1024;

/// Path to the prebuilt `text-utils` bean fixture (methods: upper/reverse/last/count).
fn bean_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples/wasm-bean-example/fixtures/text_utils.wasm")
}

/// Wrap a byte stream in an `Exchange` whose input body is `Body::Stream`.
fn stream_exchange(stream: BoxStream<'static, Result<Bytes, CamelError>>) -> Exchange {
    let body = Body::Stream(StreamBody {
        stream: Arc::new(tokio::sync::Mutex::new(Some(stream))),
        metadata: StreamMetadata::default(),
    });
    Exchange::new(Message::new(body))
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

// ---------------------------------------------------------------------------
// Test 1: 1 MiB stream is counted without materialising
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bean_streaming_counts_one_mib_stream() {
    let bean = fresh_bean().await;

    let payload = Bytes::from(vec![b'x'; MB]);
    let stream = stream::iter(vec![Ok::<_, CamelError>(payload)]).boxed();
    let mut exchange = stream_exchange(stream);

    bean.call("count", &mut exchange)
        .await
        .expect("bean count on a streaming body should succeed");

    match &exchange.input.body {
        Body::Text(s) => assert_eq!(s, "streamed 1048576 bytes"),
        other => panic!("expected Text body after count, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Test 2: empty stream (immediate EOF) → 0 bytes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bean_streaming_counts_empty_stream_as_zero() {
    let bean = fresh_bean().await;

    let stream = stream::iter(Vec::<Result<Bytes, CamelError>>::new()).boxed();
    let mut exchange = stream_exchange(stream);

    bean.call("count", &mut exchange)
        .await
        .expect("empty stream should still succeed");

    match &exchange.input.body {
        Body::Text(s) => assert_eq!(s, "streamed 0 bytes"),
        other => panic!("expected Text body after count, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Test 3: chunked stream (many small chunks) tallies the full payload
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bean_streaming_counts_chunked_stream() {
    let bean = fresh_bean().await;

    // 64 chunks of 16 KiB = 1 MiB total
    let chunk = Bytes::from(vec![b'y'; 16 * 1024]);
    let chunks: Vec<Result<Bytes, CamelError>> = std::iter::repeat_with(|| Ok(chunk.clone()))
        .take(64)
        .collect();
    let stream = stream::iter(chunks).boxed();
    let mut exchange = stream_exchange(stream);

    bean.call("count", &mut exchange)
        .await
        .expect("chunked stream should succeed");

    match &exchange.input.body {
        Body::Text(s) => assert_eq!(s, "streamed 1048576 bytes"),
        other => panic!("expected Text body after count, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Test 4: under the phase-aware watchdog (0.23+), a stalled stream read
//         during the invoke phase does NOT trip the stream-progress
//         watchdog — stalled inputs are bounded by epoch interruption
//         (timeout_secs), not the per-stream watchdog. The drain-phase
//         watchdog trip is covered by `drain_watchdog_trips_on_stalled_drain`
//         in runtime.rs.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bean_streaming_watchdog_does_not_false_trip_on_stalled_read() {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    let bean = WasmBean::new(
        bean_fixture(),
        WasmConfig::default(),
        registry,
        HashMap::new(),
    )
    .await
    .expect("bean fixture loads");

    // A stream that never yields — `count` blocks on `reader.read()` forever.
    // Under the phase-aware watchdog the invoke phase has no timeout, so the
    // call hangs. We wrap in a timeout to prove the watchdog does NOT
    // false-trip during the invoke phase.
    let stream = futures::stream::pending::<Result<Bytes, CamelError>>().boxed();
    let mut exchange = stream_exchange(stream);

    let result = tokio::time::timeout(
        Duration::from_millis(500),
        bean.call("count", &mut exchange),
    )
    .await;

    // The call should still be pending after 500ms (not completed, not errored).
    // This proves the watchdog does NOT fire during the invoke phase.
    assert!(
        result.is_err(),
        "call should still be pending after 500ms — \
         watchdog must not trip during the invoke phase"
    );
}

// ---------------------------------------------------------------------------
// Test 5: a guest-emitted WasmError propagates through the 2-layer peel to a
//         CamelError (exercises the streaming-path error mapping).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bean_streaming_propagates_guest_error() {
    let bean = fresh_bean().await;

    let payload = Bytes::from(vec![b'x'; 64]);
    let stream = stream::iter(vec![Ok::<_, CamelError>(payload)]).boxed();
    let mut exchange = stream_exchange(stream);

    let err = bean
        .call("fail", &mut exchange)
        .await
        .expect_err("the `fail` method must surface its WasmError");

    let msg = err.to_string();
    assert!(
        msg.contains("failure"),
        "expected the guest's error message to propagate, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Test 6: non-streaming (Body::Bytes) call with a tight watchdog timeout.
// The `drain_started` signal is never fired (no output stream), so the
// phase-aware watchdog never arms. The call must succeed even though the
// configured no_progress_timeout is far shorter than the processing time.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bean_non_streaming_call_succeeds_with_tight_watchdog() {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    let bean = WasmBean::new(
        bean_fixture(),
        WasmConfig::default(),
        registry,
        HashMap::new(),
    )
    .await
    .expect("bean fixture loads")
    .with_streaming_knobs(u64::MAX, Duration::ZERO);

    // Non-streaming Text input — no output stream, so drain_started is never
    // fired. The phase-aware watchdog stays in the invoke phase (no timeout)
    // and the call completes normally.
    let mut exchange = Exchange::new(Message::new(Body::Text("hello world".into())));

    bean.call("upper", &mut exchange)
        .await
        .expect("non-streaming call must not false-trip watchdog");

    match &exchange.input.body {
        Body::Text(s) => assert_eq!(s, "HELLO WORLD"),
        other => panic!("expected Text body after upper, got {other:?}"),
    }
}
