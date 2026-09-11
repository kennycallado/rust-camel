//! End-to-end negotiation battery for the REST DSL strict content
//! negotiation (change `add-rest-strict-negotiation`, Task 5.1).
//!
//! The battery pins the two negotiation requirements of the rest-dsl
//! spec at the two boundaries that matter:
//!
//! 1. **DSL boundary** — a lowered REST document compiles the
//!    content-negotiation gate as the FIRST route step. Driven over an
//!    instrumented `Body::Stream` exchange (poll counter, `Arc` identity
//!    handle, metadata snapshot), a rejected exchange fails with the
//!    typed `UnsupportedMediaType` variant and NEVER polls the request
//!    stream; an accepted exchange passes through with the exact same
//!    `StreamBody` allocation and untouched `StreamMetadata`. A live TCP
//!    request cannot prove pipeline-side poll counts (the transport
//!    itself polls), hence the DSL-level twin (same discipline as
//!    `rest_stream_contract_e2e.rs` Part 1).
//!
//! 2. **HTTP boundary** — a genuine `camel_component_http::HttpConsumer`
//!    is booted from this crate (camel-dsl dev-depends on
//!    camel-component-http; no reverse dependency exists, so the rig
//!    lives in this crate's `tests/common`) and driven with a
//!    hand-rolled TCP HTTP/1.1 client,
//!    exactly like `rest_stream_contract_e2e.rs` Part 2. The mpsc
//!    receiver stands in for the downstream runtime: for each envelope
//!    the test drives the compiled negotiation gate (the first step of
//!    the lowered route) the way the runtime would. `Err` becomes the
//!    error reply, which the HTTP finaliser maps to 415
//!    (`unsupported_media_type`) / 406 (`not_acceptable`) with a JSON
//!    body; `Ok` advances to the route sink (a counter the test
//!    inspects) and a deterministic reply. Unmarshal/marshal behaviour
//!    is pinned by `rest_schema_e2e.rs` / `rest_raw_e2e.rs` — this
//!    battery is about the gate.
//!
//! Document under test: one POST op binding `json`, one GET op binding
//! `json`, one DELETE op binding `json`, and one POST op binding `raw`
//! (octet-stream echo), all `consumes`/`produces` explicit.
//!
//! Rig notes: the process-global HTTP `ServerRegistry` is shared by
//! every test in this binary, so server tests serialize on
//! `SERVER_MUTEX`, each booting on a fresh bind-drop port; paths are
//! shared across tests and registry registration is last-write-wins.
//! Readiness uses a connect-retry loop, never a fixed sleep.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bytes::Bytes;
use camel_api::{Body, CamelError, Exchange, IdentityProcessor, Message, StreamMetadata};
use camel_component_api::ExchangeEnvelope;
use camel_core::route::BuilderStep;
use camel_dsl::{ValueSourceDef, parse_yaml};
use camel_processor::{SetHeader, SetHeaderIfAbsent};
use tokio::sync::mpsc;
use tower::ServiceExt;

use common::{
    SERVER_MUTEX, ServerHandle, chunked_request, counting_stream_body, http_roundtrip, reply_ok,
    simple_request,
};

/// Wire pin for the permissive pass-through reply (absent Accept +
/// matching Content-Type). The pin lives HERE, not in another suite: if
/// the HTTP reply path ever re-wraps, re-serialises, or annotates route
/// replies, the exact byte equality in
/// `e2e_absent_headers_permissive_pinned_bytes` breaks.
const PINNED_PASS_REPLY: &[u8] = br#"{"negotiation":"passed"}"#;

