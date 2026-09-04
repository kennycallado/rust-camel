//! Scenario action runner tests (ADR-0069 sections 5 and 7).
//!
//! Unit-test module of the lib target, declared in `src/lib.rs` under
//! `#[cfg(test)]`. Every test wraps a [`FakeAdapter`] in a
//! single-entry [`PartnerRouter`] and drives [`run_scenario`] under a
//! tokio runtime; deadlines are real monotonic time and stay at
//! test-scale magnitudes.

use std::collections::BTreeMap;
use std::time::Duration;

use camel_api::Value;

use crate::adapters::{
    FakeAdapter, IncomingMessage, OutgoingMessage, PartnerAdapter, PartnerRouter, ReceiveError,
    TransportError,
};
use crate::document::{
    EndpointRef, Expectation, RouteSource, ScenarioAction, ScenarioDocument, ScenarioTarget,
};
use crate::runner::{
    DocumentOutcome, ScenarioFailure, ScenarioVars, ScenarioVerdict, run_scenario,
    run_scenario_document,
};

/// A bare endpoint reference with no provisioning and no bind variable.
fn endpoint(uri: &str) -> EndpointRef {
    EndpointRef {
        endpoint: uri.to_string(),
        provisioning: None,
        bind_var: None,
    }
}

/// A minimal document with the given actions and file-based routes.
fn doc_with(actions: Vec<ScenarioAction>) -> ScenarioDocument {
    ScenarioDocument {
        route_source: RouteSource::RouteFiles(vec!["routes.yaml".into()]),
        scenario: actions,
        env: None,
        env_passthrough: None,
        profile: None,
    }
}

/// A single-entry router over one fake adapter, keyed by endpoint URI.
fn router_for(uri: &str, fake: FakeAdapter) -> PartnerRouter {
    PartnerRouter::new(BTreeMap::from([(
        uri.to_string(),
        Box::new(fake) as Box<dyn PartnerAdapter>,
    )]))
}

/// An incoming message with a string body and no headers.
fn text_message(body: &str) -> IncomingMessage {
    IncomingMessage {
        body: Value::String(body.to_string()),
        headers: BTreeMap::new(),
        status: None,
        method: None,
        path: None,
    }
}

#[tokio::test]
async fn send_then_receive_within_deadline() {
    let fake = FakeAdapter::scripted(vec![text_message("hello")]);
    let router = router_for("partner://fake", fake);
    let doc = doc_with(vec![
        ScenarioAction::Send {
            to: endpoint("partner://fake"),
            body: Some(Value::String("hello".to_string())),
            headers: None,
            method: "POST".to_string(),
        },
        ScenarioAction::Receive {
            from: endpoint("partner://fake"),
            deadline: Duration::from_secs(1),
            extract: None,
        },
        ScenarioAction::Validate {
            target: ScenarioTarget::LastReceived(endpoint("partner://fake")),
            expectation: Expectation::Equals(Value::String("hello".to_string())),
        },
    ]);
    let mut vars = ScenarioVars::new();
    let verdict = run_scenario(&doc, &router, &mut vars).await;
    assert_eq!(verdict, Ok(ScenarioVerdict::Pass));
}

#[tokio::test]
async fn receive_timeout_is_verdict_failure() {
    let fake = FakeAdapter::scripted(Vec::new());
    let router = router_for("partner://fake", fake);
    let doc = doc_with(vec![ScenarioAction::Receive {
        from: endpoint("partner://fake"),
        deadline: Duration::from_millis(50),
        extract: None,
    }]);
    let mut vars = ScenarioVars::new();
    let failure = run_scenario(&doc, &router, &mut vars)
        .await
        .expect_err("empty queue must time out");
    assert!(
        matches!(failure, ScenarioFailure::ReceiveTimeout { .. }),
        "expected ReceiveTimeout, got {failure:?}"
    );
    assert!(
        failure.to_string().starts_with("receive-timeout"),
        "error must name the receive-timeout class: {failure}"
    );
}

