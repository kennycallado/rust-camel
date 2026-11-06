//! HTTP-boundary pin for ADR-0096 seam blame (change
//! `fault-blame-mapping`, Task 5.1).
//!
//! The binding-injected unmarshal is request-borne: a format failure there
//! is reclassified to `CamelError::ValidationError` (400). A user-authored
//! `unmarshal` placed mid-route is not the seam: it keeps
//! `CamelError::TypeConversionFailed`, which the HTTP finaliser maps to 500
//! `conversion_failed`. This binary proves the distinction with the same
//! format (`json`) and the same operation: the request body is valid JSON,
//! the user `set_body` replaces it with non-JSON text, and the authored
//! unmarshal fails on pipeline data — blame follows the failing step's
//! input provenance, not direction.
//!
//! Rig notes: the process-global HTTP `ServerRegistry` is shared, so server
//! tests serialize on `SERVER_MUTEX` and boot on a fresh bind-drop port;
//! readiness uses the shared connect-retry helper, never a fixed sleep.

mod common;

use camel_component_api::test_support::{TEST_LOCK_DEADLINE, acquire_deadline};
use camel_core::route::BuilderStep;
use camel_dsl::parse_yaml;

use common::{SERVER_MUTEX, ServerHandle, http_roundtrip, serve_pipeline, simple_request};

const JSON_MEDIA: &str = "application/json";

/// Lowered layout for a POST with two user steps:
/// `[ContentNegotiation, Unmarshal(binding), SetBody, Unmarshal(authored),
/// ResponseMarshal, SetHeader, SetHeaderIfAbsent]`.
const SET_BODY_STEP_INDEX: usize = 2;
const AUTHORED_UNMARSHAL_INDEX: usize = 3;

/// POST `binding: json` route under test: the binding unmarshal accepts a
/// valid JSON request, then the user steps replace the body with text and
/// run an authored `unmarshal: json` that fails on that pipeline data. The
/// declared host/port are never bound — the live rig boots its own consumer;
/// only the lowered + compiled steps are taken from here.
const MID_ROUTE_UNMARSHAL_YAML: &str = r#"
rest:
  - host: 127.0.0.1
    port: 18103
    path: /seam
    operations:
      - method: POST
        path: /mid-route
        operation_id: midRouteUnmarshal
        binding: json
        consumes: application/json
        produces: application/json
        steps:
          - set_body: "not json"
          - unmarshal: json
"#;

/// Local step helper: the shared `compiled_rest_steps` hard-pins a six-step
/// layout whose first user step is a `DeclarativeSetHeader`; this route has
/// seven steps and starts with a literal `SetBody`.
fn mid_route_unmarshal_steps(yaml: &str, op_id: &str) -> Vec<BuilderStep> {
    let routes = parse_yaml(yaml).expect("REST YAML must parse + compile");
    let route = routes
        .iter()
        .find(|r| r.route_id() == op_id)
        .unwrap_or_else(|| panic!("operation '{op_id}' must lower to exactly one route"));
    let steps = route.steps().to_vec();
    assert_eq!(
        steps.len(),
        7,
        "mid-route unmarshal POST must compile to [ContentNegotiation, Unmarshal, SetBody, Unmarshal, ResponseMarshal, SetHeader, SetHeaderIfAbsent], got: {steps:?}"
    );
    assert!(
        matches!(
            &steps[SET_BODY_STEP_INDEX],
            BuilderStep::DeclarativeSetBody { .. }
        ),
        "step 2 must be the user set_body, got: {:?}",
        steps[SET_BODY_STEP_INDEX]
    );
    assert!(
        matches!(&steps[AUTHORED_UNMARSHAL_INDEX], BuilderStep::Processor(_)),
        "step 3 must be the authored unmarshal, got: {:?}",
        steps[AUTHORED_UNMARSHAL_INDEX]
    );
    steps
}

/// Server rig shared with the sibling REST e2e binaries lives in
/// `tests/common`; this battery boots the POST consumer with fixed 1 MiB
/// body caps under its own diagnostic label.
async fn spawn_server(path: &str) -> ServerHandle {
    common::spawn_test_server(
        "rest-seam-blame-e2e",
        path,
        "POST",
        1024 * 1024,
        1024 * 1024,
    )
    .await
}

#[tokio::test]
async fn mid_route_unmarshal_failure_maps_to_500() {
    let _guard = acquire_deadline(
        &SERVER_MUTEX,
        "SERVER_MUTEX (dsl rest seam blame e2e)",
        TEST_LOCK_DEADLINE,
    )
    .await;
    let ServerHandle { port, rx, token } = spawn_server("/seam/mid-route").await;
    let steps = mid_route_unmarshal_steps(MID_ROUTE_UNMARSHAL_YAML, "midRouteUnmarshal");

    let req = simple_request(
        "POST",
        "/seam/mid-route",
        &[("Content-Type", JSON_MEDIA)],
        br#"{"valid":true}"#,
    );
    let (resp, ()) = tokio::join!(
        http_roundtrip(port, req),
        serve_pipeline(rx, &steps, 1, None),
    );

    assert_eq!(
        resp.status, 500,
        "an authored mid-route unmarshal failure must be a server error"
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&resp.body).expect("500 body must parse as JSON");
    assert_eq!(
        parsed["error"], "conversion_failed",
        "the authored unmarshal must surface the raw TypeConversionFailed marker, got: {parsed}"
    );
    token.cancel();
}