/// The REST document under test: three `json`-bound operations (POST,
/// GET, DELETE — the verb trio with distinct body-bearing semantics) and
/// one `raw`-bound POST (octet-stream echo). The declared host/port are
/// never bound — the live rig boots consumers on free ports; only the
/// lowered and compiled steps are taken from this document.
const NEGOTIATION_DOC: &str = r#"
rest:
  - host: 127.0.0.1
    port: 18100
    path: /neg
    operations:
      - method: POST
        path: /post
        operation_id: negPost
        binding: json
        consumes: application/json
        produces: application/json
        to: direct:negSink
      - method: GET
        path: /get
        operation_id: negGet
        binding: json
        consumes: application/json
        produces: application/json
        to: direct:negSink
      - method: DELETE
        path: /delete
        operation_id: negDelete
        binding: json
        consumes: application/json
        produces: application/json
        to: direct:negSink
      - method: POST
        path: /raw
        operation_id: negRawPost
        binding: raw
        consumes: application/octet-stream
        produces: application/octet-stream
        to: direct:negRawSink
"#;

// ===========================================================================
// Shared DSL rig — compile a document, extract an operation's gate
// ===========================================================================

/// The FIRST compiled step of the operation's lowered route: the
/// content-negotiation gate, already wired to its media contract
/// (`BuilderStep::Processor`). Panics with a clear message if lowering
/// ever stops injecting the gate first — the position is part of the
/// contract (negotiation precedes unmarshal).
fn compiled_gate(doc: &str, op_id: &str) -> camel_api::BoxProcessor {
    let routes = parse_yaml(doc).expect("negotiation doc must parse + compile");
    let route = routes
        .iter()
        .find(|r| r.route_id() == op_id)
        .unwrap_or_else(|| panic!("operation '{op_id}' must lower to exactly one route"));
    match route.steps().first() {
        Some(BuilderStep::Processor(op)) => op.0.clone(),
        other => {
            panic!("first compiled step of '{op_id}' must be the negotiation gate, got: {other:?}")
        }
    }
}

/// Compile one compiled route step into the Tower service the runtime
/// wires for it — `Processor` steps unwrap to their pre-built service,
/// header steps compile exactly like camel-core's step compiler (same
/// idiom as `rest_raw_e2e.rs`). `To` and friends cannot be resolved
/// without a CamelContext; the negotiation battery never reaches them
/// because the gate is the FIRST step and these drives stop at the
/// first error.
fn compile_step_for_drive(step: &BuilderStep) -> camel_api::BoxProcessor {
    match step {
        BuilderStep::Processor(op) => op.0.clone(),
        BuilderStep::DeclarativeSetHeader { key, value } => match value {
            ValueSourceDef::Literal(v) => camel_api::BoxProcessor::new(SetHeader::new(
                IdentityProcessor,
                key.clone(),
                v.clone(),
            )),
            other => panic!("expected literal set_header value, got {other:?}"),
        },
        BuilderStep::DeclarativeSetHeaderIfAbsent { key, value } => match value {
            ValueSourceDef::Literal(v) => camel_api::BoxProcessor::new(SetHeaderIfAbsent::new(
                IdentityProcessor,
                key.clone(),
                v.clone(),
            )),
            other => panic!("expected literal set_header_if_absent value, got {other:?}"),
        },
        other => panic!("step cannot be driven in this rig (no endpoint resolution): {other:?}"),
    }
}

/// Drive every compiled step of the route over the exchange, in order,
/// propagating the first error. Mirrors the sibling drive loops; the
/// chain fails fast at step 0 (the gate) for rejected media.
async fn drive_compiled_steps(
    steps: &[BuilderStep],
    mut exchange: Exchange,
) -> Result<Exchange, CamelError> {
    for step in steps {
        exchange = compile_step_for_drive(step).oneshot(exchange).await?;
    }
    Ok(exchange)
}

// ===========================================================================
// Part 1 — DSL-boundary pins (compiled gate over an instrumented body)
// ===========================================================================

