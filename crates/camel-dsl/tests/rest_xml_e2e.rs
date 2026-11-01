//! End-to-end integration tests for the REST DSL `binding: xml` pipeline.
//!
//! The XML data format (`camel-processor`) and the XML binding lowering
//! (`camel-dsl`) are covered by their own unit tests; this file closes the
//! gap by driving the FULL compiled pipeline a real deployment wires:
//!
//! 1. REST YAML (`binding: xml`) → `camel_dsl::parse_yaml` → compiled
//!    `RouteDefinition` steps.
//! 2. Each compiled step is driven in declared order over an in-memory
//!    exchange with the production processors the runtime uses (the
//!    negotiation gate and XML unmarshal/marshal are `BuilderStep::Processor`
//!    services; header steps compile via `common::compile_header_step`).
//! 3. The `Exchange -> Exchange` mutator runs immediately before the marshal
//!    step — after the user steps — so bodies YAML cannot express
//!    (`Body::Empty`, an exact `Body::Json(value)`) and body observations are
//!    injected at the same point the runtime would hand the body to marshal.
//!
//! Contract under test (rest-dsl spec: XML binding mode pipeline): the
//! sequence is `ContentNegotiation`, `Unmarshal(xml)`, user steps,
//! `Marshal(xml)`, `SetHeader(Content-Type)`, `SetHeaderIfAbsent(status)`.
//! Parsing runs before schema validation, so validation sees the composed
//! JSON projection. The unmarshal fails closed on malformed or empty input,
//! so the user step never runs on a bad request.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bytes::Bytes;
use camel_api::{Body, CamelError, Exchange, IdentityProcessor, Message};
use camel_core::route::BuilderStep;
use camel_dsl::{ValueSourceDef, parse_yaml};
use camel_processor::SetBody;
use serde_json::json;
use tower::ServiceExt;

use common::compile_header_step;

const XML_MEDIA: &str = "application/xml";

/// The compiled pipeline is exactly `[ContentNegotiation, Unmarshal, user,
/// Marshal, SetHeader, SetHeaderIfAbsent]`; the user step sits at index 2 and
/// marshal at index 3.
const USER_STEP_INDEX: usize = 2;
const MARSHAL_STEP_INDEX: usize = 3;

/// Basic XML route: POST, XML media, one harmless user step. The user step is
/// the observation point for the unmarshalled JSON projection and the counter
/// that proves it never ran when the unmarshal fails.
const BASIC_XML_YAML: &str = r#"
rest:
  - host: 0.0.0.0
    port: 8080
    path: /xml
    operations:
      - method: POST
        operation_id: xmlBasic
        binding: xml
        consumes: application/xml
        produces: application/xml
        steps:
          - set_header:
              key: X-Xml-Observed
              value: "1"
"#;

/// Request-schema route: validation runs on the JSON projection the XML
/// unmarshal produced (Cases: schema valid/invalid).
const VALID_SCHEMA_XML_YAML: &str = r#"
rest:
  - host: 0.0.0.0
    port: 8080
    path: /xml
    operations:
      - method: POST
        operation_id: xmlSchemaValid
        binding: xml
        consumes: application/xml
        produces: application/xml
        request_schema:
          type: object
          properties:
            user:
              type: object
        steps:
          - set_header:
              key: X-Xml-Observed
              value: "1"
"#;

/// A schema whose required field the XML projection never carries: parsing
/// succeeds, validation is the failing layer.
const INVALID_SCHEMA_XML_YAML: &str = r#"
rest:
  - host: 0.0.0.0
    port: 8080
    path: /xml
    operations:
      - method: POST
        operation_id: xmlSchemaInvalid
        binding: xml
        consumes: application/xml
        produces: application/xml
        request_schema:
          type: object
          required: [missing]
        steps:
          - set_header:
              key: X-Xml-Observed
              value: "1"
"#;

/// Response-schema route: the schema is OpenAPI metadata, never runtime
/// response validation.
const RESPONSE_SCHEMA_XML_YAML: &str = r#"
rest:
  - host: 0.0.0.0
    port: 8080
    path: /xml
    operations:
      - method: POST
        operation_id: xmlResponseSchema
        binding: xml
        consumes: application/xml
        produces: application/xml
        response:
          schema:
            type: object
            required: [must]
        steps:
          - set_header:
              key: X-Xml-Observed
              value: "1"
"#;

/// Fetch the compiled steps of the operation's lowered route and pin the
/// layout the index constants above assume.
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

