//! End-to-end tests for clause-level `handled_by` delegation.
//!
//! OpenSpec change `handledby`, Task 5.1 (e_opus sealed ruling, bd
//! rc-ntpof). Pins the eight ruling scenarios at runtime level:
//!
//! - (a) zero-retry delegation runs the step once, then delegates
//! - (b) retry composes: step runs 1 + max_attempts times, delegate runs
//!   once with the redelivery headers set
//! - (c)/(d) a failing delegate with `handled`/`continued` propagates the
//!   ORIGINAL error (never `Completed`, never the delegate's error kind)
//! - (e) a failing delegate at the security boundary propagates the
//!   ORIGINAL boundary error
//! - (f) `steps` + `handled_by` on one clause is a typed config rejection
//! - (g) legacy `retry: {handled_by}` layout is a hard load error (YAML
//!   and JSON)
//! - (h) `handled_by` without `handled`/`continued` is a tap: delegate
//!   sees the exchange, the original error still propagates

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::error_handler::{
    ErrorHandlerConfig, HEADER_REDELIVERED, HEADER_REDELIVERY_COUNTER,
    HEADER_REDELIVERY_MAX_COUNTER,
};
use camel_api::security_policy::{
    AccessMode, AuthContext, AuthorizationDecision, CredentialSource, RouteSecurityPlan,
    SecurityPolicy, SecurityPolicyConfig, TransportId,
};
use camel_api::{
    BoxProcessor, BoxProcessorExt, CamelError, ConfigValidationError, Exchange, Value,
};
use camel_auth::credential_source::ExtractedToken;
use camel_auth::{install_carrier, kernel_authenticate};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::test_support::acquire_deadline;
use camel_core::route::RouteDefinition;
use camel_dsl::json::parse_json;
use camel_dsl::parse_yaml;
use camel_test::{CamelTestContext, SecurityConfigFixture};
use tower::ServiceExt;

fn test_rt() -> Arc<dyn camel_component_api::RuntimeObservability> {
    Arc::new(camel_component_api::NoOpComponentContext)
}

/// Send an exchange to a `direct:` endpoint and return the route's reply.
///
/// `Ok` = pipeline `Completed` (e.g. the delegate's output became the final
/// result); `Err` = pipeline `Failed` carrying the propagated error.
async fn send_to_direct(
    h: &CamelTestContext,
    endpoint_uri: &str,
    exchange: Exchange,
) -> Result<Exchange, CamelError> {
    let producer = {
        let ctx = acquire_deadline(
            h.ctx(),
            "camel context (send_to_direct)",
            Duration::from_secs(10),
        )
        .await;
        let producer_ctx = ctx.producer_context();
        let registry = ctx.registry();
        let component = registry
            .get("direct")
            .expect("direct component not registered");
        let endpoint = component
            .create_endpoint(endpoint_uri, &*ctx)
            .expect("failed to create direct endpoint");
        endpoint
            .create_producer(test_rt(), &producer_ctx)
            .expect("failed to create direct producer")
    };
    producer.oneshot(exchange).await
}

/// A step that records each invocation, then fails with `Io`. The counter
/// proves how many times the step executed (original + redeliveries).
fn counting_failing_step(counter: Arc<AtomicU32>, msg: &'static str) -> BoxProcessor {
    BoxProcessor::from_fn(move |_ex: Exchange| {
        let counter = Arc::clone(&counter);
        Box::pin(async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Err(CamelError::Io(msg.into()))
        })
    })
}

/// A plain failing step with `Io` (no counter).
fn failing_step(msg: &'static str) -> BoxProcessor {
    BoxProcessor::from_fn(move |_ex: Exchange| {
        Box::pin(async move { Err(CamelError::Io(msg.into())) })
    })
}

/// Succeeding delegate: shapes the body, then records the (possibly
/// redelivered) exchange at `mock_sink` for count/header assertions.
fn shaper_route(from: &'static str, mock_sink: &'static str) -> RouteDefinition {
    RouteBuilder::from(from)
        .route_id(format!("delegate-{from}"))
        .set_body("shaped")
        .to(mock_sink)
        .build()
        .unwrap()
}

/// Failing delegate: records its invocation, then fails with
/// `ProcessorError` — a DIFFERENT kind than the original `Io`, so
/// asserting the `Io` variant on the outcome proves the ORIGINAL error
/// won. The counter proves the delegate was actually invoked (guards the
/// original-error assertions against vacuous passes).
fn failing_delegate_route(from: &'static str, counter: Arc<AtomicU32>) -> RouteDefinition {
    RouteBuilder::from(from)
        .route_id(format!("delegate-{from}"))
        .process_fn(BoxProcessor::from_fn(move |_ex: Exchange| {
            let counter = Arc::clone(&counter);
            Box::pin(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Err(CamelError::ProcessorError("delegate blew up".into()))
            })
        }))
        .build()
        .unwrap()
}

