//! L3 streaming-contract pins for the REST DSL `binding: raw` mode
//! (change `add-rest-streaming-contract`, bd rc-q8apn).
//!
//! L1 (`add-rest-raw-binding`) landed the raw binding and its spec
//! deliberately deferred "single-consumption stream ownership, metadata
//! preservation, and reply-stream semantics" to this change. The contract
//! is pinned at two boundaries:
//!
//! 1. **DSL boundary** — the lowered raw pipeline (user steps + the two
//!    injected binding-independent steps) must not poll, materialize,
//!    re-wrap, or strip metadata from the request stream. Proven by
//!    driving the compiled steps of a real `binding: raw` route over an
//!    instrumented exchange (poll counter, `Arc` identity handle,
//!    metadata equality, one-then-`AlreadyConsumed` consumption).
//!
//! 2. **HTTP boundary** — a genuine `camel-component-http` consumer is
//!    booted from this crate (dev-dependency; camel-component-http does
//!    not depend on camel-dsl, so no cycle) and driven with a
//!    hand-rolled TCP HTTP/1.1 client. This pins the REST-registered
//!    consumer path end-to-end: request `StreamMetadata` population,
//!    the chunked-request mid-stream cap, the consumed-reply 500, the
//!    materialized-bytes response cap, the uncapped streamed reply (the
//!    decided limit policy), original-or-new reply streams, and
//!    client-disconnect survival.
//!
//! Rig notes: the process-global HTTP `ServerRegistry` is shared by every
//! test in this binary, so server tests serialize on `SERVER_MUTEX` on a
//! fresh bind-drop port; paths are shared across tests and registry
//! registration is last-write-wins (see `tests/common/mod.rs`).
//! Readiness uses a connect-retry loop, never a fixed sleep.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use camel_api::{Body, CamelError, Exchange, Message, StreamBody, StreamMetadata};
use camel_core::route::BuilderStep;
use camel_dsl::parse_yaml;
use futures::StreamExt;
use futures::stream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio::time::timeout;
use tower::ServiceExt;

use common::{
    SERVER_MUTEX, ServerHandle, chunked_request, compile_header_step, counting_stream_body,
    http_roundtrip, reply_ok, simple_request,
};

// ===========================================================================
// Part 1 — DSL-boundary pins (compiled raw pipeline over a raw route)
// ===========================================================================

const RAW_STREAM_YAML: &str = r#"
rest:
  - host: 127.0.0.1
    port: 18080
    path: /l3
    operations:
      - method: POST
        operation_id: l3Stream
        binding: raw
        consumes: application/octet-stream
        produces: application/octet-stream
        steps:
          - set_header:
              key: X-Trace
              value: t1
"#;

fn default_meta() -> StreamMetadata {
    StreamMetadata::default()
}

/// Drive every compiled header step of the raw route over the exchange, in
/// order. The content-negotiation gate (compiled to `BuilderStep::Processor`)
/// is skipped — this helper drives the header injections only.
async fn drive_raw_pipeline(mut ex: Exchange) -> Exchange {
    let routes = parse_yaml(RAW_STREAM_YAML).expect("raw stream YAML must parse + compile");
    assert_eq!(routes.len(), 1, "one operation must lower to one route");
    let steps = routes[0].steps();
    assert_eq!(
        steps.len(),
        4,
        "raw route must compile to exactly 4 steps, got {steps:?}"
    );
    for step in steps.iter() {
        // The negotiation gate is a Processor, not a header step — skip it.
        if matches!(step, BuilderStep::Processor(_)) {
            continue;
        }
        let processor = compile_header_step(step);
        ex = processor
            .oneshot(ex)
            .await
            .expect("raw pipeline step must succeed on a stream body");
    }
    ex
}