#[tokio::test]
async fn dsl_compiled_negotiation_rejects_without_polling() {
    // A POST binding-json route driven with Content-Type: text/plain
    // must fail the compiled gate with the typed 415 variant BEFORE any
    // pipeline work. `Service::call` Err semantics drop the exchange, so
    // only the error variant and the external poll counter are
    // assertable — which is the point: rejection must cost zero polls.
    let routes = parse_yaml(NEGOTIATION_DOC).expect("negotiation doc must parse + compile");
    let route = routes
        .iter()
        .find(|r| r.route_id() == "negPost")
        .expect("negPost must lower to exactly one route");

    let polls = Arc::new(AtomicUsize::new(0));
    let mut msg = Message::new(counting_stream_body(
        r#"{"name":"kenny"}"#,
        polls.clone(),
        StreamMetadata::default(),
    ));
    msg.set_header("Content-Type", "text/plain");
    let exchange = Exchange::new(msg);

    let err = drive_compiled_steps(route.steps(), exchange)
        .await
        .expect_err("text/plain against declared application/json must fail the gate");
    assert!(
        matches!(err, CamelError::UnsupportedMediaType { .. }),
        "expected UnsupportedMediaType, got: {err:?}"
    );
    assert_eq!(
        polls.load(Ordering::SeqCst),
        0,
        "rejection must never poll the request stream"
    );
}

#[tokio::test]
async fn dsl_compiled_negotiation_pass_preserves_body() {
    // Pass path: drive ONLY the compiled negotiation step via oneshot.
    // Matching headers must return the SAME exchange — body still
    // Body::Stream (asserted via matches!, no PartialEq on the variant),
    // the very same StreamBody allocation (Arc::ptr_eq), and identical
    // StreamMetadata. The returned exchange only exists on Ok, so the
    // pass path proves the other half of body neutrality.
    let gate = compiled_gate(NEGOTIATION_DOC, "negPost");

    let polls = Arc::new(AtomicUsize::new(0));
    let metadata = StreamMetadata {
        content_type: Some("application/json".to_string()),
        size_hint: Some(16),
        origin: None,
    };
    let body = counting_stream_body("identity-payload", polls.clone(), metadata);
    // Identity handle to the stream mutex BEFORE the gate runs: if the
    // gate re-wrapped or replaced the StreamBody, the reachable mutex
    // would be a different allocation.
    let Body::Stream(ref sb) = body else {
        panic!("test builds a stream body");
    };
    let stream_handle = sb.stream.clone();

    let mut msg = Message::new(body);
    msg.set_header("Content-Type", "application/json");
    msg.set_header("Accept", "application/json");

    let out = gate
        .oneshot(Exchange::new(msg))
        .await
        .expect("matching Content-Type and Accept must pass the gate");

    assert_eq!(
        polls.load(Ordering::SeqCst),
        0,
        "the gate must never poll the request stream"
    );
    assert!(
        matches!(out.input.body, Body::Stream(_)),
        "body must still be the stream variant, got: {:?}",
        out.input.body
    );
    let Body::Stream(after) = out.input.body else {
        panic!("checked the stream variant above");
    };
    assert!(
        Arc::ptr_eq(&stream_handle, &after.stream),
        "the gate must not replace the request StreamBody"
    );
    assert_eq!(
        after.metadata.content_type,
        Some("application/json".to_string()),
        "stream content-type metadata must be untouched"
    );
    assert_eq!(
        after.metadata.size_hint,
        Some(16),
        "size hint must be untouched"
    );
    assert_eq!(after.metadata.origin, None, "origin must be untouched");
}

// ===========================================================================
// Part 2 — HTTP-boundary pins (real camel-component-http consumer)
// ===========================================================================

/// Server rig shared with the sibling REST e2e binaries lives in
/// `tests/common`; this battery boots consumers with fixed 1 MiB body
/// caps under its own diagnostic label.
async fn spawn_negotiation_server(path: &str, method: &str) -> ServerHandle {
    common::spawn_test_server(
        "rest-negotiation-e2e",
        path,
        method,
        1024 * 1024,
        1024 * 1024,
    )
    .await
}