/// Wait for direct endpoints to register (consumer startup race, same as
/// do_try_test).
async fn settle() {
    tokio::time::sleep(Duration::from_millis(50)).await;
}

// ---------------------------------------------------------------------------
// Scenario (a): zero-retry delegation runs once, then delegates
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn zero_retry_delegation_runs_once_then_delegates() {
    let step_runs = Arc::new(AtomicU32::new(0));
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let shaper = shaper_route("direct:shaper-a", "mock:shaped-a");

    let main = RouteBuilder::from("direct:probe-a")
        .route_id("probe-a")
        .process_fn(counting_failing_step(Arc::clone(&step_runs), "boom"))
        .error_handler(
            ErrorHandlerConfig::log_only()
                .on_exception(|e| matches!(e, CamelError::Io(_)))
                .handled_by("direct:shaper-a")
                .handled(true)
                .build(),
        )
        .build()
        .unwrap();

    h.add_route(shaper).await.unwrap();
    h.add_route(main).await.unwrap();
    h.start().await;
    settle().await;

    let reply = send_to_direct(&h, "direct:probe-a", Exchange::default()).await;

    assert_eq!(
        step_runs.load(Ordering::SeqCst),
        1,
        "no retry block: the step must execute exactly once"
    );
    let ex = reply.expect("handled delegation must complete the pipeline");
    assert_eq!(
        ex.input.body.as_text(),
        Some("shaped"),
        "delegate output is the final result (Completed, error cleared)"
    );

    let sink = h.mock().get_endpoint("shaped-a").unwrap();
    sink.assert_exchange_count(1).await;
    let delegated = &sink.get_received_exchanges().await[0];
    assert!(
        delegated.input.header(HEADER_REDELIVERED).is_none(),
        "no redelivery occurred, so CamelRedelivered must be absent"
    );

    h.stop().await;
}

// ---------------------------------------------------------------------------
// Scenario (b): retry composes with handled_by
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn retry_composes_then_delegates_with_redelivery_headers() {
    let step_runs = Arc::new(AtomicU32::new(0));
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let shaper = shaper_route("direct:shaper-b", "mock:shaped-b");

    let main = RouteBuilder::from("direct:probe-b")
        .route_id("probe-b")
        .process_fn(counting_failing_step(Arc::clone(&step_runs), "boom"))
        .error_handler(
            ErrorHandlerConfig::log_only()
                .on_exception(|e| matches!(e, CamelError::Io(_)))
                // 1ms backoff is retry CONFIG, not a test sleep.
                .retry(2)
                .with_backoff(Duration::from_millis(1), 1.0, Duration::from_millis(1))
                .handled_by("direct:shaper-b")
                .handled(true)
                .build(),
        )
        .build()
        .unwrap();

    h.add_route(shaper).await.unwrap();
    h.add_route(main).await.unwrap();
    h.start().await;
    settle().await;

    let reply = send_to_direct(&h, "direct:probe-b", Exchange::default()).await;

    assert_eq!(
        step_runs.load(Ordering::SeqCst),
        3,
        "original execution + 2 redeliveries = 3 step executions"
    );
    let ex = reply.expect("handled delegation after retry exhaustion must complete the pipeline");
    assert_eq!(ex.input.body.as_text(), Some("shaped"));

    // Delegate invoked exactly once, after retries exhausted, carrying the
    // redelivery headers.
    let sink = h.mock().get_endpoint("shaped-b").unwrap();
    sink.assert_exchange_count(1).await;
    let delegated = &sink.get_received_exchanges().await[0];
    assert_eq!(
        delegated.input.header(HEADER_REDELIVERED),
        Some(&Value::Bool(true)),
        "delegated exchange must carry CamelRedelivered"
    );
    assert_eq!(
        delegated.input.header(HEADER_REDELIVERY_COUNTER),
        Some(&Value::Number(serde_json::Number::from(2))),
        "CamelRedeliveryCounter must equal the number of redeliveries"
    );
    assert_eq!(
        delegated.input.header(HEADER_REDELIVERY_MAX_COUNTER),
        Some(&Value::Number(serde_json::Number::from(2))),
        "CamelRedeliveryMaxCounter must mirror max_attempts"
    );

    h.stop().await;
}

