//! HTTP-boundary pins for REST response-marshal failures (change
//! `483-rest-xml-followups`, Task 1.5).
//!
//! `rest_xml_http_e2e.rs` covers the mutator-free path: a real malformed
//! request fails the compiled `Unmarshal` and the consumer renders 400.
//! This binary closes the response-side gap. A user step produces a body
//! the compiled automatic `ResponseMarshal` step cannot encode, and the
//! HTTP finaliser must map the wrapped `ProcessorErrorWithSource` to 500 —
//! never 400. The same route must still return 400 for a malformed
//! request, and an authored `marshal: xml` inside `steps:` keeps its raw
//! `TypeConversionFailed` (400) because only the automatic response step
//! is tagged as response-side.
//!
//! The user step at index 2 is replaced with a producer processor, so the
//! 500 path really traverses the compiled `ResponseMarshal` service and
//! the consumer's `pipeline_error_to_reply`; the reply is never
//! hand-written by the test.
//!
//! Rig notes: the process-global HTTP `ServerRegistry` is shared, so
//! server tests serialize on `SERVER_MUTEX` and boot on a fresh
//! bind-drop port; readiness uses the shared connect-retry helper, never
//! a fixed sleep.

mod common;

use std::sync::Arc;

use bytes::Bytes;
use camel_api::{
    Body, BoxProcessor, Exchange, OpaqueProcessor, ProcessorFn, StreamBody, StreamMetadata,
};
use camel_component_api::test_support::{TEST_LOCK_DEADLINE, acquire_deadline};
use camel_core::route::BuilderStep;
use camel_dsl::parse_yaml;
use serde_json::json;

use common::{
    SERVER_MUTEX, ServerHandle, compiled_rest_steps, http_roundtrip, serve_pipeline, simple_request,
};

const JSON_MEDIA: &str = "application/json";
const FORM_MEDIA: &str = "application/x-www-form-urlencoded";
const XML_MEDIA: &str = "application/xml";
const VND_XML_MEDIA: &str = "application/vnd.acme+xml";

/// The automatic response marshal sits at index 3 in the six-step REST
/// pipeline; the user step at index 2 is the replacement point.
const USER_STEP_INDEX: usize = 2;

/// JSON route under test. The declared host/port are never bound — the
/// live rig boots its own consumer; only the lowered + compiled steps are
/// taken from here.
const JSON_MARSHAL_YAML: &str = r#"
rest:
  - host: 127.0.0.1
    port: 18102
    path: /json-marshal
    operations:
      - method: POST
        operation_id: jsonMarshal
        binding: json
        consumes: application/json
        produces: application/json
        steps:
          - set_header:
              key: X-Json-Observed
              value: "1"
"#;

const FORM_MARSHAL_YAML: &str = r#"
rest:
  - host: 127.0.0.1
    port: 18102
    path: /form-marshal
    operations:
      - method: POST
        operation_id: formMarshal
        binding: form
        consumes: application/x-www-form-urlencoded
        produces: application/x-www-form-urlencoded
        steps:
          - set_header:
              key: X-Form-Observed
              value: "1"
"#;

const XML_MARSHAL_YAML: &str = r#"
rest:
  - host: 127.0.0.1
    port: 18102
    path: /xml-marshal
    operations:
      - method: POST
        operation_id: xmlMarshal
        binding: xml
        consumes: application/xml
        produces: application/xml
        steps:
          - set_header:
              key: X-Xml-Observed
              value: "1"
"#;

/// Same XML route, but advertising a vendor XML media type: the declared
/// `produces` must survive verbatim onto the wire.
const XML_VND_MARSHAL_YAML: &str = r#"
rest:
  - host: 127.0.0.1
    port: 18102
    path: /xml-vnd-marshal
    operations:
      - method: POST
        operation_id: xmlVndMarshal
        binding: xml
        consumes: application/xml
        produces: application/vnd.acme+xml
        steps:
          - set_header:
              key: X-Xml-Observed
              value: "1"
"#;

/// An authored `marshal: xml` in the user steps is a step-side marshal,
/// not the automatic response marshal: its failure must stay a raw
/// `TypeConversionFailed` (400), not the wrapped 500.
const XML_AUTHORED_MARSHAL_YAML: &str = r#"
rest:
  - host: 127.0.0.1
    port: 18102
    path: /xml-authored-marshal
    operations:
      - method: POST
        operation_id: xmlAuthoredMarshal
        binding: xml
        consumes: application/xml
        produces: application/xml
        steps:
          - set_body:
              value:
                a: 1
                b: 2
          - marshal: xml
"#;