/// What the stand-in route does once the gate passes.
#[derive(Clone, Copy)]
enum SinkReply {
    /// Replace the body with the pinned JSON literal (json-binding ops):
    /// the deterministic "sink produced a response" stand-in.
    PinnedJson,
    /// Echo the ORIGINAL request stream back to the wire (raw binding) —
    /// the stream-contract echo semantics.
    EchoStream,
}

/// Stand-in for the downstream runtime: for each envelope, drive the
/// compiled negotiation gate FIRST (as the lowered pipeline order
/// demands). On Err the exchange is dropped (Service::call semantics)
/// and the typed error becomes the error reply — the HTTP finaliser
/// maps it to 415/406. On Ok the sink counts the receipt and the
/// reply is fulfilled per `SinkReply`.
async fn serve_gate(
    mut rx: mpsc::Receiver<ExchangeEnvelope>,
    gate: camel_api::BoxProcessor,
    sink: Arc<AtomicUsize>,
    expected: usize,
    reply: SinkReply,
) {
    for _ in 0..expected {
        let mut envelope = rx.recv().await.expect("envelope must reach the pipeline");
        let outcome = gate.clone().oneshot(envelope.exchange).await;
        match outcome {
            Ok(mut passed) => {
                sink.fetch_add(1, Ordering::SeqCst);
                match reply {
                    SinkReply::PinnedJson => {
                        passed.input.body = Body::Bytes(Bytes::from_static(PINNED_PASS_REPLY));
                        passed.input.set_header("Content-Type", "application/json");
                    }
                    SinkReply::EchoStream => {
                        passed
                            .input
                            .set_header("Content-Type", "application/octet-stream");
                    }
                }
                envelope.exchange = passed;
                reply_ok(envelope);
            }
            Err(e) => {
                if let Some(reply_tx) = envelope.reply_tx.take() {
                    let _ = reply_tx.send(Err(e));
                }
            }
        }
    }
}

fn assert_2xx(actual: u16, context: &str) {
    assert!(
        (200..300).contains(&actual),
        "{context} must be 2xx, got {actual}"
    );
}

#[tokio::test]
async fn e2e_post_wrong_content_type_415_and_sink_untouched() {
    let _guard = SERVER_MUTEX.lock().await;
    let ServerHandle { port, rx, token } = spawn_negotiation_server("/neg/post", "POST").await;
    let sink = Arc::new(AtomicUsize::new(0));

    let req = simple_request(
        "POST",
        "/neg/post",
        &[("Content-Type", "text/plain")],
        br#"{"name":"kenny"}"#,
    );
    let (resp, _) = tokio::join!(
        http_roundtrip(port, req),
        serve_gate(
            rx,
            compiled_gate(NEGOTIATION_DOC, "negPost"),
            sink.clone(),
            1,
            SinkReply::PinnedJson
        ),
    );

    // 415 — and explicitly not 404: the registry routes by method+path
    // only, so media is not a routing key (runtime media-blindness). A
    // 404 here would mean the media type leaked into routing.
    assert_eq!(
        resp.status, 415,
        "wrong Content-Type must fail negotiation with 415 (not 404 — media is not a routing key)"
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&resp.body).expect("415 body must parse as JSON");
    assert_eq!(
        parsed["error"], "unsupported_media_type",
        "415 body must carry the typed error marker, got: {parsed}"
    );
    // Pin the finalizer payload: the message must name BOTH the
    // consumed wire media type and the declared one, or triage loses
    // half the diagnosis.
    let message = parsed["message"]
        .as_str()
        .expect("415 message must be a string");
    assert!(
        message.contains("text/plain"),
        "415 message must pin the consumed media type, got: {message}"
    );
    assert!(
        message.contains("application/json"),
        "415 message must pin the declared media type, got: {message}"
    );
    assert_eq!(
        sink.load(Ordering::SeqCst),
        0,
        "the route sink must receive NOTHING — negotiation rejects before the pipeline runs"
    );
    token.cancel();
}