#[tokio::test]
async fn raw_pipeline_never_polls_request_stream() {
    let polls = Arc::new(AtomicUsize::new(0));
    let ex = drive_raw_pipeline(Exchange::new(Message::new(counting_stream_body(
        "own-this-wire",
        polls.clone(),
        default_meta(),
    ))))
    .await;

    assert_eq!(
        polls.load(Ordering::SeqCst),
        0,
        "the raw pipeline must never poll the request stream"
    );
    assert!(
        matches!(ex.input.body, Body::Stream(_)),
        "body must still be the stream variant, got: {:?}",
        ex.input.body
    );
    assert_eq!(
        ex.input.header("Content-Type"),
        Some(&serde_json::json!("application/octet-stream")),
        "declared produces must be the explicit Content-Type"
    );
    assert_eq!(
        ex.input.header("CamelHttpResponseCode"),
        Some(&serde_json::json!(201)),
        "POST default status must be injected if-absent"
    );
}

#[tokio::test]
async fn raw_pipeline_preserves_stream_identity() {
    // Keep an identity handle to the stream mutex BEFORE the pipeline
    // runs. If any step re-wrapped or replaced the StreamBody, the
    // reachable mutex would be a different allocation.
    let polls = Arc::new(AtomicUsize::new(0));
    let body = counting_stream_body("identity", polls, default_meta());
    let Body::Stream(ref sb) = body else {
        panic!("test builds a stream body");
    };
    let handle = sb.stream.clone();

    let ex = drive_raw_pipeline(Exchange::new(Message::new(body))).await;

    let Body::Stream(after) = ex.input.body else {
        panic!("body must still be a stream after the pipeline");
    };
    assert!(
        Arc::ptr_eq(&handle, &after.stream),
        "the pipeline must not replace the request StreamBody"
    );
}

#[tokio::test]
async fn request_stream_consumable_exactly_once_after_pipeline() {
    let polls = Arc::new(AtomicUsize::new(0));
    let ex = drive_raw_pipeline(Exchange::new(Message::new(counting_stream_body(
        "payload",
        polls,
        default_meta(),
    ))))
    .await;

    let Body::Stream(sb) = ex.input.body else {
        panic!("body must still be a stream after the pipeline");
    };
    // First consumption: exactly the stream's bytes.
    let first = Body::Stream(sb.clone())
        .into_bytes(64 * 1024)
        .await
        .expect("first consumption after the pipeline must succeed");
    assert_eq!(first, Bytes::from_static(b"payload"));
    // Second consumption on the same underlying stream: AlreadyConsumed.
    let second = Body::Stream(sb).into_bytes(64 * 1024).await;
    match second {
        Err(CamelError::AlreadyConsumed) => {}
        other => panic!("expected AlreadyConsumed, got: {other:?}"),
    }
}

#[tokio::test]
async fn raw_pipeline_preserves_stream_metadata() {
    let polls = Arc::new(AtomicUsize::new(0));
    let metadata = StreamMetadata {
        content_type: Some("image/png".to_string()),
        size_hint: Some(9),
        origin: None,
    };
    let body = counting_stream_body("image data", polls, metadata.clone());
    let Body::Stream(ref sb) = body else {
        panic!("test builds a stream body");
    };
    debug_assert_eq!(sb.metadata.content_type, metadata.content_type);

    let ex = drive_raw_pipeline(Exchange::new(Message::new(body))).await;

    let Body::Stream(after) = ex.input.body else {
        panic!("body must still be a stream after the pipeline");
    };
    assert_eq!(
        after.metadata.content_type,
        Some("image/png".to_string()),
        "content type metadata must survive the pipeline"
    );
    assert_eq!(
        after.metadata.size_hint,
        Some(9),
        "size hint metadata must survive the pipeline"
    );
    assert_eq!(after.metadata.origin, None);
}

// ===========================================================================
// Part 2 — HTTP-boundary pins (real camel-component-http consumer)
// ===========================================================================

/// Server rig shared with the sibling REST e2e binaries lives in
/// `tests/common`; this suite parameterizes the body caps to pin
/// rejection at both limits, under its own diagnostic label.
async fn spawn_raw_server(
    path: &str,
    method: &str,
    max_req: usize,
    max_resp: usize,
) -> ServerHandle {
    common::spawn_test_server("l3-stream-contract-test", path, method, max_req, max_resp).await
}

