//! End-to-end integration tests for the REST DSL `binding: raw` compile path.
//!
//! Tasks 1.1–1.4 covered the raw-binding pieces at the unit level (field
//! parsing, media validation, lowering). This file closes the gap by
//! exercising the FULL parse+compile paths a real deployment uses:
//!
//! 1. `camel_dsl::yaml::parse_yaml` — YAML → REST lowering → declarative
//!    route → compiled `RouteDefinition` (the runtime artifact).
//! 2. `camel_dsl::yaml::parse_yaml_to_declarative` /
//!    `camel_dsl::json::parse_json_to_declarative` — the authoring-format
//!    parity contract: JSON and YAML must lower to identical declarative
//!    routes.
//! 3. The compiled header steps, driven through the same production
//!    processors the runtime wires (`camel_processor::SetHeader` /
//!    `SetHeaderIfAbsent` over `IdentityProcessor`, mirroring camel-core's
//!    step compiler), proving a `Body::Stream` request survives the raw
//!    pipeline untouched while Content-Type and the default 201 status are
//!    injected.
//!
//! Raw binding contract under test (rest-dsl spec: Raw binding mode
//! pipeline): NO Unmarshal/Marshal steps, `produces` (trimmed) becomes the
//! explicit Content-Type header, and the default status is injected
//! SetHeaderIfAbsent-style as the last step.

use std::sync::Arc;

use bytes::Bytes;
use camel_api::{
    Body, CamelError, Exchange, IdentityProcessor, Message, StreamBody, StreamMetadata,
};
use camel_core::route::BuilderStep;
use camel_dsl::{
    DeclarativeStep, SetHeaderStepDef, ToStepDef, ValueSourceDef, parse_json_to_declarative,
    parse_yaml, parse_yaml_to_declarative,
};
use camel_processor::{SetHeader, SetHeaderIfAbsent};
use futures::stream;
use tokio::sync::Mutex;
use tower::ServiceExt;

const RAW_POST_YAML: &str = r#"
rest:
  - host: 0.0.0.0
    port: 8080
    path: /raw
    operations:
      - method: POST
        operation_id: rawUpload
        binding: raw
        consumes: application/octet-stream
        produces: application/octet-stream
        to: direct:rawSink
"#;

const RAW_POST_JSON: &str = r#"{
    "rest": [
        {
            "host": "0.0.0.0",
            "port": 8080,
            "path": "/raw",
            "operations": [
                {
                    "method": "POST",
                    "path": "/",
                    "operation_id": "rawUpload",
                    "binding": "raw",
                    "consumes": "application/octet-stream",
                    "produces": "application/octet-stream",
                    "to": "direct:rawSink"
                }
            ]
        }
    ]
}"#;

const RAW_STREAM_YAML: &str = r#"
rest:
  - host: 0.0.0.0
    port: 8080
    path: /raw
    operations:
      - method: POST
        operation_id: rawStream
        binding: raw
        consumes: application/octet-stream
        produces: application/octet-stream
        steps:
          - set_header:
              key: X-Trace
              value: t1
"#;

const RAW_SCHEMA_YAML: &str = r#"
rest:
  - host: 0.0.0.0
    port: 8080
    path: /raw
    operations:
      - method: POST
        operation_id: rawUpload
        binding: raw
        consumes: application/octet-stream
        produces: application/octet-stream
        to: direct:rawSink
        request_schema:
          type: object
"#;

/// The exact declarative lowering expected for the raw POST: the user `to`
/// step followed by ONLY the two binding-independent injections — the
/// declared `produces` (trimmed) as Content-Type and the POST default
/// status 201 as SetHeaderIfAbsent. No Unmarshal/Marshal anywhere.
fn expected_raw_post_declarative_steps() -> Vec<DeclarativeStep> {
    vec![
        DeclarativeStep::To(ToStepDef {
            uri: "direct:rawSink".to_string(),
        }),
        DeclarativeStep::SetHeader(SetHeaderStepDef {
            key: "Content-Type".to_string(),
            value: ValueSourceDef::Literal(serde_json::Value::String(
                "application/octet-stream".to_string(),
            )),
        }),
        DeclarativeStep::SetHeaderIfAbsent(SetHeaderStepDef {
            key: "CamelHttpResponseCode".to_string(),
            value: ValueSourceDef::Literal(serde_json::Value::Number(serde_json::Number::from(
                201_u16,
            ))),
        }),
    ]
}

/// Build a `Body::Stream` carrying a single bounded chunk — mirrors the
/// `StreamBody` construction idiom used by camel-api's own tests.
fn one_chunk_stream_body(chunk: &'static str) -> Body {
    let s =
        stream::once(async move { Ok::<Bytes, CamelError>(Bytes::from_static(chunk.as_bytes())) });
    Body::Stream(StreamBody {
        stream: Arc::new(Mutex::new(Some(Box::pin(s)))),
        metadata: StreamMetadata::default(),
    })
}