#[tokio::test]
async fn e2e_post_parameterized_matching_content_type_passes() {
    let _guard = SERVER_MUTEX.lock().await;
    let ServerHandle { port, rx, token } = spawn_negotiation_server("/neg/post", "POST").await;
    let sink = Arc::new(AtomicUsize::new(0));

    let req = simple_request(
        "POST",
        "/neg/post",
        &[("Content-Type", "application/json; charset=utf-8")],
        br#"{"name":"kenny"}"#,
    );
    let (resp, _) = tokio::join!(
        http_roundtrip(port, req),
        serve_gate(
            rx,
            compiled_gate(NEGOTIATION_DOC, "negPost"),
            sink.clone(),
            1,
            SinkReply::PinnedJson
        ),
    );

    // Parameters (charset) are ignored when matching Content-Type.
    assert_2xx(resp.status, "parameterized matching Content-Type");
    assert_eq!(
        sink.load(Ordering::SeqCst),
        1,
        "the sink must receive the exchange when Content-Type matches"
    );
    token.cancel();
}

#[tokio::test]
async fn e2e_post_plus_json_suffix_passes() {
    let _guard = SERVER_MUTEX.lock().await;
    let ServerHandle { port, rx, token } = spawn_negotiation_server("/neg/post", "POST").await;
    let sink = Arc::new(AtomicUsize::new(0));

    let req = simple_request(
        "POST",
        "/neg/post",
        &[("Content-Type", "application/vnd.api+json")],
        br#"{"v":1}"#,
    );
    let (resp, _) = tokio::join!(
        http_roundtrip(port, req),
        serve_gate(
            rx,
            compiled_gate(NEGOTIATION_DOC, "negPost"),
            sink.clone(),
            1,
            SinkReply::PinnedJson
        ),
    );

    // A structured syntax suffix (+json) satisfies the JSON declaration.
    assert_2xx(resp.status, "+json structured syntax suffix");
    token.cancel();
}

#[tokio::test]
async fn e2e_post_malformed_content_type_415() {
    let _guard = SERVER_MUTEX.lock().await;
    let ServerHandle { port, rx, token } = spawn_negotiation_server("/neg/post", "POST").await;
    let sink = Arc::new(AtomicUsize::new(0));

    let req = simple_request(
        "POST",
        "/neg/post",
        &[("Content-Type", "garbage type")],
        br#"{"name":"kenny"}"#,
    );
    let (resp, _) = tokio::join!(
        http_roundtrip(port, req),
        serve_gate(
            rx,
            compiled_gate(NEGOTIATION_DOC, "negPost"),
            sink.clone(),
            1,
            SinkReply::PinnedJson
        ),
    );

    assert_eq!(
        resp.status, 415,
        "malformed Content-Type must fail with 415"
    );
    token.cancel();
}

#[tokio::test]
async fn e2e_post_wildcard_content_type_415() {
    let _guard = SERVER_MUTEX.lock().await;
    let ServerHandle { port, rx, token } = spawn_negotiation_server("/neg/post", "POST").await;
    let sink = Arc::new(AtomicUsize::new(0));

    let req = simple_request(
        "POST",
        "/neg/post",
        &[("Content-Type", "*/*")],
        br#"{"name":"kenny"}"#,
    );
    let (resp, _) = tokio::join!(
        http_roundtrip(port, req),
        serve_gate(
            rx,
            compiled_gate(NEGOTIATION_DOC, "negPost"),
            sink.clone(),
            1,
            SinkReply::PinnedJson
        ),
    );

    // A wildcard request Content-Type does not satisfy a concrete
    // declaration — the client must name the media it sends.
    assert_eq!(resp.status, 415, "wildcard Content-Type must fail with 415");
    token.cancel();
}