/// The authored-marshal route carries two user steps, so its layout is
/// `[ContentNegotiation, Unmarshal, set_body, Marshal(authored),
/// ResponseMarshal, SetHeader, SetHeaderIfAbsent]`.
fn authored_steps(yaml: &str, op_id: &str) -> Vec<BuilderStep> {
    let routes = parse_yaml(yaml).expect("authored REST YAML must parse + compile");
    let route = routes
        .iter()
        .find(|r| r.route_id() == op_id)
        .unwrap_or_else(|| panic!("operation '{op_id}' must lower to exactly one route"));
    let steps = route.steps().to_vec();
    assert_eq!(
        steps.len(),
        7,
        "authored-marshal POST must compile to [ContentNegotiation, Unmarshal, set_body, Marshal, ResponseMarshal, SetHeader, SetHeaderIfAbsent], got: {steps:?}"
    );
    steps
}

/// Replace the harmless user `set_header` at index 2 with a processor that
/// swaps the exchange body for the one `producer` returns. Keeping the
/// following steps means the failure reaches the compiled
/// `ResponseMarshal`, not a test-authored error.
fn replace_user_step(
    steps: &mut [BuilderStep],
    producer: impl Fn() -> Body + Send + Sync + 'static,
) {
    assert!(
        matches!(
            &steps[USER_STEP_INDEX],
            BuilderStep::DeclarativeSetHeader { .. }
        ),
        "the replacement point must be the user set_header, got: {:?}",
        steps[USER_STEP_INDEX]
    );
    let producer = Arc::new(producer);
    steps[USER_STEP_INDEX] = BuilderStep::Processor(OpaqueProcessor(BoxProcessor::new(
        ProcessorFn::new(move |mut ex: Exchange| {
            let body = producer();
            async move {
                ex.input.body = body;
                Ok(ex)
            }
        }),
    )));
}

/// Server rig shared with the sibling REST e2e binaries lives in
/// `tests/common`; this battery boots each POST consumer with fixed
/// 1 MiB body caps under its own diagnostic label.
async fn spawn_server(path: &str) -> ServerHandle {
    common::spawn_test_server(
        "rest-marshal-error-http-e2e",
        path,
        "POST",
        1024 * 1024,
        1024 * 1024,
    )
    .await
}

/// The same stream shape `JsonDataFormat` rejects: an empty one-chunk
/// stream, built without polling it.
fn empty_stream_body() -> Body {
    Body::Stream(StreamBody {
        stream: Arc::new(tokio::sync::Mutex::new(Some(Box::pin(
            futures::stream::empty(),
        )))),
        metadata: StreamMetadata::default(),
    })
}

#[tokio::test]
async fn e2e_json_response_marshal_failure_returns_500() {
    let _guard = acquire_deadline(
        &SERVER_MUTEX,
        "SERVER_MUTEX (dsl rest marshal e2e)",
        TEST_LOCK_DEADLINE,
    )
    .await;
    let ServerHandle { port, rx, token } = spawn_server("/json-marshal").await;
    let mut steps = compiled_rest_steps(JSON_MARSHAL_YAML, "jsonMarshal", "ResponseMarshal");
    replace_user_step(&mut steps, empty_stream_body);

    let req = simple_request(
        "POST",
        "/json-marshal",
        &[("Content-Type", JSON_MEDIA)],
        br#"{"a":1}"#,
    );
    let (resp, ()) = tokio::join!(
        http_roundtrip(port, req),
        serve_pipeline(rx, &steps, 1, None),
    );

    assert_eq!(
        resp.status, 500,
        "a response-side JSON marshal failure must be a server error"
    );
    assert_eq!(
        resp.body,
        Bytes::from_static(b"Internal Server Error"),
        "the 500 body must be the generic finaliser text"
    );
    token.cancel();
}

#[tokio::test]
async fn e2e_form_response_marshal_failure_returns_500() {
    let _guard = acquire_deadline(
        &SERVER_MUTEX,
        "SERVER_MUTEX (dsl rest marshal e2e)",
        TEST_LOCK_DEADLINE,
    )
    .await;
    let ServerHandle { port, rx, token } = spawn_server("/form-marshal").await;
    let mut steps = compiled_rest_steps(FORM_MARSHAL_YAML, "formMarshal", "ResponseMarshal");
    replace_user_step(&mut steps, || Body::Json(json!([1, 2])));

    let req = simple_request(
        "POST",
        "/form-marshal",
        &[("Content-Type", FORM_MEDIA)],
        b"a=1",
    );
    let (resp, ()) = tokio::join!(
        http_roundtrip(port, req),
        serve_pipeline(rx, &steps, 1, None),
    );

    assert_eq!(
        resp.status, 500,
        "a response-side form marshal failure must be a server error"
    );
    token.cancel();
}

#[tokio::test]
async fn e2e_xml_response_marshal_failure_returns_500() {
    let _guard = acquire_deadline(
        &SERVER_MUTEX,
        "SERVER_MUTEX (dsl rest marshal e2e)",
        TEST_LOCK_DEADLINE,
    )
    .await;
    let ServerHandle { port, rx, token } = spawn_server("/xml-marshal").await;
    let mut steps = compiled_rest_steps(XML_MARSHAL_YAML, "xmlMarshal", "ResponseMarshal");
    replace_user_step(&mut steps, || {
        Body::Bytes(Bytes::from_static(&[0xFF, 0xFE]))
    });

    let req = simple_request(
        "POST",
        "/xml-marshal",
        &[("Content-Type", XML_MEDIA)],
        b"<root/>",
    );
    let (resp, ()) = tokio::join!(
        http_roundtrip(port, req),
        serve_pipeline(rx, &steps, 1, None),
    );

    assert_eq!(
        resp.status, 500,
        "a response-side XML marshal failure must be a server error"
    );
    token.cancel();
}

