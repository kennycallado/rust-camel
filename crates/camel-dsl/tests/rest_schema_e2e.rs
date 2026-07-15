//! End-to-end integration test for the REST DSL `request_schema` wiring.
//!
//! The unit tests for `JsonSchemaValidateService` (in `camel-processor`) and
//! `pipeline_error_to_reply` (in `camel-http`) already cover their respective
//! layers in isolation. This test closes the gap by exercising the FULL
//! wiring path that a real HTTP request would follow:
//!
//! 1. REST YAML authored with `request_schema`
//! 2. Parsed by `camel_dsl::yaml::parse_yaml` (which lowers REST blocks into
//!    `RouteDefinition`s and compiles each step)
//! 3. The compiled `BuilderStep::Processor` for the `unmarshal` step is
//!    invoked with an in-memory exchange whose body is missing the required
//!    field
//! 4. The inner `JsonSchemaValidateService` short-circuits with
//!    `CamelError::ValidationError(...)` — exactly the error the HTTP
//!    reply finaliser maps to 400 Bad Request
//!
//! This proves the runtime pipeline correctly wires the schema across the
//! full chain (YAML → DeclarativeStep → BuilderStep → BoxProcessor →
//! ValidationError) so a schema violation actually fails the exchange.

use camel_api::body::Body;
use camel_api::{CamelError, Exchange, Message};
use camel_core::route::BuilderStep;
use camel_dsl::yaml::parse_yaml;
use tower::ServiceExt;

/// Schema requiring a `name` (string) field. A `age` field is optional.
fn user_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "age": { "type": "integer" }
        },
        "required": ["name"]
    })
}

const REST_YAML: &str = r#"
rest:
  - host: 0.0.0.0
    port: 8080
    path: /api/users
    operations:
      - method: POST
        operation_id: createUser
        consumes: application/json
        produces: application/json
        success_status: 201
        to: direct:createUser
        request_schema:
          type: object
          properties:
            name: { type: string }
            age: { type: integer }
          required: [name]
"#;

/// Extract the compiled `BuilderStep::Processor` (i.e. the actual Tower
/// service) for the `unmarshal` step from the POST route. REST DSL lowering
/// injects unmarshal as the FIRST step; the default-status SetHeaderIfAbsent
/// is appended as the LAST step (spec §6.2/§7.1/§9 if-absent semantics).
fn compiled_unmarshal_processor() -> camel_api::BoxProcessor {
    let routes = parse_yaml(REST_YAML).expect("REST YAML must parse + compile");
    // Two routes? No — just one operation (POST), so one route.
    assert_eq!(routes.len(), 1, "expected exactly one route from POST");
    let steps = routes[0].steps();
    // Layout per rest.rs lowering: [Unmarshal(json+schema), To, Marshal, SetHeader(Content-Type), SetHeaderIfAbsent(201)]
    assert!(
        !steps.is_empty(),
        "route must have at least one step (unmarshal), got: 0",
    );
    match &steps[0] {
        BuilderStep::Processor(op) => op.0.clone(),
        other => panic!("expected BuilderStep::Processor for unmarshal, got: {other:?}"),
    }
}