#[tokio::test]
async fn e2e_get_and_delete_no_content_type_check() {
    let _guard = SERVER_MUTEX.lock().await;
    // Body-less verbs keep the Accept-side gate but skip the
    // Content-Type side: a text/plain Content-Type on GET/DELETE must
    // not 415 — each op returns its own outcome.
    let ServerHandle {
        port: get_port,
        rx: get_rx,
        token: get_token,
    } = spawn_negotiation_server("/neg/get", "GET").await;
    let get_sink = Arc::new(AtomicUsize::new(0));

    let (get_resp, _) = tokio::join!(
        http_roundtrip(
            get_port,
            simple_request("GET", "/neg/get", &[("Content-Type", "text/plain")], b"")
        ),
        serve_gate(
            get_rx,
            compiled_gate(NEGOTIATION_DOC, "negGet"),
            get_sink.clone(),
            1,
            SinkReply::PinnedJson
        ),
    );
    assert_2xx(get_resp.status, "GET with text/plain Content-Type");
    assert_eq!(
        get_sink.load(Ordering::SeqCst),
        1,
        "GET must reach the sink despite the body-less Content-Type"
    );

    let ServerHandle {
        port: del_port,
        rx: del_rx,
        token: del_token,
    } = spawn_negotiation_server("/neg/delete", "DELETE").await;
    let del_sink = Arc::new(AtomicUsize::new(0));

    let (del_resp, _) = tokio::join!(
        http_roundtrip(
            del_port,
            simple_request(
                "DELETE",
                "/neg/delete",
                &[("Content-Type", "text/plain")],
                b""
            )
        ),
        serve_gate(
            del_rx,
            compiled_gate(NEGOTIATION_DOC, "negDelete"),
            del_sink.clone(),
            1,
            SinkReply::PinnedJson
        ),
    );
    assert_2xx(del_resp.status, "DELETE with text/plain Content-Type");
    assert_eq!(
        del_sink.load(Ordering::SeqCst),
        1,
        "DELETE must reach the sink despite the body-less Content-Type"
    );

    get_token.cancel();
    del_token.cancel();
}

#[tokio::test]
async fn e2e_accept_mismatch_406() {
    let _guard = SERVER_MUTEX.lock().await;
    let ServerHandle { port, rx, token } = spawn_negotiation_server("/neg/get", "GET").await;
    let sink = Arc::new(AtomicUsize::new(0));

    let req = simple_request("GET", "/neg/get", &[("Accept", "application/xml")], b"");
    let (resp, _) = tokio::join!(
        http_roundtrip(port, req),
        serve_gate(
            rx,
            compiled_gate(NEGOTIATION_DOC, "negGet"),
            sink.clone(),
            1,
            SinkReply::PinnedJson
        ),
    );

    assert_eq!(
        resp.status, 406,
        "unacceptable representation must fail with 406"
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&resp.body).expect("406 body must parse as JSON");
    assert_eq!(
        parsed["error"], "not_acceptable",
        "406 body must carry the typed error marker, got: {parsed}"
    );
    // Pin the finalizer payload: the message must name BOTH the
    // requested representation and the produced one, or triage loses
    // half the diagnosis.
    let message = parsed["message"]
        .as_str()
        .expect("406 message must be a string");
    assert!(
        message.contains("application/xml"),
        "406 message must pin the requested representation, got: {message}"
    );
    assert!(
        message.contains("application/json"),
        "406 message must pin the produced representation, got: {message}"
    );
    token.cancel();
}

#[tokio::test]
async fn e2e_accept_q0_406() {
    let _guard = SERVER_MUTEX.lock().await;
    let ServerHandle { port, rx, token } = spawn_negotiation_server("/neg/get", "GET").await;
    let sink = Arc::new(AtomicUsize::new(0));

    let req = simple_request(
        "GET",
        "/neg/get",
        &[("Accept", "application/json;q=0")],
        b"",
    );
    let (resp, _) = tokio::join!(
        http_roundtrip(port, req),
        serve_gate(
            rx,
            compiled_gate(NEGOTIATION_DOC, "negGet"),
            sink.clone(),
            1,
            SinkReply::PinnedJson
        ),
    );

    // An explicit quality of zero rejects an otherwise matching entry.
    assert_eq!(resp.status, 406, "Accept q=0 must fail with 406");
    token.cancel();
}