/// Compile a declarative header `BuilderStep` into the exact Tower service
/// the runtime's step compiler wires for it (camel-core
/// `step_compilers/core.rs`): `SetHeader::new(IdentityProcessor, key, value)`
/// for set_header and the `SetHeaderIfAbsent` twin for the default-status
/// injection. This drives the REAL production processors, not test doubles.
fn compile_header_step(step: &BuilderStep) -> camel_api::BoxProcessor {
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

#[test]
fn raw_post_compiles_to_exactly_three_steps() {
    // Compile path: the full YAML → RouteDefinition lowering must produce
    // exactly three steps (user `to` + Content-Type + default status) —
    // raw binding adds no Unmarshal/Marshal around them.
    let routes = parse_yaml(RAW_POST_YAML).expect("raw REST YAML must parse + compile");
    assert_eq!(routes.len(), 1, "one POST op must lower to one route");
    assert_eq!(
        routes[0].steps().len(),
        3,
        "raw POST must compile to exactly [To, SetHeader, SetHeaderIfAbsent], got: {:?}",
        routes[0].steps()
    );

    // Declarative path: the same YAML, inspected before compilation, must
    // match the expected step vector literally.
    let decl = parse_yaml_to_declarative(RAW_POST_YAML).expect("declarative parse must succeed");
    assert_eq!(decl.len(), 1);
    assert_eq!(decl[0].route_id, "rawUpload");
    assert_eq!(decl[0].steps, expected_raw_post_declarative_steps());
}

#[tokio::test]
async fn raw_pipeline_preserves_stream_body() {
    // Compile the raw-stream route and drive its three steps in order with
    // a Body::Stream exchange. The raw contract: the stream body must come
    // out untouched (no unmarshal/marshal ever touches it), the declared
    // produces must land as Content-Type, and the POST default 201 must be
    // injected if-absent.
    let routes = parse_yaml(RAW_STREAM_YAML).expect("raw stream YAML must parse + compile");
    assert_eq!(routes.len(), 1);
    let steps = routes[0].steps();
    assert_eq!(
        steps.len(),
        3,
        "raw route must have exactly 3 compiled steps, got: {steps:?}"
    );

    let mut ex = Exchange::new(Message::new(one_chunk_stream_body("binary-payload")));

    for (idx, step) in steps.iter().enumerate() {
        let processor = compile_header_step(step);
        let result = processor.oneshot(ex).await;
        assert!(
            result.is_ok(),
            "step {idx} must succeed on a stream body, got: {:?}",
            result.err()
        );
        ex = result.expect("checked is_ok above");
    }

    // Body preserved: still the stream variant, never materialized or
    // re-wrapped into Text/Json/Bytes by the pipeline.
    assert!(
        matches!(ex.input.body, Body::Stream(_)),
        "raw pipeline must preserve Body::Stream, got: {:?}",
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
    // The user set_header step executed (not just returned Ok): its header
    // landed on the final exchange.
    assert_eq!(
        ex.input.header("X-Trace"),
        Some(&serde_json::json!("t1")),
        "user set_header step must have written X-Trace"
    );
}

#[test]
fn json_authored_raw_lowers_like_yaml() {
    // Authoring-format parity (ADR-0026): a raw rest block authored in JSON
    // must lower to the exact same declarative route as the YAML twin —
    // same route id, same from URI, same step vector.
    let json_routes = parse_json_to_declarative(RAW_POST_JSON).expect("raw REST JSON must parse");
    let yaml_routes = parse_yaml_to_declarative(RAW_POST_YAML).expect("raw REST YAML must parse");

    assert_eq!(json_routes.len(), 1);
    assert_eq!(yaml_routes.len(), 1);

    let json_route = &json_routes[0];
    let yaml_route = &yaml_routes[0];

    assert_eq!(json_route.route_id, yaml_route.route_id);
    assert_eq!(json_route.from, yaml_route.from);
    assert_eq!(
        json_route.steps, yaml_route.steps,
        "JSON and YAML must lower raw binding to identical declarative steps"
    );
    // And both match the canonical raw contract.
    assert_eq!(json_route.steps, expected_raw_post_declarative_steps());
}

#[test]
fn raw_yaml_rejects_schema_at_parse() {
    // The JSON-schema hooks are binding:'json'-only. A raw op declaring
    // request_schema must fail the parse itself (not silently drop the
    // schema), with the offending field named in the error.
    let err = match parse_yaml(RAW_SCHEMA_YAML) {
        Ok(routes) => panic!(
            "raw binding with request_schema must be rejected at parse, got {} route(s)",
            routes.len()
        ),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("request_schema"),
        "error must name 'request_schema', got: {err}"
    );
}