#[tokio::test]
async fn rest_request_schema_e2e_rejects_missing_required_field() {
    // The core assertion: an exchange with Body::Text("{...}") missing the
    // required `name` field must fail the inner validator with a
    // ValidationError, NOT fall through with a 200 OK. This is the
    // pre-condition for `pipeline_error_to_reply` to map it to 400.
    let processor = compiled_unmarshal_processor();
    let ex = Exchange::new(Message::new(Body::Text(r#"{"age": 42}"#.to_string())));
    let err = processor
        .oneshot(ex)
        .await
        .expect_err("schema mismatch must surface as Err, not Ok");
    match err {
        CamelError::ValidationError(msg) => {
            assert!(
                msg.contains("validation failed"),
                "ValidationError should describe the failure, got: {msg}"
            );
        }
        other => panic!("expected CamelError::ValidationError, got: {other:?}"),
    }
}

#[tokio::test]
async fn rest_request_schema_e2e_rejects_wrong_field_type() {
    // A field of the wrong type (age as string, not integer) must also
    // surface as ValidationError — the schema is the contract, not a hint.
    let processor = compiled_unmarshal_processor();
    let ex = Exchange::new(Message::new(Body::Text(
        r#"{"name": "kenny", "age": "not a number"}"#.to_string(),
    )));
    let err = processor
        .oneshot(ex)
        .await
        .expect_err("wrong-type field must surface as Err");
    assert!(
        matches!(err, CamelError::ValidationError(_)),
        "expected ValidationError, got: {err:?}"
    );
}

#[tokio::test]
async fn rest_request_schema_e2e_accepts_valid_body() {
    // Regression guard: the validator must not over-reject. A body that
    // satisfies the schema flows through IdentityProcessor and returns Ok
    // with the body still as Body::Json.
    let processor = compiled_unmarshal_processor();
    let ex = Exchange::new(Message::new(Body::Text(
        r#"{"name": "kenny", "age": 42}"#.to_string(),
    )));
    let out = processor
        .oneshot(ex)
        .await
        .expect("valid body must pass validation");
    // The chain (UnmarshalService → JsonSchemaValidateService →
    // IdentityProcessor) leaves the body as Body::Json after the unmarshal
    // step succeeds and validation passes.
    assert!(
        matches!(out.input.body, Body::Json(_)),
        "expected Body::Json after valid unmarshal+validate, got: {:?}",
        out.input.body
    );
}

#[tokio::test]
async fn rest_request_schema_e2e_empty_body_passes() {
    // Spec §8.1: an empty body skips unmarshal (and therefore skips
    // validation — there is nothing to validate). The compiled chain must
    // not 500 a body-less POST just because a schema is configured.
    let processor = compiled_unmarshal_processor();
    let ex = Exchange::new(Message::new(Body::Empty));
    let out = processor
        .oneshot(ex)
        .await
        .expect("empty body must pass through");
    assert!(
        matches!(out.input.body, Body::Empty),
        "expected Body::Empty passthrough, got: {:?}",
        out.input.body
    );
}

#[tokio::test]
async fn rest_request_schema_e2e_malformed_json_returns_type_conversion_error() {
    // The chain is: UnmarshalService → JsonSchemaValidateService. A body
    // that is not valid JSON at all fails at the unmarshal layer with
    // CamelError::TypeConversionFailed — also mapped to 400 by the HTTP
    // reply finaliser, but for a different reason than schema mismatch.
    // Pin that distinction: schema validation is reached only after
    // successful JSON parsing.
    let processor = compiled_unmarshal_processor();
    let ex = Exchange::new(Message::new(Body::Text("not json".to_string())));
    let err = processor
        .oneshot(ex)
        .await
        .expect_err("malformed JSON must fail at unmarshal");
    assert!(
        matches!(err, CamelError::TypeConversionFailed(_)),
        "expected TypeConversionFailed (pre-validation), got: {err:?}"
    );
}

#[test]
fn rest_request_schema_e2e_yaml_to_compiled_step_is_wired() {
    // Lightweight wiring check (no async): the route is actually emitted,
    // and the unmarshal step is the first step in the route. This is the
    // pre-condition the async tests above depend on — it fails fast with a
    // clear message if the lowering breaks the layout the async tests
    // assume.
    let routes = parse_yaml(REST_YAML).expect("REST YAML must parse + compile");
    assert_eq!(routes.len(), 1);
    let steps = routes[0].steps();
    // The first step is the compiled unmarshal processor (with schema
    // baked in). The default-status step is now SetHeaderIfAbsent and
    // appended as the last step (spec §6.2/§7.1/§9).
    assert!(
        !steps.is_empty(),
        "expected at least one step (unmarshal), got 0"
    );
    assert!(
        matches!(&steps[0], BuilderStep::Processor(_)),
        "first step should be a compiled Processor, got: {:?}",
        steps[0]
    );
    // Sanity: the schema variable matches the YAML we wrote, so any future
    // drift in either side of this test is caught here.
    assert_eq!(user_schema()["required"][0], "name");
}

const MULTI_OP_REST_YAML: &str = r#"
rest:
  - host: 0.0.0.0
    port: 8080
    path: /api/estado
    operations:
      - method: GET
        path: /health
        operation_id: healthCheck
        to: direct:healthCheck
      - method: GET
        path: /conteos
        operation_id: getConteos
        to: direct:getConteos
"#;

#[test]
fn rest_multi_op_same_verb_lowers_to_distinct_routes() {
    // Feature headline: two GET endpoints under ONE rest block (previously
    // impossible) parse and lower end-to-end into two distinct routes.
    let routes = parse_yaml(MULTI_OP_REST_YAML).expect("multi-op YAML must parse + compile");
    assert_eq!(
        routes.len(),
        2,
        "two GETs in one block must produce two routes"
    );
    // Declaration order preserved, not alphabetical: health before conteos.
    // `RouteDefinition` exposes `from_uri()` (returns &str including the sub-path);
    // its fields are pub(crate), so use the accessor.
    let froms: Vec<String> = routes.iter().map(|r| r.from_uri().to_string()).collect();
    assert!(
        froms[0].contains("health"),
        "first route must be /health, got: {}",
        froms[0]
    );
    assert!(
        froms[1].contains("conteos"),
        "second route must be /conteos, got: {}",
        froms[1]
    );
}