#[tokio::test]
async fn e2e_accept_precedence_q0_406() {
    let _guard = SERVER_MUTEX.lock().await;
    let ServerHandle { port, rx, token } = spawn_negotiation_server("/neg/get", "GET").await;
    let sink = Arc::new(AtomicUsize::new(0));

    let req = simple_request(
        "GET",
        "/neg/get",
        &[("Accept", "application/json;q=0, */*;q=1")],
        b"",
    );
    let (resp, _) = tokio::join!(
        http_roundtrip(port, req),
        serve_gate(
            rx,
            compiled_gate(NEGOTIATION_DOC, "negGet"),
            sink.clone(),
            1,
            SinkReply::PinnedJson
        ),
    );

    // The exact type/subtype entry outranks the wildcard per media-range
    // precedence, so its q=0 governs despite the accepting */*.
    assert_eq!(
        resp.status, 406,
        "specific q=0 must outrank an accepting wildcard"
    );
    token.cancel();
}

#[tokio::test]
async fn e2e_accept_tie_lowest_q_406() {
    let _guard = SERVER_MUTEX.lock().await;
    let ServerHandle { port, rx, token } = spawn_negotiation_server("/neg/get", "GET").await;
    let sink = Arc::new(AtomicUsize::new(0));

    let req = simple_request(
        "GET",
        "/neg/get",
        &[("Accept", "application/json;q=0, application/json;q=1")],
        b"",
    );
    let (resp, _) = tokio::join!(
        http_roundtrip(port, req),
        serve_gate(
            rx,
            compiled_gate(NEGOTIATION_DOC, "negGet"),
            sink.clone(),
            1,
            SinkReply::PinnedJson
        ),
    );

    // Duplicate equal-specificity entries fail closed: the lowest q
    // governs.
    assert_eq!(
        resp.status, 406,
        "equal-specificity tie must take the lowest q"
    );
    token.cancel();
}

#[tokio::test]
async fn e2e_accept_wildcard_passes() {
    let _guard = SERVER_MUTEX.lock().await;
    let ServerHandle { port, rx, token } = spawn_negotiation_server("/neg/get", "GET").await;
    let sink = Arc::new(AtomicUsize::new(0));

    let (full, partial, _) = tokio::join!(
        http_roundtrip(
            port,
            simple_request("GET", "/neg/get", &[("Accept", "*/*")], b"")
        ),
        http_roundtrip(
            port,
            simple_request("GET", "/neg/get", &[("Accept", "application/*")], b"")
        ),
        serve_gate(
            rx,
            compiled_gate(NEGOTIATION_DOC, "negGet"),
            sink.clone(),
            2,
            SinkReply::PinnedJson
        ),
    );

    assert_2xx(full.status, "Accept */*");
    assert_2xx(partial.status, "Accept application/*");
    assert_eq!(
        sink.load(Ordering::SeqCst),
        2,
        "both wildcard requests must reach the sink"
    );
    token.cancel();
}

#[tokio::test]
async fn e2e_accept_multi_entry_passes() {
    let _guard = SERVER_MUTEX.lock().await;
    let ServerHandle { port, rx, token } = spawn_negotiation_server("/neg/get", "GET").await;
    let sink = Arc::new(AtomicUsize::new(0));

    let req = simple_request(
        "GET",
        "/neg/get",
        &[(
            "Accept",
            "text/html, application/xhtml+xml, application/json;q=0.9",
        )],
        b"",
    );
    let (resp, _) = tokio::join!(
        http_roundtrip(port, req),
        serve_gate(
            rx,
            compiled_gate(NEGOTIATION_DOC, "negGet"),
            sink.clone(),
            1,
            SinkReply::PinnedJson
        ),
    );

    // One admissible entry in a longer list is enough (q defaults to 1,
    // explicit 0.9 still accepts).
    assert_2xx(resp.status, "multi-entry Accept with a matching entry");
    token.cancel();
}