// ---------------------------------------------------------------------------
// Scenario (c): failed delegate with handled:true fails with the original kind
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn failed_delegate_with_handled_fails_original() {
    let delegate_runs = Arc::new(AtomicU32::new(0));
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let broken = failing_delegate_route("direct:broken-shaper-c", Arc::clone(&delegate_runs));

    let main = RouteBuilder::from("direct:probe-c")
        .route_id("probe-c")
        .process_fn(failing_step("original failure"))
        .error_handler(
            ErrorHandlerConfig::log_only()
                .on_exception(|e| matches!(e, CamelError::Io(_)))
                .handled_by("direct:broken-shaper-c")
                .handled(true)
                .build(),
        )
        .build()
        .unwrap();

    h.add_route(broken).await.unwrap();
    h.add_route(main).await.unwrap();
    h.start().await;
    settle().await;

    let reply = send_to_direct(&h, "direct:probe-c", Exchange::default()).await;

    // Original Io wins: never Completed, never the delegate's ProcessorError.
    match reply {
        Err(CamelError::Io(_)) => {}
        other => panic!("expected Failed with the ORIGINAL Io error, got {other:?}"),
    }
    assert_eq!(
        delegate_runs.load(Ordering::SeqCst),
        1,
        "the delegate was invoked exactly once and failed"
    );

    h.stop().await;
}

// ---------------------------------------------------------------------------
// Scenario (d): failed delegate with continued:true fails with the original kind
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn failed_delegate_with_continued_fails_original() {
    let delegate_runs = Arc::new(AtomicU32::new(0));
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let broken = failing_delegate_route("direct:broken-shaper-d", Arc::clone(&delegate_runs));

    let main = RouteBuilder::from("direct:probe-d")
        .route_id("probe-d")
        .process_fn(failing_step("original failure"))
        .error_handler(
            ErrorHandlerConfig::log_only()
                .on_exception(|e| matches!(e, CamelError::Io(_)))
                .handled_by("direct:broken-shaper-d")
                .continued(true)
                .build(),
        )
        .build()
        .unwrap();

    h.add_route(broken).await.unwrap();
    h.add_route(main).await.unwrap();
    h.start().await;
    settle().await;

    let reply = send_to_direct(&h, "direct:probe-d", Exchange::default()).await;

    match reply {
        Err(CamelError::Io(_)) => {}
        other => panic!(
            "continued must not absorb a failed delegate: expected the ORIGINAL Io error, got {other:?}"
        ),
    }
    assert_eq!(
        delegate_runs.load(Ordering::SeqCst),
        1,
        "the delegate was invoked exactly once and failed"
    );

    h.stop().await;
}

// ---------------------------------------------------------------------------
// Scenario (e): failed delegate at the security boundary fails with the
// original boundary error
// ---------------------------------------------------------------------------

struct DenyAllPolicy;

#[async_trait]
impl SecurityPolicy for DenyAllPolicy {
    async fn evaluate(
        &self,
        _exchange: &mut Exchange,
        _auth: &AuthContext<'_>,
    ) -> Result<AuthorizationDecision, CamelError> {
        Ok(AuthorizationDecision::Denied {
            reason: "denied by fixture".into(),
            required: vec!["admin".into()],
            actual: vec![],
        })
    }
}

/// Mint an Exchange carrying the typed kernel carrier, exactly as a
/// transport boundary does (same shape as security_policy_test.rs).
const FIXTURE_PROVIDER: &str = "idp-handledby";

