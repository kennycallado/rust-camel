// Every REST e2e binary compiles this module but exercises only the
// helper family its battery needs (the raw binary drives header
// steps; the HTTP suites use the server rig), so cross-binary items
// read as dead in any single binary.
#![allow(dead_code)]

//! Shared test rig for the camel-dsl REST e2e binaries
//! (`rest_negotiation_e2e`, `rest_raw_e2e`,
//! `rest_stream_contract_e2e`, `rest_xml_http_e2e`,
//! `rest_marshal_error_http_e2e`).
//!
//! One home for the HTTP-boundary rig that previously lived as three
//! verbatim per-file copies (pattern precedent:
//! `camel-integration-test/tests/common/mod.rs`).
//!
//! Two helper families, deliberately separate:
//!
//! - HTTP server rig: [`SERVER_MUTEX`] serializes server tests inside
//!   one binary, [`spawn_test_server`] boots a real
//!   `camel_component_http::HttpConsumer` on a fresh bind-drop port
//!   with the channel receiver standing in for the downstream
//!   runtime, and the [`RawResponse`] / [`http_roundtrip`] /
//!   [`simple_request`] / [`chunked_request`] family is a
//!   hand-rolled TCP HTTP/1.1 client. Readiness uses a connect-retry
//!   loop, never a fixed sleep.
//! - Step-level helpers: [`compile_header_step`] compiles a
//!   declarative header step into the exact Tower service camel-core
//!   wires, and [`counting_stream_body`] builds a poll-counting
//!   `Body::Stream` for never-touched-the-stream proofs.
//! - Step-drive family: [`compiled_rest_steps`] pins the six-step REST
//!   layout, [`compile_step_for_drive`] turns one compiled step into
//!   the Tower service the runtime wires, [`drive_steps`] runs the
//!   whole pipeline with an optional injection point, and
//!   [`serve_pipeline`] is the envelope runner for the live HTTP rig.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use camel_api::{Body, CamelError, Exchange, IdentityProcessor, StreamBody, StreamMetadata};
use camel_component_api::{Consumer, ConsumerContext, ExchangeEnvelope, NoopRuntimeObservability};
use camel_component_http::{HttpConsumer, HttpServerConfig};
use camel_core::route::BuilderStep;
use camel_dsl::{ValueSourceDef, parse_yaml};
use camel_processor::{SetBody, SetHeader, SetHeaderIfAbsent};
use futures::stream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

/// Serializes server tests inside one test binary: the process-global
/// HTTP `ServerRegistry` is shared by every test, paths are not a
/// uniqueness device (last-write-wins), so server tests take this
/// mutex and each boot on a fresh bind-drop port (same discipline as
/// camel-http's in-crate REGISTRY_TEST_MUTEX and the sibling e2e
/// rigs).
pub static SERVER_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Handle to a booted test server: the port to dial, the receiver
/// standing in for the downstream pipeline, and the shutdown token.
pub struct ServerHandle {
    pub port: u16,
    pub rx: mpsc::Receiver<ExchangeEnvelope>,
    pub token: CancellationToken,
}

/// Boot a real `HttpConsumer` on a free port (bind-drop idiom). The
/// channel receiver replaces the downstream pipeline; each test
/// fulfills received envelopes by hand, exactly like camel-http's
/// in-crate rig. `label` names the consumer in diagnostics; the body
/// caps are parameters because the stream-contract suite pins
/// rejection at both limits.
pub async fn spawn_test_server(
    label: &str,
    path: &str,
    method: &str,
    max_request_body: usize,
    max_response_body: usize,
) -> ServerHandle {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind probe");
    let port = listener.local_addr().expect("port").port();
    drop(listener);

    let cfg = HttpServerConfig {
        scheme: "http".to_string(),
        host: "127.0.0.1".to_string(),
        port,
        path: path.to_string(),
        max_request_body,
        max_response_body,
        max_inflight_requests: 64,
        method: Some(method.to_string()),
        tls_config: None,
    };
    let mut consumer = HttpConsumer::new(cfg, Arc::new(NoopRuntimeObservability));
    let (tx, rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let token = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, token.clone(), label.to_string());
    tokio::spawn(async move {
        consumer.start(ctx).await.expect("consumer must start");
    });
    wait_listening(port).await;

    ServerHandle { port, rx, token }
}

/// Retry-connect until the axum listener accepts — no fixed sleeps.
async fn wait_listening(port: u16) {
    for _ in 0..150 {
        // Per-attempt deadline (lintwiden D4.3): a wedged listener must
        // fail the attempt (and the retry loop) instead of parking the
        // helper; refusal — the normal not-yet-listening case — retries.
        if matches!(
            tokio::time::timeout(
                Duration::from_millis(500),
                tokio::net::TcpStream::connect(("127.0.0.1", port)),
            )
            .await,
            Ok(Ok(_))
        ) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("server on port {port} did not accept connections within 3s");
}

pub struct RawResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
}

