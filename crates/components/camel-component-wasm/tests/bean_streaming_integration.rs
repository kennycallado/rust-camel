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
/// Per-stream byte cap large enough to admit any test payload.
const GENEROUS_MAX_BYTES: u64 = 64 * MB as u64;

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
// Test 4: no-progress watchdog fires when the guest awaits a stream that
//         never produces — and the cancel path is exercised (rc-2lge review).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bean_streaming_watchdog_fires_on_stalled_read() {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    let bean = WasmBean::new(
        bean_fixture(),
        WasmConfig::default(),
        registry,
        HashMap::new(),
    )
    .await
    .expect("bean fixture loads")
    // Tight watchdog: a stalled read must trip well before the default 60s.
    .with_streaming_knobs(GENEROUS_MAX_BYTES, Duration::from_millis(200));

    // A stream that never yields and never closes — `count` blocks on
    // `reader.read()` forever, so no chunk ships and progress_notify never
    // fires → the no-progress watchdog cancels the call.
    let stream = futures::stream::pending::<Result<Bytes, CamelError>>().boxed();
    let mut exchange = stream_exchange(stream);

    let err = bean
        .call("count", &mut exchange)
        .await
        .expect_err("stalled read must trip the watchdog");

    let msg = err.to_string();
    assert!(
        msg.contains("no-progress") || msg.contains("stalled") || msg.contains("timeout"),
        "expected a watchdog timeout error, got: {msg}"
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