#[tokio::test]
async fn e2e_accept_malformed_permissive() {
    let _guard = SERVER_MUTEX.lock().await;
    let ServerHandle { port, rx, token } = spawn_negotiation_server("/neg/get", "GET").await;
    let sink = Arc::new(AtomicUsize::new(0));

    let req = simple_request("GET", "/neg/get", &[("Accept", "garbage header!!")], b"");
    let (resp, _) = tokio::join!(
        http_roundtrip(port, req),
        serve_gate(
            rx,
            compiled_gate(NEGOTIATION_DOC, "negGet"),
            sink.clone(),
            1,
            SinkReply::PinnedJson
        ),
    );

    // A malformed Accept header degrades to permissive — never a 406.
    assert_2xx(resp.status, "malformed Accept header");
    token.cancel();
}

#[tokio::test]
async fn e2e_absent_headers_permissive_pinned_bytes() {
    let _guard = SERVER_MUTEX.lock().await;
    let ServerHandle { port, rx, token } = spawn_negotiation_server("/neg/post", "POST").await;
    let sink = Arc::new(AtomicUsize::new(0));

    // Matching Content-Type, NO Accept header.
    let req = simple_request(
        "POST",
        "/neg/post",
        &[("Content-Type", "application/json")],
        br#"{"name":"kenny"}"#,
    );
    let (resp, _) = tokio::join!(
        http_roundtrip(port, req),
        serve_gate(
            rx,
            compiled_gate(NEGOTIATION_DOC, "negPost"),
            sink.clone(),
            1,
            SinkReply::PinnedJson
        ),
    );

    assert_2xx(resp.status, "absent Accept + matching Content-Type");
    assert_eq!(
        resp.header("content-type"),
        Some("application/json"),
        "the reply must advertise the JSON media"
    );
    // The pin (lives HERE): the permissive pass-through must put exactly
    // these bytes on the wire — no re-wrapping, no re-serialisation, no
    // trailing newline.
    assert_eq!(
        resp.body,
        Bytes::from_static(PINNED_PASS_REPLY),
        "response bytes must equal the inline pin exactly"
    );
    token.cancel();
}

#[tokio::test]
async fn e2e_raw_stream_passthrough_with_negotiation() {
    let _guard = SERVER_MUTEX.lock().await;
    let ServerHandle { port, rx, token } = spawn_negotiation_server("/neg/raw", "POST").await;
    let sink = Arc::new(AtomicUsize::new(0));

    // Chunked framing forces a true streamed request; matching
    // octet-stream negotiation on both sides must pass the gate and the
    // echoed reply must be byte-identical to the request payload.
    let req = chunked_request(
        "POST",
        "/neg/raw",
        &[
            ("Content-Type", "application/octet-stream"),
            ("Accept", "application/octet-stream"),
        ],
        &[b"alpha-", b"beta"],
    );
    let (resp, _) = tokio::join!(
        http_roundtrip(port, req),
        serve_gate(
            rx,
            compiled_gate(NEGOTIATION_DOC, "negRawPost"),
            sink.clone(),
            1,
            SinkReply::EchoStream
        ),
    );

    assert_2xx(resp.status, "raw binding with matching octet-stream media");
    assert_eq!(
        sink.load(Ordering::SeqCst),
        1,
        "the raw op sink must receive the streamed exchange"
    );
    assert_eq!(
        resp.body,
        Bytes::from_static(b"alpha-beta"),
        "the echoed reply must be byte-identical to the streamed request"
    );
    token.cancel();
}
