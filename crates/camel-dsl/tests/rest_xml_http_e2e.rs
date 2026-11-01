//! HTTP-boundary end-to-end pins for the REST DSL `binding: xml` pipeline
//! (change `481-rest-xml-binding`, Task 1.6).
//!
//! `rest_xml_e2e.rs` pins the compiled XML pipeline over an in-memory
//! exchange; this binary closes the last gap by booting a real
//! `camel_component_http::HttpConsumer` and driving the SAME compiled
//! steps for each received envelope, exactly like
//! `rest_negotiation_e2e.rs` Part 2:
//!
//! 1. A malformed request body must fail the compiled `Unmarshal(xml)`
//!    with `TypeConversionFailed`; the error reply goes through the
//!    consumer's own `pipeline_error_to_reply`, which renders 400 with
//!    the typed `bad_request` marker and the parse message.
//! 2. A valid request must complete the whole pipeline (`Unmarshal`,
//!    user step, `Marshal`, `SetHeader(Content-Type)`,
//!    `SetHeaderIfAbsent(status)`) and return 201 (POST default) with the
//!    marshalled XML bytes and `application/xml`.
//!
//! The test never hand-writes the error reply: the malformed case must
//! reach the real compiled `Unmarshal` step, so the 400 comes from the
//! production pipeline. The `Exchange -> Exchange` mutator runs
//! immediately before the compiled `Marshal` step (the same injection
//! point as Task 1.5), pinning the exact JSON projection handed to
//! marshal.
//!
//! Rig notes: the process-global HTTP `ServerRegistry` is shared, so
//! server tests serialize on `SERVER_MUTEX` and boot on a fresh
//! bind-drop port; readiness uses the shared connect-retry helper, never
//! a fixed sleep.

mod common;

use std::time::Duration;

use bytes::Bytes;
use camel_api::{Body, CamelError, Exchange};
use camel_component_api::ExchangeEnvelope;
use camel_component_api::test_support::{TEST_LOCK_DEADLINE, acquire_deadline};
use camel_core::route::BuilderStep;
use camel_dsl::parse_yaml;
use serde_json::json;
use tokio::sync::mpsc;
use tower::ServiceExt;

use common::{
    SERVER_MUTEX, ServerHandle, compile_header_step, http_roundtrip, reply_ok, simple_request,
};

const XML_MEDIA: &str = "application/xml";

/// Compiled XML POST pipeline layout:
/// `[ContentNegotiation, Unmarshal, user, Marshal, SetHeader, SetHeaderIfAbsent]`.
const MARSHAL_STEP_INDEX: usize = 3;

/// POST `binding: xml` route under test: explicit XML media on both
/// sides and one harmless user step between unmarshal and marshal. The
/// declared host/port are never bound — the live rig boots its own
/// consumer; only the lowered + compiled steps are taken from here.
const XML_HTTP_YAML: &str = r#"
rest:
  - host: 127.0.0.1
    port: 18101
    path: /xml
    operations:
      - method: POST
        path: /post
        operation_id: xmlHttpPost
        binding: xml
        consumes: application/xml
        produces: application/xml
        steps:
          - set_header:
              key: X-Xml-Observed
              value: "1"
"#;

/// Fetch the compiled steps of the operation's lowered route and pin the
/// layout the marshal injection index assumes.
fn compiled_steps(yaml: &str, op_id: &str) -> Vec<BuilderStep> {
    let routes = parse_yaml(yaml).expect("xml REST YAML must parse + compile");
    let route = routes
        .iter()
        .find(|r| r.route_id() == op_id)
        .unwrap_or_else(|| panic!("operation '{op_id}' must lower to exactly one route"));
    let steps = route.steps().to_vec();
    assert_eq!(
        steps.len(),
        6,
        "xml POST must compile to exactly [ContentNegotiation, Unmarshal, user, Marshal, SetHeader, SetHeaderIfAbsent], got: {steps:?}"
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
        "step 3 must be the compiled marshal, got: {:?}",
        steps[3]
    );
    steps
}

/// Compile one compiled route step into the Tower service the runtime
/// wires: `Processor` steps unwrap to their pre-built service; header
/// steps compile exactly like camel-core's step compiler (shared
/// helper). `To` and friends cannot be resolved without a
/// `CamelContext` — this document has none.
fn compile_step_for_drive(step: &BuilderStep) -> camel_api::BoxProcessor {
    match step {
        BuilderStep::Processor(op) => op.0.clone(),
        BuilderStep::DeclarativeSetHeader { .. }
        | BuilderStep::DeclarativeSetHeaderIfAbsent { .. } => compile_header_step(step),
        other => panic!("step cannot be driven in this rig: {other:?}"),
    }
}

/// Mutator applied to the exchange immediately before the marshal step.
type Mutator<'a> = &'a dyn Fn(Exchange) -> Exchange;