#[tokio::test]
async fn variable_extraction_flows_forward() {
    fn scripted_with_id(id: &str) -> FakeAdapter {
        FakeAdapter::scripted(vec![IncomingMessage {
            body: Value::String("payload".to_string()),
            headers: BTreeMap::from([("X-Id".to_string(), Value::String(id.to_string()))]),
            status: None,
            method: None,
            path: None,
        }])
    }
    fn extraction_doc() -> ScenarioDocument {
        doc_with(vec![
            ScenarioAction::Receive {
                from: endpoint("partner://fake"),
                deadline: Duration::from_secs(1),
                extract: Some(BTreeMap::from([(
                    "id".to_string(),
                    "headers.X-Id".to_string(),
                )])),
            },
            ScenarioAction::Validate {
                target: ScenarioTarget::Variable("id".to_string()),
                expectation: Expectation::Equals(Value::String("abc-123".to_string())),
            },
        ])
    }

    // Matching header: extraction sets the variable, validation passes.
    let router = router_for("partner://fake", scripted_with_id("abc-123"));
    let mut vars = ScenarioVars::new();
    let verdict = run_scenario(&extraction_doc(), &router, &mut vars).await;
    assert_eq!(verdict, Ok(ScenarioVerdict::Pass));
    assert_eq!(
        vars.get("id"),
        Some(&Value::String("abc-123".to_string())),
        "extraction must persist the variable for later actions"
    );

    // Mismatched header: validation fails with the action index named.
    let router = router_for("partner://fake", scripted_with_id("nope"));
    let mut vars = ScenarioVars::new();
    let failure = run_scenario(&extraction_doc(), &router, &mut vars)
        .await
        .expect_err("mismatched header must fail validation");
    assert!(
        matches!(
            failure,
            ScenarioFailure::ValidationMismatch { action: 1, .. }
        ),
        "expected ValidationMismatch on action 1, got {failure:?}"
    );
}

#[tokio::test]
async fn transport_error_is_apparatus_failure() {
    let fake = FakeAdapter::failing_send("connection refused");
    let router = router_for("partner://fake", fake);
    let doc = doc_with(vec![ScenarioAction::Send {
        to: endpoint("partner://fake"),
        body: None,
        headers: None,
        method: "GET".to_string(),
    }]);
    let mut vars = ScenarioVars::new();
    let failure = run_scenario(&doc, &router, &mut vars)
        .await
        .expect_err("failing send must fail the scenario");
    assert!(
        matches!(failure, ScenarioFailure::ActionTransport { action: 0, .. }),
        "expected ActionTransport on action 0, got {failure:?}"
    );
    assert!(
        failure.to_string().starts_with("action-transport-failure"),
        "error must name the action-transport-failure class: {failure}"
    );
}

/// A receive that fails at the transport mid-scenario is apparatus
/// class (`action-transport-failure`), not a verdict-class timeout.
#[tokio::test]
async fn receive_transport_error_is_apparatus_failure() {
    let fake = FakeAdapter::failing_receive("connection reset");
    let router = router_for("partner://fake", fake);
    let doc = doc_with(vec![ScenarioAction::Receive {
        from: endpoint("partner://fake"),
        deadline: Duration::from_secs(1),
        extract: None,
    }]);
    let mut vars = ScenarioVars::new();
    let failure = run_scenario(&doc, &router, &mut vars)
        .await
        .expect_err("failing receive must fail the scenario");
    assert!(
        matches!(failure, ScenarioFailure::ActionTransport { action: 0, .. }),
        "expected ActionTransport on action 0, got {failure:?}"
    );
    assert!(
        failure.to_string().starts_with("action-transport-failure"),
        "error must name the action-transport-failure class: {failure}"
    );
}

// -------------------------------------------------------------------------
// Adapter-level contract checks (dispatch, recording, message shapes)
// -------------------------------------------------------------------------

/// The router dispatches by endpoint equality and records sends on the
/// owning fake.
#[tokio::test]
async fn router_dispatches_and_fake_records_sends() {
    let fake = FakeAdapter::scripted(Vec::new());
    let handle = fake.recorder();
    let router = router_for("partner://fake", fake);
    let sent = OutgoingMessage {
        body: Value::String("recorded".to_string()),
        headers: BTreeMap::from([("X-Trace".to_string(), Value::String("t1".to_string()))]),
        method: "POST".to_string(),
    };
    PartnerAdapter::send(&router, &endpoint("partner://fake"), sent)
        .await
        .expect("send must succeed");
    let recorded = handle.sent_messages();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].endpoint, "partner://fake");
    assert_eq!(
        recorded[0].message.body,
        Value::String("recorded".to_string())
    );

    // Unknown endpoint: the send fails at the transport, and the
    // receive fails at the transport too — no partner exists that
    // could ever deliver, so the failure is apparatus class, not a
    // verdict-class timeout, and the call never hangs.
    let err = PartnerAdapter::send(
        &router,
        &endpoint("partner://other"),
        OutgoingMessage {
            body: Value::Null,
            headers: BTreeMap::new(),
            method: "GET".to_string(),
        },
    )
    .await
    .expect_err("unbound endpoint must fail");
    assert!(matches!(err, TransportError::Unbound { .. }));
    let failure = PartnerAdapter::receive(
        &router,
        &endpoint("partner://other"),
        Duration::from_secs(30),
    )
    .await
    .expect_err("unbound endpoint must never deliver");
    assert!(
        matches!(
            failure,
            ReceiveError::Transport(TransportError::Unbound { .. })
        ),
        "expected Transport(Unbound), got {failure:?}"
    );
}