/// A reply stream carrying `n` identical bytes in one chunk.
fn sized_reply_stream(n: usize, content_type: &str) -> Body {
    let s = stream::once(async move { Ok::<Bytes, CamelError>(Bytes::from(vec![b'x'; n])) });
    Body::Stream(StreamBody {
        stream: Arc::new(Mutex::new(Some(Box::pin(s)))),
        metadata: StreamMetadata {
            size_hint: Some(n as u64),
            content_type: Some(content_type.to_string()),
            origin: None,
        },
    })
}

#[tokio::test]
async fn http_request_metadata_carries_content_type_and_length() {
    let _guard = SERVER_MUTEX.lock().await;
    let ServerHandle {
        port,
        mut rx,
        token,
    } = spawn_raw_server("/meta", "POST", 1024 * 1024, 1024 * 1024).await;

    let req = simple_request("POST", "/meta", &[("Content-Type", "image/png")], b"PNG!");
    let (resp, _) = tokio::join!(http_roundtrip(port, req), async {
        let mut envelope = rx.recv().await.expect("envelope must arrive");
        let Body::Stream(sb) = &envelope.exchange.input.body else {
            panic!("request must arrive as Body::Stream");
        };
        assert_eq!(
            sb.metadata.content_type.as_deref(),
            Some("image/png"),
            "request Content-Type must be preserved in stream metadata"
        );
        assert_eq!(
            sb.metadata.size_hint,
            Some(4),
            "request Content-Length must be preserved as size hint"
        );
        envelope.exchange.input.body = Body::Bytes(Bytes::from_static(b"ok"));
        envelope
            .exchange
            .input
            .set_header("Content-Type", serde_json::json!("text/plain"));
        reply_ok(envelope);
    });

    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, Bytes::from_static(b"ok"));
    token.cancel();
}

#[tokio::test]
async fn chunked_request_over_cap_fails_closed() {
    let _guard = SERVER_MUTEX.lock().await;
    // Cap 12 bytes; the request streams 16 content bytes in two chunks —
    // past the Content-Length pre-check, so only the mid-stream cap can
    // stop it, exactly on consumption.
    let ServerHandle {
        port,
        mut rx,
        token,
    } = spawn_raw_server("/chunked-cap", "POST", 12, 1024 * 1024).await;

    let req = chunked_request("POST", "/chunked-cap", &[], &[b"AAAAAAAA", b"BBBBBBBB"]);
    let (resp, _) = tokio::join!(http_roundtrip(port, req), async {
        let mut envelope = rx.recv().await.expect("envelope must arrive");
        let err = match envelope.exchange.input.body.into_bytes(64 * 1024).await {
            Ok(bytes) => panic!("over-cap chunked request must fail closed, got {bytes:?}"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("exceeds configured limit"),
            "consumption must fail with the cap error, got: {err}"
        );
        assert!(
            err.to_string().contains("of 12 bytes"),
            "cap error must name the configured limit, got: {err}"
        );
        envelope.exchange.input.body = Body::Bytes(Bytes::from_static(b"rejected"));
        envelope
            .exchange
            .input
            .set_header("Content-Type", serde_json::json!("text/plain"));
        reply_ok(envelope);
    });

    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, Bytes::from_static(b"rejected"));
    token.cancel();
}

#[tokio::test]
async fn in_route_double_consumption_surfaces_already_consumed() {
    let _guard = SERVER_MUTEX.lock().await;
    let ServerHandle {
        port,
        mut rx,
        token,
    } = spawn_raw_server("/double", "POST", 1024 * 1024, 1024 * 1024).await;

    let req = simple_request("POST", "/double", &[], b"once-only");
    let (resp, _) = tokio::join!(http_roundtrip(port, req), async {
        let mut envelope = rx.recv().await.expect("envelope must arrive");
        let Body::Stream(sb) = std::mem::take(&mut envelope.exchange.input.body) else {
            panic!("request must arrive as Body::Stream");
        };
        // First in-route consumption succeeds with the exact bytes.
        let first = Body::Stream(sb.clone())
            .into_bytes(1024)
            .await
            .expect("first consumption must succeed");
        assert_eq!(first, Bytes::from_static(b"once-only"));
        // Second consumption: AlreadyConsumed, propagated as the error
        // reply — never a panic.
        let err = match Body::Stream(sb).into_bytes(1024).await {
            Ok(bytes) => panic!("second consumption must fail, got {bytes:?}"),
            Err(e) => e,
        };
        assert!(
            matches!(err, CamelError::AlreadyConsumed),
            "expected AlreadyConsumed, got: {err:?}"
        );
        if let Some(reply_tx) = envelope.reply_tx.take() {
            let _ = reply_tx.send(Err(err));
        }
    });

    assert!(
        resp.status >= 500,
        "AlreadyConsumed must propagate as a 5xx error reply, got {}",
        resp.status
    );
    token.cancel();
}