/// Compile one compiled route step into the Tower service the runtime wires:
/// Processor steps unwrap to their pre-built service, header steps mirror
/// camel-core's step compiler (via the shared helper), and a literal
/// `set_body` compiles exactly like the core declarative set-body compiler.
fn compile_step_for_drive(step: &BuilderStep) -> camel_api::BoxProcessor {
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

/// Mutator applied to the exchange immediately before the marshal step.
type Mutator<'a> = &'a dyn Fn(Exchange) -> Exchange;

/// Drive the compiled steps in declared order, propagating the first error.
/// The mutator runs immediately before the marshal step (after the user
/// steps); the optional counter records user-step executions so a test can
/// prove the step never ran when an earlier step failed.
async fn drive_steps(
    steps: &[BuilderStep],
    mut ex: Exchange,
    mutator: Option<Mutator<'_>>,
    user_step_runs: Option<&AtomicUsize>,
) -> Result<Exchange, CamelError> {
    for (index, step) in steps.iter().enumerate() {
        if index == MARSHAL_STEP_INDEX
            && let Some(mutate) = mutator
        {
            ex = mutate(ex);
        }
        if index == USER_STEP_INDEX
            && let Some(runs) = user_step_runs
        {
            runs.fetch_add(1, Ordering::SeqCst);
        }
        ex = compile_step_for_drive(step).oneshot(ex).await?;
    }
    Ok(ex)
}

/// Build a POST exchange whose body is the inbound HTTP variant
/// (`Body::Bytes`) with the XML Content-Type the gate requires.
fn xml_request(xml: &str) -> Exchange {
    xml_request_with_body(Body::Bytes(Bytes::from(xml.to_string())))
}

/// Same builder for bodies that are not a string (`Body::Empty`).
fn xml_request_with_body(body: Body) -> Exchange {
    let mut msg = Message::new(body);
    msg.set_header("Content-Type", XML_MEDIA);
    Exchange::new(msg)
}

#[tokio::test]
async fn xml_request_unmarshals_to_json_body() {
    // The request XML becomes a JSON projection with leaf text kept as a
    // string. Record the body right before marshal (after the user step);
    // the next step is the XML marshal.
    let steps = compiled_steps(BASIC_XML_YAML, "xmlBasic");

    let observed = Arc::new(std::sync::Mutex::new(None::<Body>));
    let record = Arc::clone(&observed);
    let mutator = move |ex: Exchange| {
        *record.lock().expect("observation lock") = Some(ex.input.body.clone());
        ex
    };

    let out = drive_steps(
        &steps,
        xml_request("<user><name>kenny</name><n>42</n></user>"),
        Some(&mutator),
        None,
    )
    .await
    .expect("a valid xml request must unmarshal");

    assert!(
        matches!(out.input.body, Body::Text(_)),
        "the pipeline must marshal the projection back to XML text, got: {:?}",
        out.input.body
    );
    let body = observed
        .lock()
        .expect("observation lock")
        .take()
        .expect("the mutator must observe the body before marshal");
    assert_eq!(
        body,
        Body::Json(json!({"user": {"name": "kenny", "n": "42"}})),
        "xml unmarshal must produce the JSON projection (leaf text as string)"
    );
}

#[tokio::test]
async fn xml_response_marshals_from_json() {
    // The user steps leave a JSON object; the mutator supplies it before
    // marshal, which must encode it to XML text. A valid request body keeps
    // the failing-closed unmarshal out of the way.
    let steps = compiled_steps(BASIC_XML_YAML, "xmlBasic");
    let mutator = |mut ex: Exchange| {
        ex.input.body = Body::Json(json!({"user": {"@id": "7", "name": "kenny"}}));
        ex
    };

    let out = drive_steps(&steps, xml_request("<request/>"), Some(&mutator), None)
        .await
        .expect("an xml response pipeline must complete");

    assert_eq!(
        out.input.body,
        Body::Text("<user id=\"7\"><name>kenny</name></user>".to_string()),
        "xml marshal must encode attributes and children"
    );
    assert_eq!(
        out.input.header("Content-Type"),
        Some(&json!("application/xml")),
        "the declared produces must be the response Content-Type"
    );
}

#[tokio::test]
async fn xml_malformed_request_fails_before_user_step() {
    // Truncated XML fails at the unmarshal step; the counter proves the user
    // step after it never executed.
    let steps = compiled_steps(BASIC_XML_YAML, "xmlBasic");
    let user_step_runs = AtomicUsize::new(0);
    let err = drive_steps(
        &steps,
        xml_request("<user><name>"),
        None,
        Some(&user_step_runs),
    )
    .await
    .expect_err("malformed xml must fail the unmarshal");

    assert!(
        matches!(err, CamelError::TypeConversionFailed(_)),
        "expected TypeConversionFailed from the xml unmarshal, got: {err:?}"
    );
    assert_eq!(
        user_step_runs.load(Ordering::SeqCst),
        0,
        "the user step must not run after a failed unmarshal"
    );
}

#[tokio::test]
async fn xml_empty_request_body_fails_closed() {
    // XML unmarshal fails closed on an empty request body (unlike json/form),
    // so the user step is never reached.
    let steps = compiled_steps(BASIC_XML_YAML, "xmlBasic");
    let user_step_runs = AtomicUsize::new(0);
    let err = drive_steps(
        &steps,
        xml_request_with_body(Body::Empty),
        None,
        Some(&user_step_runs),
    )
    .await
    .expect_err("an empty xml request body must fail closed");

    assert!(
        matches!(err, CamelError::TypeConversionFailed(_)),
        "expected TypeConversionFailed from the xml unmarshal, got: {err:?}"
    );
    assert_eq!(
        user_step_runs.load(Ordering::SeqCst),
        0,
        "the user step must not run after an empty-body rejection"
    );
}

#[tokio::test]
async fn xml_doctype_entity_fails_closed() {
    // A custom entity reference is never expanded: the unmarshal fails closed
    // before the user step.
    let steps = compiled_steps(BASIC_XML_YAML, "xmlBasic");
    let user_step_runs = AtomicUsize::new(0);
    let err = drive_steps(
        &steps,
        xml_request(r#"<!DOCTYPE foo [<!ENTITY x "BOOM">]><foo>&x;</foo>"#),
        None,
        Some(&user_step_runs),
    )
    .await
    .expect_err("a custom doctype entity must fail closed");

    assert!(
        matches!(err, CamelError::TypeConversionFailed(_)),
        "expected TypeConversionFailed from the xml unmarshal, got: {err:?}"
    );
    assert_eq!(
        user_step_runs.load(Ordering::SeqCst),
        0,
        "the user step must not run after the doctype rejection"
    );
}

#[tokio::test]
async fn xml_schema_valid_accepts_projection() {
    // The schema validates the JSON projection, not the wire XML.
    let steps = compiled_steps(VALID_SCHEMA_XML_YAML, "xmlSchemaValid");
    let out = drive_steps(
        &steps,
        xml_request("<user><name>kenny</name></user>"),
        None,
        None,
    )
    .await
    .expect("a schema-satisfying projection must pass validation");

    assert!(
        matches!(out.input.body, Body::Text(_)),
        "the pipeline must marshal the validated projection, got: {:?}",
        out.input.body
    );
}

#[tokio::test]
async fn xml_schema_invalid_rejects_projection() {
    // Parsing succeeds; the required field is missing from the projection, so
    // the failure is schema validation, not an XML parse error.
    let steps = compiled_steps(INVALID_SCHEMA_XML_YAML, "xmlSchemaInvalid");
    let err = drive_steps(
        &steps,
        xml_request("<user><name>kenny</name></user>"),
        None,
        None,
    )
    .await
    .expect_err("a projection missing the required field must fail validation");

    assert!(
        matches!(err, CamelError::ValidationError(_)),
        "expected CamelError::ValidationError, got: {err:?}"
    );
}

#[tokio::test]
async fn xml_multi_root_response_fails() {
    // XML can only carry one root element; the marshal step must reject a
    // multi-key object at response time.
    let steps = compiled_steps(BASIC_XML_YAML, "xmlBasic");
    let mutator = |mut ex: Exchange| {
        ex.input.body = Body::Json(json!({"a": 1, "b": 2}));
        ex
    };

    let err = drive_steps(&steps, xml_request("<request/>"), Some(&mutator), None)
        .await
        .expect_err("a multi-root projection must fail the xml marshal");

    assert!(
        matches!(err, CamelError::TypeConversionFailed(_)),
        "expected TypeConversionFailed from the xml marshal, got: {err:?}"
    );
}

#[tokio::test]
async fn xml_response_schema_is_metadata_only() {
    // response.schema is OpenAPI metadata: a non-conforming user body must
    // still marshal to XML without any runtime response validation.
    let steps = compiled_steps(RESPONSE_SCHEMA_XML_YAML, "xmlResponseSchema");
    let mutator = |mut ex: Exchange| {
        ex.input.body = Body::Json(json!({"user": {"name": "kenny"}}));
        ex
    };

    let out = drive_steps(&steps, xml_request("<request/>"), Some(&mutator), None)
        .await
        .expect("response.schema must not validate at runtime");

    assert_eq!(
        out.input.body,
        Body::Text("<user><name>kenny</name></user>".to_string()),
        "the non-conforming body must still marshal to XML"
    );
}

#[tokio::test]
async fn xml_empty_response_body_stays_empty() {
    // marshal is a no-op on Body::Empty; the final header steps still declare
    // the XML Content-Type and default the POST status to 201.
    let steps = compiled_steps(BASIC_XML_YAML, "xmlBasic");
    let mutator = |mut ex: Exchange| {
        ex.input.body = Body::Empty;
        ex
    };

    let out = drive_steps(&steps, xml_request("<request/>"), Some(&mutator), None)
        .await
        .expect("an empty response body must not fail the xml marshal");

    assert_eq!(
        out.input.body,
        Body::Empty,
        "marshal must leave an empty response body empty"
    );
    assert_eq!(
        out.input.header("Content-Type"),
        Some(&json!("application/xml")),
        "the declared produces must be the response Content-Type"
    );
    assert_eq!(
        out.input.header("CamelHttpResponseCode"),
        Some(&json!(201)),
        "POST default status must be injected if-absent"
    );
}
