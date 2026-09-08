//! End-to-end tests for the `on_exceptions` wildcard clause (`kind: "*"`).
//!
//! OpenSpec change `on-exceptions-wildcard`, Task oe-wc-2. Proves at HTTP
//! level that one wildcard clause with `handled: true` + `retry.handled_by`
//! gives the handler route full ownership of the HTTP response (status,
//! body, headers) for every error kind, and that a specific clause placed
//! before the wildcard wins (first-match-wins).
//!
//! Tests 1–2 pin engine behavior via the builder API (pass before and after
//! Task 1); test 3 exercises the new declarative wildcard through the JSON
//! compile path (`camel_dsl::json::parse_json`, which parses AND compiles).

mod support;
use support::stage_http_listener;

use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{Body, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::CamelError;
use camel_component_http::HttpComponent;
use camel_test::CamelTestContext;

/// Shape the HTTP reply completely: body, custom header (STRING value —
/// non-string values are dropped by the reply finaliser, bd rc-lidtk), and
/// response status. Ends at `mock_sink` so tests can count shaper
/// invocations.
fn shaper_route(from: &'static str, mock_sink: &'static str) -> camel_core::route::RouteDefinition {
    RouteBuilder::from(from)
        .route_id(format!("shaper-{from}"))
        .set_body("shaped")
        .set_header("X-Custom", Value::String("yes".into()))
        .set_header("CamelHttpResponseCode", Value::Number(422.into()))
        .to(mock_sink)
        .build()
        .unwrap()
}

/// The request body as text. The HTTP consumer delivers request bodies as
/// `Body::Stream`, so the body must be materialized before text matching.
async fn request_text(body: Body, max_size: usize) -> String {
    let bytes = body.into_bytes(max_size).await.unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

// ---------------------------------------------------------------------------
// Test 1: wildcard clause owns the HTTP response for every error kind
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn wildcard_owns_response_for_every_kind() {
    let port = stage_http_listener("127.0.0.1").await;
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_direct()
        .with_mock()
        .build()
        .await;

    let shaper = shaper_route("direct:shaper", "mock:shaped");

    let main = RouteBuilder::from(&format!("http://127.0.0.1:{port}/probe"))
        .route_id("wildcard-probe")
        .process(|ex| async move {
            let body = request_text(ex.input.body, 1024 * 1024).await;
            if body.contains("invalid") {
                Err(CamelError::ValidationError("schema mismatch".into()))
            } else {
                Err(CamelError::ProcessorError("boom".into()))
            }
        })
        .error_handler(
            ErrorHandlerConfig::log_only()
                .on_exception(|_e| true)
                .handled_by("direct:shaper")
                .handled(true)
                .build(),
        )
        .build()
        .unwrap();

    h.add_route(shaper).await.unwrap();
    h.add_route(main).await.unwrap();
    h.start().await;

    let client = reqwest::Client::new();
    for body in ["invalid payload", "other payload"] {
        let resp = client
            .post(format!("http://127.0.0.1:{port}/probe"))
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 422, "body {body:?}: shaper owns the status");
        assert_eq!(
            resp.headers().get("x-custom").unwrap(),
            "yes",
            "body {body:?}: shaper owns the headers"
        );
        assert_eq!(resp.text().await.unwrap(), "shaped");
    }

    // Both error kinds were shaped by the same handler route.
    h.mock()
        .get_endpoint("shaped")
        .unwrap()
        .assert_exchange_count(2)
        .await;

    h.stop().await;
}

// ---------------------------------------------------------------------------
// Test 2: specific clause placed first wins over the wildcard
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn specific_clause_precedes_wildcard() {
    let port = stage_http_listener("127.0.0.1").await;
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_direct()
        .with_mock()
        .build()
        .await;

    // Wildcard handler (distinct mock sink so counts are absolute).
    let generic_shaper = shaper_route("direct:generic-shaper", "mock:shaped-ordering");

    let main = RouteBuilder::from(&format!("http://127.0.0.1:{port}/probe"))
        .route_id("ordering-probe")
        .process(|ex| async move {
            let body = request_text(ex.input.body, 1024 * 1024).await;
            if body.contains("io") {
                Err(CamelError::Io("disk".into()))
            } else {
                Err(CamelError::ValidationError("schema mismatch".into()))
            }
        })
        // Trailing step: runs only on the continued path.
        .set_body("recovered")
        .error_handler(
            // FIRST clause: `Io` continues the pipeline (first-match-wins).
            ErrorHandlerConfig::log_only()
                .on_exception(|e| matches!(e, CamelError::Io(_)))
                .continued(true)
                .build()
                // SECOND clause: wildcard hands the response to the shaper.
                .on_exception(|_e| true)
                .handled_by("direct:generic-shaper")
                .handled(true)
                .build(),
        )
        .build()
        .unwrap();

    h.add_route(generic_shaper).await.unwrap();
    h.add_route(main).await.unwrap();
    h.start().await;

    let client = reqwest::Client::new();

    // `Io` path: the specific continued clause won — pipeline finished with
    // the trailing body and the wildcard shaper never ran.
    let resp = client
        .post(format!("http://127.0.0.1:{port}/probe"))
        .body("io")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "continued path replies with success");
    assert_eq!(resp.text().await.unwrap(), "recovered");
    h.mock()
        .get_endpoint("shaped-ordering")
        .unwrap()
        .assert_exchange_count(0)
        .await;

    // Other kinds fall through to the wildcard: the shaper owns the response.
    let resp = client
        .post(format!("http://127.0.0.1:{port}/probe"))
        .body("invalid")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 422);
    assert_eq!(resp.headers().get("x-custom").unwrap(), "yes");
    assert_eq!(resp.text().await.unwrap(), "shaped");
    h.mock()
        .get_endpoint("shaped-ordering")
        .unwrap()
        .assert_exchange_count(1)
        .await;

    h.stop().await;
}

// ---------------------------------------------------------------------------
// Test 3: the wildcard clause compiles from the declarative JSON DSL
// ---------------------------------------------------------------------------

#[test]
fn wildcard_compiles_from_json() {
    let json = r#"{
        "routes": [
            {
                "id": "wildcard-json",
                "from": "direct:json-probe",
                "error_handler": {
                    "on_exceptions": [
                        {
                            "kind": "*",
                            "handled": true,
                            "retry": {
                                "handled_by": "direct:shaper",
                                "max_attempts": 1
                            }
                        }
                    ]
                },
                "steps": [
                    { "to": "log:json-probe" }
                ]
            }
        ]
    }"#;

    // `parse_json` parses AND compiles, returning RouteDefinitions directly.
    let routes = camel_dsl::json::parse_json(json).unwrap();
    assert_eq!(routes.len(), 1, "the single JSON route must compile");

    let eh = routes[0]
        .error_handler_config()
        .expect("error_handler must compile into a config");
    assert_eq!(eh.policies.len(), 1, "exactly one on_exceptions clause");

    let policy = &eh.policies[0];
    assert!(
        (policy.matches)(&CamelError::ValidationError("x".into())),
        "wildcard clause must match ValidationError"
    );
    assert!(
        (policy.matches)(&CamelError::ProcessorError("x".into())),
        "wildcard clause must match ProcessorError"
    );
    assert_eq!(
        policy.handled_by.as_deref(),
        Some("direct:shaper"),
        "retry.handled_by must surface on the compiled policy"
    );
}