impl RawResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Minimal HTTP/1.1 round trip: write the request, read the response to
/// EOF (`Connection: close`). Tolerates a connection reset after the
/// server wrote the response but before a clean FIN (possible when the
/// request body was not fully drained server-side).
pub async fn http_roundtrip(port: u16, request: String) -> RawResponse {
    let mut sock = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::TcpStream::connect(("127.0.0.1", port)),
    )
    .await
    .expect("connect to test server timed out after 5s")
    .expect("connect to test server");
    sock.write_all(request.as_bytes())
        .await
        .expect("write request");
    let mut buf = Vec::new();
    loop {
        let mut chunk = [0u8; 4096];
        match sock.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
            Err(e) => panic!("read response: {e}"),
        }
    }
    parse_response(&buf)
}

fn parse_response(raw: &[u8]) -> RawResponse {
    let header_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("response must contain a header terminator");
    let head = String::from_utf8_lossy(&raw[..header_end]);
    let mut lines = head.split("\r\n");
    let status_line = lines.next().expect("status line");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("numeric status");
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect();
    let mut body = Bytes::copy_from_slice(&raw[header_end + 4..]);
    let chunked = headers
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("transfer-encoding") && v.contains("chunked"));
    if chunked {
        body = dechunk(&body);
    }
    RawResponse {
        status,
        headers,
        body,
    }
}

/// Decode HTTP/1.1 chunked framing.
fn dechunk(mut input: &[u8]) -> Bytes {
    let mut out = Vec::new();
    while let Some(line_end) = input.windows(2).position(|w| w == b"\r\n") {
        let size_str = String::from_utf8_lossy(&input[..line_end]);
        let size_str = size_str.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_str, 16).expect("chunk size hex");
        input = &input[line_end + 2..];
        if size == 0 {
            break;
        }
        assert!(
            input.len() >= size,
            "truncated chunk body in test response ({size} declared)"
        );
        out.extend_from_slice(&input[..size]);
        input = &input[size..];
        assert!(input.starts_with(b"\r\n"), "chunk must end with CRLF");
        input = &input[2..];
    }
    Bytes::from(out)
}

