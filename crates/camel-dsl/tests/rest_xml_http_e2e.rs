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

use bytes::Bytes;
use camel_api::{Body, Exchange};
use camel_component_api::test_support::{TEST_LOCK_DEADLINE, acquire_deadline};
use serde_json::json;

use common::{
    SERVER_MUTEX, ServerHandle, compiled_rest_steps, http_roundtrip, serve_pipeline, simple_request,
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

/// Server rig shared with the sibling REST e2e binaries lives in
/// `tests/common`; this battery boots the XML POST consumer with fixed
/// 1 MiB body caps under its own diagnostic label.
async fn spawn_xml_server(path: &str, method: &str) -> ServerHandle {
    common::spawn_test_server("rest-xml-http-e2e", path, method, 1024 * 1024, 1024 * 1024).await
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
    let steps = compiled_rest_steps(XML_HTTP_YAML, "xmlHttpPost", "Marshal");

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
    let steps = compiled_rest_steps(XML_HTTP_YAML, "xmlHttpPost", "Marshal");

    // Pin the exact JSON projection handed to marshal (after the user
    // step), independent of the unmarshal's leaf-type choices.
    let mutator = |mut ex: Exchange| {
        ex.input.body = Body::Json(json!({"user": {"name": "kenny"}}));
        ex
    };

    // 489 boundary: a route Content-Type equal to the inbound value is
    // inherited and omitted (body inference then wins). Use a parameterized
    // inbound value so the route's exact `application/xml` is distinct and
    // must be emitted. The equal-value boundary is pinned in
    // rest_negotiation_e2e.
    let req = simple_request(
        "POST",
        "/xml/post",
        &[("Content-Type", "application/xml; charset=utf-8")],
        b"<user><name>kenny</name></user>",
    );
    let (resp, ()) = tokio::join!(
        http_roundtrip(port, req),
        serve_pipeline(rx, &steps, 1, Some((MARSHAL_STEP_INDEX, &mutator))),
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