#[tokio::test]
async fn e2e_xml_user_step_body_xml_returns_declared_media() {
    let _guard = acquire_deadline(
        &SERVER_MUTEX,
        "SERVER_MUTEX (dsl rest marshal e2e)",
        TEST_LOCK_DEADLINE,
    )
    .await;
    let ServerHandle { port, rx, token } = spawn_server("/xml-marshal").await;
    let mut steps = compiled_rest_steps(XML_MARSHAL_YAML, "xmlMarshal", "ResponseMarshal");
    replace_user_step(&mut steps, || {
        Body::Xml("<user><name>kenny</name></user>".to_string())
    });

    let req = simple_request(
        "POST",
        "/xml-marshal",
        &[("Content-Type", XML_MEDIA)],
        b"<user><name>kenny</name></user>",
    );
    let (resp, ()) = tokio::join!(
        http_roundtrip(port, req),
        serve_pipeline(rx, &steps, 1, None),
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
        "a pre-serialised Body::Xml must pass through the response marshal"
    );
    token.cancel();
}

#[tokio::test]
async fn e2e_xml_user_step_body_xml_uses_vnd_media() {
    let _guard = acquire_deadline(
        &SERVER_MUTEX,
        "SERVER_MUTEX (dsl rest marshal e2e)",
        TEST_LOCK_DEADLINE,
    )
    .await;
    let ServerHandle { port, rx, token } = spawn_server("/xml-vnd-marshal").await;
    let mut steps = compiled_rest_steps(XML_VND_MARSHAL_YAML, "xmlVndMarshal", "ResponseMarshal");
    replace_user_step(&mut steps, || {
        Body::Xml("<user><name>kenny</name></user>".to_string())
    });

    let req = simple_request(
        "POST",
        "/xml-vnd-marshal",
        &[("Content-Type", XML_MEDIA)],
        b"<user><name>kenny</name></user>",
    );
    let (resp, ()) = tokio::join!(
        http_roundtrip(port, req),
        serve_pipeline(rx, &steps, 1, None),
    );

    assert_eq!(
        resp.status, 201,
        "a valid vendor-XML POST must return the POST default status"
    );
    assert_eq!(
        resp.header("content-type"),
        Some(VND_XML_MEDIA),
        "the vendor produces must survive verbatim onto the wire"
    );
    token.cancel();
}

#[tokio::test]
async fn e2e_json_malformed_request_still_returns_400() {
    let _guard = acquire_deadline(
        &SERVER_MUTEX,
        "SERVER_MUTEX (dsl rest marshal e2e)",
        TEST_LOCK_DEADLINE,
    )
    .await;
    let ServerHandle { port, rx, token } = spawn_server("/json-marshal").await;
    let steps = compiled_rest_steps(JSON_MARSHAL_YAML, "jsonMarshal", "ResponseMarshal");

    let req = simple_request(
        "POST",
        "/json-marshal",
        &[("Content-Type", JSON_MEDIA)],
        br#"{"a="#,
    );
    let (resp, ()) = tokio::join!(
        http_roundtrip(port, req),
        serve_pipeline(rx, &steps, 1, None),
    );

    assert_eq!(
        resp.status, 400,
        "a malformed request must stay a client error"
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&resp.body).expect("400 body must parse as JSON");
    assert_eq!(
        parsed["error"], "bad_request",
        "400 body must carry the typed error marker, got: {parsed}"
    );
    token.cancel();
}

#[tokio::test]
async fn e2e_authored_marshal_in_rest_user_steps_stays_400() {
    let _guard = acquire_deadline(
        &SERVER_MUTEX,
        "SERVER_MUTEX (dsl rest marshal e2e)",
        TEST_LOCK_DEADLINE,
    )
    .await;
    let ServerHandle { port, rx, token } = spawn_server("/xml-authored-marshal").await;
    let steps = authored_steps(XML_AUTHORED_MARSHAL_YAML, "xmlAuthoredMarshal");

    let req = simple_request(
        "POST",
        "/xml-authored-marshal",
        &[("Content-Type", XML_MEDIA)],
        b"<request/>",
    );
    let (resp, ()) = tokio::join!(
        http_roundtrip(port, req),
        serve_pipeline(rx, &steps, 1, None),
    );

    assert_eq!(
        resp.status, 400,
        "an authored marshal failure must stay a client error"
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&resp.body).expect("400 body must parse as JSON");
    assert_eq!(
        parsed["error"], "bad_request",
        "authored marshal must surface the raw TypeConversionFailed marker, got: {parsed}"
    );
    token.cancel();
}