/// `Content-Length` request with optional extra headers. All test
/// bodies are ASCII; writing them inline keeps the helper a single
/// String.
pub fn simple_request(method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> String {
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: rig-test\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if !body.is_empty() || method == "POST" {
        req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    req.push_str("Connection: close\r\n\r\n");
    req.push_str(&String::from_utf8_lossy(body));
    req
}

/// Chunked request framing (forces a true streamed body — no
/// Content-Length pre-check can short-circuit the transport).
pub fn chunked_request(
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    chunks: &[&[u8]],
) -> String {
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: rig-test\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n");
    for chunk in chunks {
        req.push_str(&format!("{:x}\r\n", chunk.len()));
        req.push_str(&String::from_utf8_lossy(chunk));
        req.push_str("\r\n");
    }
    req.push_str("0\r\n\r\n");
    req
}

/// Fulfil an envelope with its (successful) exchange.
pub fn reply_ok(mut envelope: ExchangeEnvelope) {
    if let Some(reply_tx) = envelope.reply_tx.take() {
        let _ = reply_tx.send(Ok(envelope.exchange));
    }
}

/// Compile a declarative header `BuilderStep` into the exact Tower service
/// the runtime's step compiler wires for it (camel-core
/// `step_compilers/core.rs`): `SetHeader::new(IdentityProcessor, key, value)`
/// for set_header and the `SetHeaderIfAbsent` twin for the default-status
/// injection. This drives the REAL production processors, not test doubles.
pub fn compile_header_step(step: &BuilderStep) -> camel_api::BoxProcessor {
    match step {
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
        other => panic!("expected declarative header step, got: {other:?}"),
    }
}

/// Mutator applied to the exchange immediately before the injected step
/// index. The step-drive rig runs it after the steps ahead of that index
/// and before the step at the index itself.
pub type Mutator<'a> = &'a dyn Fn(Exchange) -> Exchange;

/// Fetch the compiled steps of a REST operation's lowered route and pin
/// the six-step layout every REST e2e rig assumes:
/// `[ContentNegotiation, Unmarshal, user, <marshal_label>, SetHeader,
/// SetHeaderIfAbsent]`.
pub fn compiled_rest_steps(yaml: &str, op_id: &str, marshal_label: &str) -> Vec<BuilderStep> {
    let routes = parse_yaml(yaml).expect("REST YAML must parse + compile");
    let route = routes
        .iter()
        .find(|r| r.route_id() == op_id)
        .unwrap_or_else(|| panic!("operation '{op_id}' must lower to exactly one route"));
    let steps = route.steps().to_vec();
    assert_eq!(
        steps.len(),
        6,
        "POST must compile to exactly [ContentNegotiation, Unmarshal, user, {marshal_label}, SetHeader, SetHeaderIfAbsent], got: {steps:?}"
    );
    assert!(
        matches!(&steps[0], BuilderStep::Processor(_)),
        "step 0 must be the content-negotiation gate, got: {:?}",
        steps[0]
    );
    assert!(
        matches!(&steps[1], BuilderStep::Processor(_)),
        "step 1 must be the compiled unmarshal, got: {:?}",
        steps[1]
    );
    assert!(
        matches!(&steps[2], BuilderStep::DeclarativeSetHeader { .. }),
        "step 2 must be the user set_header, got: {:?}",
        steps[2]
    );
    assert!(
        matches!(&steps[3], BuilderStep::Processor(_)),
        "step 3 must be the compiled {marshal_label}, got: {:?}",
        steps[3]
    );
    steps
}

/// Compile one compiled route step into the Tower service the runtime
/// wires: `Processor` steps unwrap to their pre-built service; header
/// steps compile exactly like camel-core's step compiler (shared helper);
/// a literal `set_body` compiles exactly like the core declarative
/// set-body compiler. `To` and friends cannot be resolved without a
/// `CamelContext` — this document has none.
pub fn compile_step_for_drive(step: &BuilderStep) -> camel_api::BoxProcessor {
    match step {
        BuilderStep::Processor(op) => op.0.clone(),
        BuilderStep::DeclarativeSetHeader { .. }
        | BuilderStep::DeclarativeSetHeaderIfAbsent { .. } => compile_header_step(step),
        BuilderStep::DeclarativeSetBody { value } => match value {
            ValueSourceDef::Literal(body_value) => {
                let body_value = body_value.clone();
                let source = Arc::new(move |_: &Exchange| body_value.clone())
                    as Arc<dyn Fn(&Exchange) -> serde_json::Value + Send + Sync>;
                camel_api::BoxProcessor::new(SetBody::new(IdentityProcessor, source))
            }
            other => panic!("expected literal set_body value, got {other:?}"),
        },
        other => panic!("step cannot be driven in this rig: {other:?}"),
    }
}

/// Drive the compiled steps in declared order, propagating the first
/// error. When `inject` is present, its mutator runs immediately before
/// the step at the given index (after every earlier step).
pub async fn drive_steps(
    steps: &[BuilderStep],
    mut ex: Exchange,
    inject: Option<(usize, Mutator<'_>)>,
) -> Result<Exchange, CamelError> {
    for (index, step) in steps.iter().enumerate() {
        if let Some((at, mutate)) = inject
            && index == at
        {
            ex = mutate(ex);
        }
        ex = compile_step_for_drive(step).oneshot(ex).await?;
    }
    Ok(ex)
}

/// Stand-in for the downstream runtime: for each envelope, drive the
/// compiled steps in order. On `Ok` the fulfilled exchange becomes the
/// reply; on `Err` the typed error goes to the consumer's reply channel,
/// whose `pipeline_error_to_reply` renders the HTTP status and body.
pub async fn serve_pipeline(
    mut rx: mpsc::Receiver<ExchangeEnvelope>,
    steps: &[BuilderStep],
    expected: usize,
    inject: Option<(usize, Mutator<'_>)>,
) {
    for _ in 0..expected {
        // Per-iteration deadline: a stalled pipeline must fail loudly
        // instead of parking this helper; the channel-closed arm keeps
        // the original early-EOF panic.
        let mut envelope = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("envelope receive stalled beyond 10s in serve_pipeline")
            .expect("envelope must reach the pipeline");
        let outcome = drive_steps(steps, envelope.exchange, inject).await;
        match outcome {
            Ok(passed) => {
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

/// A one-chunk stream body that counts every poll of its underlying
/// future, carrying the given metadata. Zero polls after the gate or
/// pipeline proves neither touched the stream.
pub fn counting_stream_body(
    chunk: &'static str,
    polls: Arc<AtomicUsize>,
    metadata: StreamMetadata,
) -> Body {
    let s = stream::once({
        let polls = polls.clone();
        async move {
            polls.fetch_add(1, Ordering::SeqCst);
            Ok::<Bytes, CamelError>(Bytes::from_static(chunk.as_bytes()))
        }
    });
    Body::Stream(StreamBody {
        stream: Arc::new(Mutex::new(Some(Box::pin(s)))),
        metadata,
    })
}