// -------------------------------------------------------------------------
// Selector grammar (status / method / path heads, case-insensitive
// header lookup — ADR-0069 section 5 partner-side validation)
// -------------------------------------------------------------------------

/// `extract` selectors reach the transport status, the request method,
/// and the request path a partner adapter reports.
#[tokio::test]
async fn selector_extracts_status_method_and_path() {
    let fake = FakeAdapter::scripted(vec![IncomingMessage {
        body: Value::String("payload".to_string()),
        headers: BTreeMap::new(),
        status: Some(201),
        method: Some("POST".to_string()),
        path: Some("/orders".to_string()),
    }]);
    let router = router_for("partner://fake", fake);
    let doc = doc_with(vec![
        ScenarioAction::Receive {
            from: endpoint("partner://fake"),
            deadline: Duration::from_secs(1),
            extract: Some(BTreeMap::from([
                ("status".to_string(), "status".to_string()),
                ("method".to_string(), "method".to_string()),
                ("path".to_string(), "path".to_string()),
            ])),
        },
        ScenarioAction::Validate {
            target: ScenarioTarget::Variable("status".to_string()),
            expectation: Expectation::Equals(Value::Number(201.into())),
        },
        ScenarioAction::Validate {
            target: ScenarioTarget::Variable("method".to_string()),
            expectation: Expectation::Equals(Value::String("POST".to_string())),
        },
        ScenarioAction::Validate {
            target: ScenarioTarget::Variable("path".to_string()),
            expectation: Expectation::Equals(Value::String("/orders".to_string())),
        },
    ]);
    let mut vars = ScenarioVars::new();
    let verdict = run_scenario(&doc, &router, &mut vars).await;
    assert_eq!(verdict, Ok(ScenarioVerdict::Pass));
}

/// Header lookup is ASCII-case-insensitive: the same selector behaves
/// identically whether the adapter preserved author casing (`X-Trace`)
/// or the wire normalized it to lowercase (`x-trace`), and vice versa.
#[tokio::test]
async fn selector_header_lookup_is_case_insensitive() {
    fn scripted_header(header_key: &str) -> FakeAdapter {
        FakeAdapter::scripted(vec![IncomingMessage {
            body: Value::Null,
            headers: BTreeMap::from([(header_key.to_string(), Value::String("t-42".to_string()))]),
            status: None,
            method: None,
            path: None,
        }])
    }
    fn doc(selector: &str) -> ScenarioDocument {
        doc_with(vec![
            ScenarioAction::Receive {
                from: endpoint("partner://fake"),
                deadline: Duration::from_secs(1),
                extract: Some(BTreeMap::from([(
                    "trace".to_string(),
                    selector.to_string(),
                )])),
            },
            ScenarioAction::Validate {
                target: ScenarioTarget::Variable("trace".to_string()),
                expectation: Expectation::Equals(Value::String("t-42".to_string())),
            },
        ])
    }

    // Author-cased header, author-cased selector (the FakeAdapter shape).
    let router = router_for("partner://fake", scripted_header("X-Trace"));
    let mut vars = ScenarioVars::new();
    let verdict = run_scenario(&doc("headers.X-Trace"), &router, &mut vars).await;
    assert_eq!(verdict, Ok(ScenarioVerdict::Pass));

    // Wire-lowercased header, author-cased selector (the hyper shape).
    let router = router_for("partner://fake", scripted_header("x-trace"));
    let mut vars = ScenarioVars::new();
    let verdict = run_scenario(&doc("headers.X-Trace"), &router, &mut vars).await;
    assert_eq!(verdict, Ok(ScenarioVerdict::Pass));

    // Author-cased header, lowercase selector.
    let router = router_for("partner://fake", scripted_header("X-Trace"));
    let mut vars = ScenarioVars::new();
    let verdict = run_scenario(&doc("headers.x-trace"), &router, &mut vars).await;
    assert_eq!(verdict, Ok(ScenarioVerdict::Pass));
}

// -------------------------------------------------------------------------
// Document-level execution (run_scenario_document)
// -------------------------------------------------------------------------