async fn carrier_exchange() -> Exchange {
    let fixture = SecurityConfigFixture::single_static_provider(FIXTURE_PROVIDER);
    let providers = fixture.providers();
    let plan = RouteSecurityPlan {
        access_mode: AccessMode::Authenticated,
        provider_ref: Some(FIXTURE_PROVIDER.to_string()),
        transport: TransportId::Http,
        credential_sources: vec![CredentialSource::AuthorizationHeader],
        audience_binding: None,
    };
    let credentials = ExtractedToken {
        token: format!("test-token-{FIXTURE_PROVIDER}"),
        source: CredentialSource::AuthorizationHeader,
    };
    let principal = kernel_authenticate(&plan, &providers, &credentials)
        .await
        .expect("fixture token must authenticate"); // allow-unwrap
    let mut exchange = Exchange::default();
    install_carrier(&mut exchange, &principal);
    exchange
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_delegate_at_boundary_fails_original() {
    let delegate_runs = Arc::new(AtomicU32::new(0));
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let broken = failing_delegate_route("direct:broken-audit-e", Arc::clone(&delegate_runs));

    let main = RouteBuilder::from("direct:probe-e")
        .route_id("probe-e")
        .to("mock:result-e")
        .error_handler(
            ErrorHandlerConfig::log_only()
                .on_exception(|e| matches!(e, CamelError::Unauthorized(_)))
                .handled_by("direct:broken-audit-e")
                .handled(true)
                .build(),
        )
        .build()
        .unwrap()
        .with_security_policy(SecurityPolicyConfig::new(DenyAllPolicy));

    h.add_route(broken).await.unwrap();
    h.add_route(main).await.unwrap();
    h.start().await;
    settle().await;

    // The security gate denies the (authorized but role-less) carrier with
    // `Unauthorized`; the boundary handler delegates to the failing route.
    let reply = send_to_direct(&h, "direct:probe-e", carrier_exchange().await).await;

    match reply {
        Err(CamelError::Unauthorized(_)) => {}
        other => {
            panic!("expected Failed with the ORIGINAL Unauthorized boundary error, got {other:?}")
        }
    }
    assert_eq!(
        delegate_runs.load(Ordering::SeqCst),
        1,
        "the boundary delegate was invoked exactly once and failed"
    );
    h.mock()
        .get_endpoint("result-e")
        .unwrap()
        .assert_exchange_count(0)
        .await;

    h.stop().await;
}

// ---------------------------------------------------------------------------
// Scenario (f): steps plus handled_by is a typed rejection
// ---------------------------------------------------------------------------

#[test]
fn steps_plus_handled_by_rejected_typed() {
    let yaml = r#"
routes:
  - id: "steps-conflict"
    from: "direct:steps-conflict"
    error_handler:
      on_exceptions:
        - kind: "Io"
          handled: true
          handled_by: "direct:shaper"
          steps:
            - to: "log:steps-conflict"
    steps:
      - to: "log:steps-conflict"
"#;

    match parse_yaml(yaml) {
        Err(CamelError::ConfigValidation(
            ConfigValidationError::OnExceptionStepsHandledByConflict,
        )) => {}
        Ok(routes) => panic!(
            "steps + handled_by must be a typed config rejection; compiled {} route(s)",
            routes.len()
        ),
        Err(other) => panic!("expected OnExceptionStepsHandledByConflict, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Scenario (g): legacy retry.handled_by layout is a hard load error
// ---------------------------------------------------------------------------

#[test]
fn legacy_retry_handled_by_hard_error_yaml_and_json() {
    let yaml = r#"
routes:
  - id: "legacy-yaml"
    from: "direct:legacy-yaml"
    error_handler:
      on_exceptions:
        - kind: "*"
          handled: true
          retry:
            max_attempts: 1
            handled_by: "direct:shaper"
    steps:
      - to: "log:legacy-yaml"
"#;
    let Err(yaml_err) = parse_yaml(yaml) else {
        panic!("legacy retry.handled_by layout must be a hard load error (YAML)");
    };
    let text = yaml_err.to_string();
    assert!(
        text.contains("unknown field"),
        "must reject handled_by as an unknown retry field: {text}"
    );
    assert!(text.contains("handled_by"), "must name the field: {text}");

    let json = r#"{
        "routes": [
            {
                "id": "legacy-json",
                "from": "direct:legacy-json",
                "error_handler": {
                    "on_exceptions": [
                        {
                            "kind": "*",
                            "handled": true,
                            "retry": {
                                "max_attempts": 1,
                                "handled_by": "direct:shaper"
                            }
                        }
                    ]
                },
                "steps": [
                    { "to": "log:legacy-json" }
                ]
            }
        ]
    }"#;
    let Err(json_err) = parse_json(json) else {
        panic!("legacy retry.handled_by layout must be a hard load error (JSON)");
    };
    let text = json_err.to_string();
    assert!(
        text.contains("unknown field"),
        "must reject handled_by as an unknown retry field: {text}"
    );
    assert!(text.contains("handled_by"), "must name the field: {text}");
}

// ---------------------------------------------------------------------------
// Scenario (h): handled_by without handled/continued is a tap
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn handled_by_without_handled_is_tap() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let audit = RouteBuilder::from("direct:audit-h")
        .route_id("audit-h")
        .to("mock:audit-h")
        .build()
        .unwrap();

    let main = RouteBuilder::from("direct:probe-h")
        .route_id("probe-h")
        .process_fn(failing_step("boom"))
        .error_handler(
            ErrorHandlerConfig::log_only()
                .on_exception(|e| matches!(e, CamelError::Io(_)))
                // No handled/continued: delegation is a tap, disposition
                // stays Propagate.
                .handled_by("direct:audit-h")
                .build(),
        )
        .build()
        .unwrap();

    h.add_route(audit).await.unwrap();
    h.add_route(main).await.unwrap();
    h.start().await;
    settle().await;

    let reply = send_to_direct(&h, "direct:probe-h", Exchange::default()).await;

    h.mock()
        .get_endpoint("audit-h")
        .unwrap()
        .assert_exchange_count(1)
        .await;
    match reply {
        Err(CamelError::Io(_)) => {}
        other => panic!("tap must propagate the ORIGINAL Io error, got {other:?}"),
    }

    h.stop().await;
}