/// Drive the compiled steps in declared order, propagating the first
/// error. The optional mutator runs immediately before the marshal step
/// (after the user steps), the same injection point as Task 1.5.
async fn drive_steps(
    steps: &[BuilderStep],
    mut ex: Exchange,
    mutator: Option<Mutator<'_>>,
) -> Result<Exchange, CamelError> {
    for (index, step) in steps.iter().enumerate() {
        if index == MARSHAL_STEP_INDEX
            && let Some(mutate) = mutator
        {
            ex = mutate(ex);
        }
        ex = compile_step_for_drive(step).oneshot(ex).await?;
    }
    Ok(ex)
}

/// Server rig shared with the sibling REST e2e binaries lives in
/// `tests/common`; this battery boots the XML POST consumer with fixed
/// 1 MiB body caps under its own diagnostic label.
async fn spawn_xml_server(path: &str, method: &str) -> ServerHandle {
    common::spawn_test_server("rest-xml-http-e2e", path, method, 1024 * 1024, 1024 * 1024).await
}

/// Stand-in for the downstream runtime: for each envelope, drive the
/// compiled steps in order. On `Ok` the fulfilled exchange becomes the
/// reply; on `Err` the typed error goes to the consumer's reply channel,
/// whose `pipeline_error_to_reply` renders the HTTP status and JSON body.
async fn serve_pipeline(
    mut rx: mpsc::Receiver<ExchangeEnvelope>,
    steps: &[BuilderStep],
    expected: usize,
    mutator: Option<Mutator<'_>>,
) {
    for _ in 0..expected {
        // Per-iteration deadline: a stalled pipeline must fail loudly
        // instead of parking this helper; the channel-closed arm keeps
        // the original early-EOF panic.
        let mut envelope = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("envelope receive stalled beyond 10s in serve_pipeline")
            .expect("envelope must reach the pipeline");
        let outcome = drive_steps(steps, envelope.exchange, mutator).await;
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

#[tokio::test]
async fn e2e_malformed_xml_returns_400() {
    let _guard = acquire_deadline(
        &SERVER_MUTEX,
        "SERVER_MUTEX (dsl rest e2e)",
        TEST_LOCK_DEADLINE,
    )
    .await;
    let ServerHandle { port, rx, token } = spawn_xml_server("/xml/post", "POST").await;
    let steps = compiled_steps(XML_HTTP_YAML, "xmlHttpPost");

    let req = simple_request(
        "POST",
        "/xml/post",
        &[("Content-Type", XML_MEDIA)],
        b"<user><name>",
    );
    let (resp, ()) = tokio::join!(
        http_roundtrip(port, req),
        serve_pipeline(rx, &steps, 1, None),
    );

    assert_eq!(
        resp.status, 400,
        "a malformed XML body must fail the compiled unmarshal with 400"
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&resp.body).expect("400 body must parse as JSON");
    assert_eq!(
        parsed["error"], "bad_request",
        "400 body must carry the typed error marker, got: {parsed}"
    );
    let message = parsed["message"]
        .as_str()
        .expect("400 message must be a string");
    assert!(
        message.contains("unexpected end of XML input"),
        "400 message must carry the pinned parse error, got: {message}"
    );
    token.cancel();
}

#[tokio::test]
async fn e2e_valid_xml_returns_201_xml() {
    let _guard = acquire_deadline(
        &SERVER_MUTEX,
        "SERVER_MUTEX (dsl rest e2e)",
        TEST_LOCK_DEADLINE,
    )
    .await;
    let ServerHandle { port, rx, token } = spawn_xml_server("/xml/post", "POST").await;
    let steps = compiled_steps(XML_HTTP_YAML, "xmlHttpPost");

    // Pin the exact JSON projection handed to marshal (after the user
    // step), independent of the unmarshal's leaf-type choices.
    let mutator = |mut ex: Exchange| {
        ex.input.body = Body::Json(json!({"user": {"name": "kenny"}}));
        ex
    };

    let req = simple_request(
        "POST",
        "/xml/post",
        &[("Content-Type", XML_MEDIA)],
        b"<user><name>kenny</name></user>",
    );
    let (resp, ()) = tokio::join!(
        http_roundtrip(port, req),
        serve_pipeline(rx, &steps, 1, Some(&mutator)),
    );

    assert_eq!(
        resp.status, 201,
        "a valid XML POST must return the POST default status"
    );
    assert_eq!(
        resp.header("content-type"),
        Some(XML_MEDIA),
        "the declared produces must be the response Content-Type"
    );
    assert_eq!(
        resp.body,
        Bytes::from_static(b"<user><name>kenny</name></user>"),
        "the response body must be the marshalled XML bytes"
    );
    token.cancel();
}