/// The document run records one outcome per executed action, passes
/// the verdict when every action passed, and leaves the
/// post-shutdown slot empty for the caller.
#[tokio::test]
async fn document_run_all_pass_records_verdict() {
    let fake = FakeAdapter::scripted(vec![text_message("one"), text_message("two")]);
    let router = router_for("partner://fake", fake);
    let doc = doc_with(vec![
        ScenarioAction::Receive {
            from: endpoint("partner://fake"),
            deadline: Duration::from_secs(1),
            extract: None,
        },
        ScenarioAction::Validate {
            target: ScenarioTarget::Variable("unset".to_string()),
            expectation: Expectation::Exists,
        },
    ]);
    // Seed the variable so the `Exists` validation passes.
    let mut vars = ScenarioVars::new();
    vars.set("unset", Value::String("set".to_string()));

    let outcome = run_scenario_document(&doc, &router, &mut vars).await;
    assert_eq!(
        outcome,
        DocumentOutcome {
            per_action: vec![Ok(ScenarioVerdict::Pass), Ok(ScenarioVerdict::Pass)],
            verdict: Some(ScenarioVerdict::Pass),
            final_failure: None,
        }
    );
}

/// The document run stops at the first failure: later actions never
/// execute, each executed action carries its own outcome, and no
/// verdict is recorded.
#[tokio::test]
async fn document_run_stops_at_first_failure() {
    let fake = FakeAdapter::scripted(vec![text_message("one")]);
    let router = router_for("partner://fake", fake);
    let doc = doc_with(vec![
        ScenarioAction::Receive {
            from: endpoint("partner://fake"),
            deadline: Duration::from_secs(1),
            extract: None,
        },
        ScenarioAction::Receive {
            from: endpoint("partner://fake"),
            deadline: Duration::from_millis(50),
            extract: None,
        },
        ScenarioAction::Receive {
            from: endpoint("partner://fake"),
            deadline: Duration::from_secs(1),
            extract: None,
        },
    ]);
    let mut vars = ScenarioVars::new();
    let outcome = run_scenario_document(&doc, &router, &mut vars).await;
    assert_eq!(outcome.per_action.len(), 2, "only two actions ran");
    assert_eq!(outcome.per_action[0], Ok(ScenarioVerdict::Pass));
    assert!(matches!(
        outcome.per_action[1],
        Err(ScenarioFailure::ReceiveTimeout { .. })
    ));
    assert_eq!(outcome.verdict, None, "no verdict after a failure");
    assert_eq!(outcome.final_failure, None);
    assert!(
        vars.last_received("partner://fake").is_some(),
        "executed actions' side effects must persist"
    );
}

/// A validation mismatch on a variable names the variable, so a
/// corrupted-header regression is diagnosable from the failure text.
#[tokio::test]
async fn variable_mismatch_names_the_variable() {
    let fake = FakeAdapter::scripted(vec![IncomingMessage {
        body: Value::Null,
        headers: BTreeMap::from([(
            "X-Order-Type".to_string(),
            Value::String("priority".to_string()),
        )]),
        status: None,
        method: None,
        path: None,
    }]);
    let router = router_for("partner://fake", fake);
    let doc = doc_with(vec![
        ScenarioAction::Receive {
            from: endpoint("partner://fake"),
            deadline: Duration::from_secs(1),
            extract: Some(BTreeMap::from([(
                "orderType".to_string(),
                "headers.X-Order-Type".to_string(),
            )])),
        },
        ScenarioAction::Validate {
            target: ScenarioTarget::Variable("orderType".to_string()),
            expectation: Expectation::Equals(Value::String("express".to_string())),
        },
    ]);
    let mut vars = ScenarioVars::new();
    let outcome = run_scenario_document(&doc, &router, &mut vars).await;
    assert_eq!(outcome.verdict, None);
    match &outcome.per_action[1] {
        Err(ScenarioFailure::ValidationMismatch { action: 1, detail }) => {
            assert!(
                detail.contains("orderType"),
                "mismatch must name the variable: {detail}"
            );
            assert!(
                detail.contains("express") && detail.contains("priority"),
                "mismatch must show expected and actual: {detail}"
            );
        }
        other => panic!("expected ValidationMismatch on action 1, got {other:?}"),
    }
}

/// Compile-time shape check: the trait is object-safe and the trait
/// object is Send + Sync, as the runner and the router map require.
#[test]
fn partner_adapter_trait_object_is_send_sync() {
    fn assert_send_sync<T: Send + Sync + ?Sized>() {}
    assert_send_sync::<dyn PartnerAdapter>();
    assert_send_sync::<Box<dyn PartnerAdapter>>();
}