#[tokio::test]
async fn consumed_reply_stream_returns_500() {
    let _guard = SERVER_MUTEX.lock().await;
    let ServerHandle {
        port,
        mut rx,
        token,
    } = spawn_raw_server("/consumed", "POST", 1024 * 1024, 1024 * 1024).await;

    let req = simple_request("POST", "/consumed", &[], b"consume-me");
    let (resp, _) = tokio::join!(http_roundtrip(port, req), async {
        let mut envelope = rx.recv().await.expect("envelope must arrive");
        let Body::Stream(sb) = std::mem::take(&mut envelope.exchange.input.body) else {
            panic!("request must arrive as Body::Stream");
        };
        // The route consumes the stream during processing, then leaves
        // the (now empty) stream body in place as the reply.
        let eaten = Body::Stream(sb.clone())
            .into_bytes(1024)
            .await
            .expect("route consumption must succeed");
        assert_eq!(eaten, Bytes::from_static(b"consume-me"));
        envelope.exchange.input.body = Body::Stream(sb);
        reply_ok(envelope);
    });

    assert_eq!(resp.status, 500, "consumed reply stream must yield 500");
    assert!(
        resp.body.is_empty(),
        "consumed reply stream 500 body must be empty, got {:?}",
        resp.body
    );
    token.cancel();
}

#[tokio::test]
async fn materialized_reply_bytes_over_cap_replaced_with_500() {
    let _guard = SERVER_MUTEX.lock().await;
    let ServerHandle {
        port,
        mut rx,
        token,
    } = spawn_raw_server("/bytes-cap", "GET", 1024 * 1024, 8).await;

    let req = simple_request("GET", "/bytes-cap", &[], b"");
    let (resp, _) = tokio::join!(http_roundtrip(port, req), async {
        let mut envelope = rx.recv().await.expect("envelope must arrive");
        envelope.exchange.input.body = Body::Bytes(Bytes::from(vec![b'y'; 32]));
        envelope
            .exchange
            .input
            .set_header("Content-Type", serde_json::json!("text/plain"));
        reply_ok(envelope);
    });

    assert_eq!(resp.status, 500);
    assert_eq!(
        resp.body,
        Bytes::from_static(b"Response body exceeds configured limit")
    );
    token.cancel();
}

#[tokio::test]
async fn streamed_reply_over_max_response_body_succeeds() {
    let _guard = SERVER_MUTEX.lock().await;
    // The decided limit policy: max_response_body caps materialized
    // (Bytes) replies only; a streamed reply flows uncapped end-to-end.
    let ServerHandle {
        port,
        mut rx,
        token,
    } = spawn_raw_server("/stream-cap", "GET", 1024 * 1024, 8).await;

    let req = simple_request("GET", "/stream-cap", &[], b"");
    let (resp, _) = tokio::join!(http_roundtrip(port, req), async {
        let mut envelope = rx.recv().await.expect("envelope must arrive");
        // Metadata content type deliberately differs from the route-supplied
        // header so header-vs-metadata precedence stays observable.
        envelope.exchange.input.body = sized_reply_stream(32, "application/octet-stream");
        envelope
            .exchange
            .input
            .set_header("Content-Type", serde_json::json!("text/plain"));
        reply_ok(envelope);
    });

    assert_eq!(resp.status, 200, "streamed reply must not be byte-capped");
    assert_eq!(
        resp.header("content-type"),
        Some("text/plain"),
        "route-supplied Content-Type must win over stream metadata"
    );
    assert_eq!(resp.body.len(), 32, "every stream byte must reach the wire");
    assert!(resp.body.iter().all(|&b| b == b'x'));
    token.cancel();
}

