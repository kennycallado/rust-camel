//! End-to-end integration tests for the REST DSL `binding: form` pipeline.
//!
//! The form data format (`camel-processor`) and the form binding lowering
//! (`camel-dsl`) are covered by their own unit tests; this file closes the
//! gap by driving the FULL compiled pipeline a real deployment wires:
//!
//! 1. REST YAML (`binding: form`) → `camel_dsl::parse_yaml` → compiled
//!    `RouteDefinition` steps.
//! 2. Each compiled step is driven in declared order over an in-memory
//!    exchange with the production processors the runtime uses (the
//!    negotiation gate and form unmarshal/marshal are `BuilderStep::Processor`
//!    services; header steps compile via `common::compile_header_step`; the
//!    user `set_body` compiles like camel-core's step compiler).
//! 3. Cases A/D observe the exchange right after the user step — the
//!    unmarshalled body is still `Body::Json` there, before the following
//!    marshal re-encodes it to form text.
//!
//! Contract under test (rest-dsl spec: Form binding mode pipeline): the
//! sequence is `ContentNegotiation`, `Unmarshal(form)`, user steps,
//! `Marshal(form)`, `SetHeader(Content-Type)`, `SetHeaderIfAbsent(status)`.
//! Parsing runs before schema validation, so validation sees the composed
//! JSON object.

mod common;

use std::sync::Arc;

use camel_api::{Body, CamelError, Exchange, IdentityProcessor, Message};
use camel_core::route::BuilderStep;
use camel_dsl::{ValueSourceDef, parse_yaml};
use camel_processor::SetBody;
use serde_json::json;
use tower::ServiceExt;

use common::compile_header_step;

const FORM_MEDIA: &str = "application/x-www-form-urlencoded";

/// Basic form route: POST, form media, one harmless user step. The user
/// step is the observation point for the unmarshalled JSON body (Cases
/// A/D) because the following step is the form marshal.
const BASIC_FORM_YAML: &str = r#"
rest:
  - host: 0.0.0.0
    port: 8080
    path: /form
    operations:
      - method: POST
        operation_id: formBasic
        binding: form
        consumes: application/x-www-form-urlencoded
        produces: application/x-www-form-urlencoded
        steps:
          - set_header:
              key: X-Form-Observed
              value: "1"
"#;

/// Response route: the user step supplies the JSON object the form marshal
/// must encode back to form text (Case B).
const RESPONSE_FORM_YAML: &str = r#"
rest:
  - host: 0.0.0.0
    port: 8080
    path: /form
    operations:
      - method: POST
        operation_id: formResponse
        binding: form
        consumes: application/x-www-form-urlencoded
        produces: application/x-www-form-urlencoded
        steps:
          - set_body:
              value:
                status: ok
                tag: [a, b]
"#;

/// Schema route: `request_schema` validates the JSON object produced by the
/// form unmarshal, ordered form-parse then schema-validate (Cases D/E).
const SCHEMA_FORM_YAML: &str = r#"
rest:
  - host: 0.0.0.0
    port: 8080
    path: /form
    operations:
      - method: POST
        operation_id: formSchema
        binding: form
        consumes: application/x-www-form-urlencoded
        produces: application/x-www-form-urlencoded
        request_schema:
          type: object
          required: [name]
          properties:
            name: { type: string }
        steps:
          - set_header:
              key: X-Form-Observed
              value: "1"
"#;

/// Fetch the compiled steps of the operation's lowered route. A form POST
/// lowers to exactly `[ContentNegotiation, Unmarshal(form), <user step>,
/// Marshal(form), SetHeader(Content-Type), SetHeaderIfAbsent(status)]`.
fn compiled_steps(yaml: &str, op_id: &str) -> Vec<BuilderStep> {
    let routes = parse_yaml(yaml).expect("form REST YAML must parse + compile");
    let route = routes
        .iter()
        .find(|r| r.route_id() == op_id)
        .unwrap_or_else(|| panic!("operation '{op_id}' must lower to exactly one route"));
    route.steps().to_vec()
}

/// Compile one compiled route step into the Tower service the runtime
/// wires: Processor steps unwrap to their pre-built service, header steps
/// mirror camel-core's step compiler (via the shared helper), and a literal
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

/// Drive the given compiled steps in order, propagating the first error.
async fn drive_steps(steps: &[BuilderStep], mut ex: Exchange) -> Result<Exchange, CamelError> {
    for step in steps {
        ex = compile_step_for_drive(step).oneshot(ex).await?;
    }
    Ok(ex)
}

/// Build a POST exchange with the form Content-Type the gate requires.
fn form_request(body: Body, content_type: &str) -> Exchange {
    let mut msg = Message::new(body);
    msg.set_header("Content-Type", content_type);
    Exchange::new(msg)
}

#[tokio::test]
async fn form_request_unmarshals_to_json_body() {
    // Case A: the request form body becomes a JSON object with duplicate
    // keys collected as arrays in encounter order. Stop after the user step
    // so the unmarshalled body is observable (the next step marshals it).
    let steps = compiled_steps(BASIC_FORM_YAML, "formBasic");
    assert_eq!(
        steps.len(),
        6,
        "form POST must compile to exactly [ContentNegotiation, Unmarshal, user, Marshal, SetHeader, SetHeaderIfAbsent], got: {steps:?}"
    );

    let ex = form_request(
        Body::Text("name=ada&role=admin&role=dev".to_string()),
        FORM_MEDIA,
    );
    let out = drive_steps(&steps[..3], ex)
        .await
        .expect("a valid form request must unmarshal");

    assert_eq!(
        out.input.body,
        Body::Json(json!({"name": "ada", "role": ["admin", "dev"]})),
        "form unmarshal must produce the composed JSON object"
    );
}

#[tokio::test]
async fn form_response_marshal_from_json() {
    // Case B: the user step sets a JSON object and the form marshal encodes
    // it back to form text; pair order is unspecified, so compare pair-sets.
    let steps = compiled_steps(RESPONSE_FORM_YAML, "formResponse");
    assert_eq!(
        steps.len(),
        6,
        "form POST must compile to exactly 6 steps, got: {steps:?}"
    );

    let ex = form_request(Body::Empty, FORM_MEDIA);
    let out = drive_steps(&steps, ex)
        .await
        .expect("a form response pipeline must complete");

    let Body::Text(text) = out.input.body else {
        panic!(
            "form marshal must produce Body::Text, got: {:?}",
            out.input.body
        );
    };
    let mut pairs: Vec<String> = text.split('&').map(ToString::to_string).collect();
    pairs.sort();
    assert_eq!(
        pairs,
        vec![
            "status=ok".to_string(),
            "tag=a".to_string(),
            "tag=b".to_string()
        ],
        "form response must encode the JSON object as its pair-set, got: {text}"
    );
}

#[tokio::test]
async fn form_gate_opaque_charset_fails_at_unmarshal() {
    // Case C: the negotiation gate treats Content-Type parameters as opaque,
    // so a non-UTF-8 charset passes the gate; the form unmarshal then fails
    // closed on the invalid percent-decoded byte.
    let steps = compiled_steps(BASIC_FORM_YAML, "formBasic");
    let ex = form_request(
        Body::Text("a=%E9".to_string()),
        "application/x-www-form-urlencoded; charset=iso-8859-1",
    );
    let err = drive_steps(&steps, ex)
        .await
        .expect_err("non-UTF-8 percent-decoded bytes must fail the form unmarshal");

    match err {
        CamelError::TypeConversionFailed(msg) => assert!(
            msg.starts_with("form"),
            "form unmarshal error must start with 'form', got: {msg}"
        ),
        other => panic!(
            "expected TypeConversionFailed from the unmarshal (not a gate rejection), got: {other:?}"
        ),
    }
}

#[tokio::test]
async fn form_schema_valid_accepts_parsed_object() {
    // Case D: schema validation runs on the JSON object produced by the form
    // unmarshal, so a schema-satisfying form body completes and the captured
    // body is that object.
    let steps = compiled_steps(SCHEMA_FORM_YAML, "formSchema");
    assert_eq!(
        steps.len(),
        6,
        "form POST with schema must compile to exactly 6 steps, got: {steps:?}"
    );

    let ex = form_request(Body::Text("name=ada".to_string()), FORM_MEDIA);
    let out = drive_steps(&steps[..3], ex)
        .await
        .expect("a schema-satisfying form body must pass validation");

    assert_eq!(
        out.input.body,
        Body::Json(json!({"name": "ada"})),
        "the captured body must be the parsed JSON object"
    );

    // The pipeline must complete: marshal back to form text, then set the
    // response Content-Type and the absent-only status header.
    drive_steps(&steps[3..], out)
        .await
        .expect("the full form pipeline must complete through marshal and header steps");
}

#[tokio::test]
async fn form_schema_invalid_rejects_parsed_object() {
    // Case E: parsing succeeds (the body is valid form text) but validation
    // rejects the composed object for the missing required field — the
    // failure is the schema-validation kind, never a form parse error.
    let steps = compiled_steps(SCHEMA_FORM_YAML, "formSchema");
    let ex = form_request(Body::Text("role=admin".to_string()), FORM_MEDIA);
    let err = drive_steps(&steps, ex)
        .await
        .expect_err("a form body missing the required field must fail validation");

    match err {
        CamelError::ValidationError(msg) => {
            assert!(
                msg.contains("validation failed"),
                "ValidationError should describe the failure, got: {msg}"
            );
            assert!(
                !msg.starts_with("form"),
                "validation failure must not be reported as a form parse error, got: {msg}"
            );
        }
        other => {
            panic!("expected CamelError::ValidationError from schema validation, got: {other:?}")
        }
    }
}