#[tokio::test]
async fn reply_may_stream_original_request_body() {
    let _guard = SERVER_MUTEX.lock().await;
    let ServerHandle {
        port,
        mut rx,
        token,
    } = spawn_raw_server("/echo", "POST", 1024 * 1024, 1024 * 1024).await;

    let req = simple_request("POST", "/echo", &[], b"echo-me");
    let (resp, _) = tokio::join!(http_roundtrip(port, req), async {
        let mut envelope = rx.recv().await.expect("envelope must arrive");
        // Echo semantics: the route never touches the request body —
        // the ORIGINAL stream is the reply body.
        envelope.exchange.input.set_header(
            "Content-Type",
            serde_json::json!("application/octet-stream"),
        );
        reply_ok(envelope);
    });

    assert_eq!(resp.status, 200);
    assert_eq!(
        resp.header("content-type"),
        Some("application/octet-stream"),
        "route-supplied Content-Type must win"
    );
    assert_eq!(resp.body, Bytes::from_static(b"echo-me"));
    token.cancel();
}

#[tokio::test]
async fn client_disconnect_during_streamed_reply_keeps_server_healthy() {
    let _guard = SERVER_MUTEX.lock().await;
    let ServerHandle {
        port,
        mut rx,
        token,
    } = spawn_raw_server("/disconnect", "GET", 1024 * 1024, 1024 * 1024).await;

    // Handler serves any number of requests: the first replies with a
    // stream that yields one chunk then pends forever; later requests
    // get a plain materialized reply.
    let handler = tokio::spawn(async move {
        let mut first = true;
        loop {
            let mut envelope = match timeout(Duration::from_secs(2), rx.recv()).await {
                Ok(Some(envelope)) => envelope,
                Ok(None) => break, // closed: drain complete
                Err(_) => break,   // stalled: drainer ends
            };
            if first {
                first = false;
                let s = stream::once(async {
                    Ok::<Bytes, CamelError>(Bytes::from_static(b"first-chunk"))
                })
                .chain(stream::pending());
                envelope.exchange.input.body = Body::Stream(StreamBody {
                    stream: Arc::new(Mutex::new(Some(Box::pin(s)))),
                    metadata: StreamMetadata::default(),
                });
                envelope
                    .exchange
                    .input
                    .set_header("Content-Type", serde_json::json!("text/plain"));
            } else {
                envelope.exchange.input.body = Body::Bytes(Bytes::from_static(b"still-alive"));
            }
            reply_ok(envelope);
        }
    });

    // First request: read until the first chunk is on the wire, then
    // drop the socket mid-reply.
    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect for disconnect test");
    sock.write_all(b"GET /disconnect HTTP/1.1\r\nHost: l3-test\r\nConnection: close\r\n\r\n")
        .await
        .expect("write disconnect request");
    let mut seen = Vec::new();
    let got_chunk = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let mut chunk = [0u8; 1024];
            let n = sock.read(&mut chunk).await.expect("read streamed reply");
            seen.extend_from_slice(&chunk[..n]);
            if seen.windows(11).any(|w| w == b"first-chunk") {
                break;
            }
        }
    })
    .await;
    assert!(
        got_chunk.is_ok(),
        "first chunk must arrive before the disconnect, got buffer: {:?}",
        String::from_utf8_lossy(&seen)
    );
    drop(sock); // the disconnect — the server must survive it

    // Second request on the same server: must be served.
    let resp = http_roundtrip(port, simple_request("GET", "/disconnect", &[], b"")).await;
    assert_eq!(
        resp.status, 200,
        "consumer must survive the client disconnect"
    );
    assert_eq!(resp.body, Bytes::from_static(b"still-alive"));

    token.cancel();
    tokio::time::timeout(Duration::from_secs(3), handler)
        .await
        .expect("handler task must finish after cancel")
        .expect("handler task must not panic");
}
