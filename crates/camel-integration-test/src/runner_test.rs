//! Scenario action runner tests (ADR-0069 sections 5 and 7).
//!
//! Unit-test module of the lib target, declared in `src/lib.rs` under
//! `#[cfg(test)]`. Every test wraps a [`FakeAdapter`] in a
//! single-entry [`PartnerRouter`] and drives [`run_scenario`] under a
//! tokio runtime; deadlines are real monotonic time and stay at
//! test-scale magnitudes.

use std::collections::BTreeMap;
use std::time::Duration;

use camel_api::{Body, Exchange, Message, Value};
use futures::future::BoxFuture;

#[cfg(feature = "http")]
use crate::adapters::ReceiveTimeout;
use crate::adapters::{
    ArrivalLaneOverflow, FakeAdapter, IncomingMessage, OutgoingMessage, PartnerAdapter,
    PartnerRouter, ReceiveError, TransportError,
};
use crate::document::{
    EndpointRef, Expectation, Provisioning, RouteSource, ScenarioAction, ScenarioDocument,
    ScenarioTarget, ValidateExpectation,
};
use crate::runner::{
    DocumentOutcome, ScenarioFailure, ScenarioVars, ScenarioVerdict, effective_send_deadline,
    fill_bind_vars, interpolate_value, reply_body_value, resolve_placeholders, run_scenario,
    run_scenario_document,
};

#[cfg(feature = "http")]
use crate::adapters::http::{HttpPartner, HttpWireRequest};
#[cfg(feature = "http")]
use crate::document::{CountBound, PartnerExpectation, PathFilter};
#[cfg(feature = "http")]
use crate::runner::{matching_requests, partner_mismatch_detail, render_bound, render_filters};

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
        source_path: std::path::PathBuf::new(),
        route_source: RouteSource::RouteFiles(vec!["routes.yaml".into()]),
        scenario: actions,
        partners: None,
        env: None,
        env_passthrough: None,
        profile: None,
        send_deadline: None,
        inbound: None,
        logs: None,
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
        arrival: std::time::Instant::now(),
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
            expect_reply: None,
        },
        ScenarioAction::Receive {
            from: endpoint("partner://fake"),
            deadline: Duration::from_secs(1),
            extract: None,
        },
        ScenarioAction::Validate {
            target: ScenarioTarget::LastReceived(endpoint("partner://fake")),
            expectation: ValidateExpectation::Message(Expectation::Equals(Value::String(
                "hello".to_string(),
            ))),
            deadline: None,
            elapsed_at_least: None,
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

/// The arrival-lane-overflow failure names its class, the endpoint,
/// and the dropped count (rc-7mli).
#[test]
fn arrival_lane_overflow_error_display() {
    let failure = ScenarioFailure::ArrivalLaneOverflow {
        endpoint: "http://127.0.0.1:9/orders".to_string(),
        dropped: 6,
    };
    let text = failure.to_string();
    assert!(
        text.contains("arrival-lane-overflow"),
        "error must name the arrival-lane-overflow class: {text}"
    );
    assert!(
        text.contains("http://127.0.0.1:9/orders"),
        "error must name the endpoint: {text}"
    );
    assert!(
        text.contains('6'),
        "error must name the dropped count: {text}"
    );
}

/// The adapter-level arrival-lane-overflow error renders the endpoint
/// and the dropped count, and makes no drain-window claim: the
/// dropped counter is cumulative across the lane's lifetime, so a
/// "while no receive drained the lane" clause would mislead once an
/// intervening receive ran (rc-qogy).
#[test]
fn adapter_lane_overflow_display_names_endpoint_and_count() {
    let error = ArrivalLaneOverflow {
        endpoint: "http://127.0.0.1:9/orders".to_string(),
        dropped: 6,
    };
    let text = error.to_string();
    assert!(
        text.contains("arrival lane overflow"),
        "error must name the overflow: {text}"
    );
    assert!(
        text.contains("http://127.0.0.1:9/orders"),
        "error must name the endpoint: {text}"
    );
    assert!(
        text.contains('6'),
        "error must name the dropped count: {text}"
    );
    assert!(
        !text.contains("while no receive drained"),
        "the cumulative dropped counter never resets, so the Display must not claim a drain window: {text}"
    );
}

/// The effective send bound: a document without `sendDeadline`
/// resolves to the thirty-second default; a document with
/// `sendDeadline` overrides it (rc-tr4w).
#[test]
fn effective_send_deadline_defaults_to_thirty_seconds() {
    let bare = doc_with(vec![]);
    assert_eq!(
        effective_send_deadline(&bare),
        Duration::from_secs(30),
        "a document without `sendDeadline` must keep the thirty-second default"
    );

    let mut bounded = doc_with(vec![]);
    bounded.send_deadline = Some(Duration::from_millis(500));
    assert_eq!(
        effective_send_deadline(&bounded),
        Duration::from_millis(500),
        "the document's `sendDeadline` must override the default"
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
            arrival: std::time::Instant::now(),
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
                expectation: ValidateExpectation::Message(Expectation::Equals(Value::String(
                    "abc-123".to_string(),
                ))),
                deadline: None,
                elapsed_at_least: None,
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
        expect_reply: None,
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
    router
        .send("partner://fake", "partner://fake", sent)
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
    let err = router
        .send(
            "partner://other",
            "partner://other",
            OutgoingMessage {
                body: Value::Null,
                headers: BTreeMap::new(),
                method: "GET".to_string(),
            },
        )
        .await
        .expect_err("unbound endpoint must fail");
    assert!(matches!(err, TransportError::Unbound { .. }));
    let failure = router
        .receive(
            "partner://other",
            "partner://other",
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
        arrival: std::time::Instant::now(),
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
            expectation: ValidateExpectation::Message(Expectation::Equals(Value::Number(
                201.into(),
            ))),
            deadline: None,
            elapsed_at_least: None,
        },
        ScenarioAction::Validate {
            target: ScenarioTarget::Variable("method".to_string()),
            expectation: ValidateExpectation::Message(Expectation::Equals(Value::String(
                "POST".to_string(),
            ))),
            deadline: None,
            elapsed_at_least: None,
        },
        ScenarioAction::Validate {
            target: ScenarioTarget::Variable("path".to_string()),
            expectation: ValidateExpectation::Message(Expectation::Equals(Value::String(
                "/orders".to_string(),
            ))),
            deadline: None,
            elapsed_at_least: None,
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
            arrival: std::time::Instant::now(),
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
                expectation: ValidateExpectation::Message(Expectation::Equals(Value::String(
                    "t-42".to_string(),
                ))),
                deadline: None,
                elapsed_at_least: None,
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
            expectation: ValidateExpectation::Message(Expectation::Exists),
            deadline: None,
            elapsed_at_least: None,
        },
    ]);
    // Seed the variable so the `Exists` validation passes.
    let mut vars = ScenarioVars::new();
    vars.set("unset", Value::String("set".to_string()));

    let outcome = run_scenario_document(&doc, &router, &mut vars, None).await;
    assert_eq!(
        outcome,
        DocumentOutcome {
            per_action: vec![Ok(ScenarioVerdict::Pass), Ok(ScenarioVerdict::Pass)],
            verdict: Some(ScenarioVerdict::Pass),
            final_failure: None,
            inbound_bound: None,
            logs_failure: None,
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
    let outcome = run_scenario_document(&doc, &router, &mut vars, None).await;
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
        arrival: std::time::Instant::now(),
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
            expectation: ValidateExpectation::Message(Expectation::Equals(Value::String(
                "express".to_string(),
            ))),
            deadline: None,
            elapsed_at_least: None,
        },
    ]);
    let mut vars = ScenarioVars::new();
    let outcome = run_scenario_document(&doc, &router, &mut vars, None).await;
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

/// The `expectReply` value reads the exchange's OUTPUT message first:
/// a route that populates a real InOut reply body wins over the
/// (route-mutated — `set_body` writes it) input message, which stays
/// the fallback when no output exists. The e2e direct-reply tests
/// only exercise the input-fallback arm, so this pins the output arm.
#[test]
fn reply_body_value_reads_output_before_input() {
    let mut exchange = Exchange::new(Message::new(Body::Text("mutated-input".to_string())));
    assert_eq!(
        reply_body_value(&exchange),
        Value::String("mutated-input".to_string()),
        "without an output message the route-mutated input body is the reply"
    );
    exchange.output = Some(Message::new(Body::Text("out-reply".to_string())));
    assert_eq!(
        reply_body_value(&exchange),
        Value::String("out-reply".to_string()),
        "an output message's body wins over the input body"
    );
}

// -------------------------------------------------------------------------
// Placeholder resolution (${name} in scenario strings, ADR-0069 §5)
// -------------------------------------------------------------------------

/// A known variable substitutes its string value into the placeholder.
#[test]
fn resolve_substitutes_known_var() {
    let mut vars = ScenarioVars::new();
    vars.set("PARTNER", Value::String("127.0.0.1:9".to_string()));
    assert_eq!(
        resolve_placeholders("http://${PARTNER}/orders", &vars),
        Ok("http://127.0.0.1:9/orders".to_string())
    );
}

/// `$${` escapes to a literal `${`; the rest is scanned literally and
/// no lookup happens.
#[test]
fn resolve_escape_yields_literal() {
    let vars = ScenarioVars::new();
    assert_eq!(
        resolve_placeholders("$${not_a_var}", &vars),
        Ok("${not_a_var}".to_string())
    );
}

/// An unset variable fails with the variable's name named.
#[test]
fn resolve_unset_var_names_it() {
    let vars = ScenarioVars::new();
    assert_eq!(
        resolve_placeholders("${missing}", &vars),
        Err(ScenarioFailure::VarUnresolved {
            name: "missing".to_string()
        })
    );
}

/// A non-string variable substitutes its JSON representation.
#[test]
fn resolve_non_string_stringifies() {
    let mut vars = ScenarioVars::new();
    vars.set("N", Value::Number(42.into()));
    assert_eq!(resolve_placeholders("${N}", &vars), Ok("42".to_string()));
}

/// A name that does not match `[A-Za-z0-9_]+` stays literal.
#[test]
fn resolve_invalid_name_stays_literal() {
    let vars = ScenarioVars::new();
    assert_eq!(
        resolve_placeholders("${a-b}", &vars),
        Ok("${a-b}".to_string())
    );
}

/// An env-style placeholder (a colon after the name) stays literal:
/// `${env:}` never resolves in scenarios.
#[test]
fn resolve_env_placeholder_stays_literal() {
    let vars = ScenarioVars::new();
    assert_eq!(
        resolve_placeholders("${env:FOO}", &vars),
        Ok("${env:FOO}".to_string())
    );
}

/// Interpolation rebuilds maps and arrays recursively, substituting
/// string leaves and leaving other leaves untouched.
#[test]
fn interpolate_walks_nested_leaves() {
    let mut vars = ScenarioVars::new();
    vars.set("x", Value::String("1".to_string()));
    vars.set("y", Value::String("2".to_string()));
    let body = Value::Object(
        [
            (
                "a".to_string(),
                Value::Array(vec![
                    Value::String("${x}".to_string()),
                    Value::Number(1.into()),
                ]),
            ),
            (
                "b".to_string(),
                Value::Object(
                    [("c".to_string(), Value::String("${y}".to_string()))]
                        .into_iter()
                        .collect(),
                ),
            ),
        ]
        .into_iter()
        .collect(),
    );
    let expected = Value::Object(
        [
            (
                "a".to_string(),
                Value::Array(vec![
                    Value::String("1".to_string()),
                    Value::Number(1.into()),
                ]),
            ),
            (
                "b".to_string(),
                Value::Object(
                    [("c".to_string(), Value::String("2".to_string()))]
                        .into_iter()
                        .collect(),
                ),
            ),
        ]
        .into_iter()
        .collect(),
    );
    assert_eq!(interpolate_value(&body, &vars), Ok(expected));
}

/// An unset variable inside a nested body propagates the failure.
#[test]
fn interpolate_unset_in_body_propagates() {
    let vars = ScenarioVars::new();
    let body = Value::Object(
        [(
            "a".to_string(),
            Value::Object(
                [("b".to_string(), Value::String("${missing}".to_string()))]
                    .into_iter()
                    .collect(),
            ),
        )]
        .into_iter()
        .collect(),
    );
    assert_eq!(
        interpolate_value(&body, &vars),
        Err(ScenarioFailure::VarUnresolved {
            name: "missing".to_string()
        })
    );
}

// -------------------------------------------------------------------------
// Interpolation and bind vars in the action path (ADR-0069 §5, §9)
// -------------------------------------------------------------------------

/// An adapter standing in for one that owns a listener: it reports a
/// fixed bound authority and nothing else.
struct StaticAuthority(&'static str);

impl PartnerAdapter for StaticAuthority {
    fn receive<'a>(
        &'a self,
        _lane_key: &'a str,
        source_uri: &'a str,
        _deadline: Duration,
    ) -> BoxFuture<'a, Result<IncomingMessage, ReceiveError>> {
        Box::pin(async move {
            Err(ReceiveError::Transport(TransportError::Other {
                message: format!("{source_uri} has no receive role in this test"),
            }))
        })
    }

    fn bound_authority(&self) -> Option<String> {
        Some(self.0.to_string())
    }
}

/// A send to a dynamic `http://${PARTNER}/...` reference dials the
/// partner's bound authority: the interpolated URI resolves to the
/// registered partner by authority, and the partner listener records
/// the request path.
#[tokio::test]
#[cfg(feature = "http")]
async fn send_interpolates_endpoint() {
    let partner = HttpPartner::start_permissive(200)
        .await
        .expect("partner must bind 127.0.0.1:0");
    let recorder = partner.recorder();
    let authority = partner.bound_addr().to_string();
    let router = PartnerRouter::new(BTreeMap::from([(
        "http://127.0.0.1:0/orders".to_string(),
        Box::new(partner) as Box<dyn PartnerAdapter>,
    )]));
    let doc = doc_with(vec![
        ScenarioAction::Send {
            to: endpoint("http://${PARTNER}/orders"),
            body: None,
            headers: None,
            method: "POST".to_string(),
            expect_reply: None,
        },
        // The client lane dials in a spawned task (task 1.5 makes the
        // send await the connect); the receive takes the parked
        // roundtrip and synchronizes the server-side recording, the
        // same pattern the http partner tests use.
        ScenarioAction::Receive {
            from: endpoint("http://${PARTNER}/orders"),
            deadline: Duration::from_secs(5),
            extract: None,
        },
    ]);
    let mut vars = ScenarioVars::new();
    vars.set("PARTNER", Value::String(authority));
    run_scenario(&doc, &router, &mut vars)
        .await
        .expect("the interpolated send must reach the partner");
    let recorded = recorder.recorded_requests();
    assert_eq!(recorded.len(), 1, "exactly one request must reach the wire");
    assert_eq!(recorded[0].path, "/orders");
}

/// A send interpolates its body's string leaves and its header
/// values: the recorded wire request carries the substituted bytes.
#[tokio::test]
#[cfg(feature = "http")]
async fn send_interpolates_body_and_headers() {
    let partner = HttpPartner::start_permissive(200)
        .await
        .expect("partner must bind 127.0.0.1:0");
    let recorder = partner.recorder();
    let uri = format!("http://{}/orders", partner.bound_addr());
    let router = PartnerRouter::new(BTreeMap::from([(
        uri.clone(),
        Box::new(partner) as Box<dyn PartnerAdapter>,
    )]));
    let doc = doc_with(vec![
        ScenarioAction::Send {
            to: endpoint(&uri),
            body: Some(Value::Object(
                [("sku".to_string(), Value::String("${SKU}".to_string()))]
                    .into_iter()
                    .collect(),
            )),
            headers: Some(BTreeMap::from([(
                "X-Trace".to_string(),
                Value::String("${SKU}".to_string()),
            )])),
            method: "POST".to_string(),
            expect_reply: None,
        },
        // Synchronizes the spawned client-lane exchange and the
        // server-side recording.
        ScenarioAction::Receive {
            from: endpoint(&uri),
            deadline: Duration::from_secs(5),
            extract: None,
        },
    ]);
    let mut vars = ScenarioVars::new();
    vars.set("SKU", Value::String("x1".to_string()));
    run_scenario(&doc, &router, &mut vars)
        .await
        .expect("the send must reach the partner");
    let recorded = recorder.recorded_requests();
    assert_eq!(recorded.len(), 1);
    assert!(
        String::from_utf8_lossy(&recorded[0].body).contains("x1"),
        "recorded body must carry the substituted SKU: {:?}",
        recorded[0].body,
    );
    assert_eq!(
        recorded[0].headers.get("x-trace").map(String::as_str),
        Some("x1"),
        "recorded header must carry the substituted value"
    );
}

/// `fill_bind_vars` writes the partner's bound authority —
/// `host:port`, no scheme — into the scenario variable named by the
/// wired reference's `bindVar`.
#[test]
fn fill_bind_vars_sets_authority_without_scheme() {
    let uri = "http://127.0.0.1:0/orders";
    let router = PartnerRouter::new(BTreeMap::from([(
        uri.to_string(),
        Box::new(StaticAuthority("127.0.0.1:45678")) as Box<dyn PartnerAdapter>,
    )]));
    let wired = vec![EndpointRef {
        endpoint: uri.to_string(),
        provisioning: Some(Provisioning::Harness),
        bind_var: Some("PARTNER".to_string()),
    }];
    let mut vars = ScenarioVars::new();
    fill_bind_vars(&wired, &router, &mut vars);
    assert_eq!(
        vars.get("PARTNER"),
        Some(&Value::String("127.0.0.1:45678".to_string())),
        "the bind variable must carry the bare host:port authority"
    );
}

// -------------------------------------------------------------------------
// Partner validation (recorded-request counts, ADR-0069 §5)
// -------------------------------------------------------------------------

/// The declared harness endpoint every partner validate here reads.
#[cfg(feature = "http")]
const ORDERS: &str = "http://127.0.0.1:0/orders";

/// A single-entry router with `partner` registered under the declared
/// `:0` orders endpoint.
#[cfg(feature = "http")]
fn orders_router(partner: HttpPartner) -> PartnerRouter {
    PartnerRouter::new(BTreeMap::from([(
        ORDERS.to_string(),
        Box::new(partner) as Box<dyn PartnerAdapter>,
    )]))
}

/// A POST send to the declared `:0` orders endpoint.
#[cfg(feature = "http")]
fn orders_send() -> ScenarioAction {
    ScenarioAction::Send {
        to: endpoint(ORDERS),
        body: None,
        headers: None,
        method: "POST".to_string(),
        expect_reply: None,
    }
}

/// A partner validate on the declared orders endpoint: the count
/// expectation with optional method/path filters and an optional poll
/// deadline.
#[cfg(feature = "http")]
fn partner_validate(
    count: u64,
    method: Option<&str>,
    path: Option<&str>,
    deadline: Option<Duration>,
) -> ScenarioAction {
    ScenarioAction::Validate {
        target: ScenarioTarget::Partner(endpoint(ORDERS)),
        expectation: ValidateExpectation::Partner(PartnerExpectation {
            bound: CountBound::Exact(count),
            method: method.map(str::to_string),
            path: path.map(|path| PathFilter::Exact(path.to_string())),
            query: None,
        }),
        deadline,
        elapsed_at_least: None,
    }
}

/// One raw HTTP/1.1 exchange straight to the partner's bound address —
/// the foreign-client arrival path no router lane owns.
/// `connection: close` makes it one write and one drained read, and
/// the partner records the request before it answers, so a completed
/// call means a recorded arrival.
#[cfg(feature = "http")]
async fn raw_request(authority: &str, method: &str, path: &str) {
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    let mut stream = tokio::net::TcpStream::connect(authority)
        .await
        .expect("the partner's bound address must accept");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nhost: {authority}\r\nconnection: close\r\ncontent-length: 0\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("the raw request must leave");
    let mut sink = Vec::new();
    stream
        .read_to_end(&mut sink)
        .await
        .expect("the partner must close after its response");
}

/// The first failure of an outcome that must have failed.
#[cfg(feature = "http")]
fn first_failure(outcome: &DocumentOutcome) -> &ScenarioFailure {
    outcome
        .per_action
        .iter()
        .find_map(|result| result.as_ref().err())
        .expect("the document must have failed")
}

/// A wire HTTP request with no headers and no body.
#[cfg(feature = "http")]
fn wire(method: &str, path: &str) -> HttpWireRequest {
    HttpWireRequest {
        method: method.to_string(),
        path: path.to_string(),
        headers: BTreeMap::new(),
        body: Vec::new(),
    }
}

/// A GET wire request with no headers and no body.
#[cfg(feature = "http")]
fn wire_get(path: &str) -> HttpWireRequest {
    wire("GET", path)
}

/// Filter semantics of `matching_requests`: the method filter folds
/// ASCII case, the path filter is the exact path-and-query, and `None`
/// filters pass everything.
#[test]
#[cfg(feature = "http")]
fn matching_requests_filters_method_case_insensitive_and_exact_path() {
    let requests = vec![
        wire("POST", "/orders"),
        wire("GET", "/orders"),
        wire("GET", "/orders?page=2"),
        wire("GET", "/health"),
        wire("delete", "/orders"),
    ];
    // `None` filters pass every request.
    assert_eq!(matching_requests(&requests, None, None, None), 5);
    // The method filter folds ASCII case in both directions.
    assert_eq!(matching_requests(&requests, Some("get"), None, None), 3);
    assert_eq!(matching_requests(&requests, Some("DELETE"), None, None), 1);
    // The Exact path filter is the exact path-and-query: no prefix and
    // no query-blind matching.
    assert_eq!(
        matching_requests(
            &requests,
            None,
            Some(&PathFilter::Exact("/orders".to_string())),
            None
        ),
        3
    );
    assert_eq!(
        matching_requests(
            &requests,
            None,
            Some(&PathFilter::Exact("/orders?page=2".to_string())),
            None
        ),
        1
    );
    // All filters combine conjunctively.
    assert_eq!(
        matching_requests(
            &requests,
            Some("get"),
            Some(&PathFilter::Exact("/orders".to_string())),
            None
        ),
        1
    );
}

/// The Exact path filter is byte-strict on the path-and-query: the
/// percent-encoded comma never equals the decoded comma, so only the
/// request carrying the identical bytes counts. Encoding leniency
/// belongs to Contains/Matches and the decoded query subset, never to
/// the Exact comparison.
#[test]
#[cfg(feature = "http")]
fn matching_exact_path_is_byte_strict() {
    let requests = vec![wire_get("/q?bbox=1.5%2C2.5"), wire_get("/q?bbox=1.5,2.5")];
    assert_eq!(
        matching_requests(
            &requests,
            None,
            Some(&PathFilter::Exact("/q?bbox=1.5%2C2.5".to_string())),
            None
        ),
        1
    );
}

/// The Contains path filter tolerates encoding differences: the
/// substring `bbox=` appears in both the percent-encoded and the raw
/// comma form of the request path.
#[test]
#[cfg(feature = "http")]
fn matching_contains_tolerates_encoding() {
    let requests = vec![wire_get("/q?bbox=1.5%2C2.5"), wire_get("/q?bbox=1.5,2.5")];
    assert_eq!(
        matching_requests(
            &requests,
            None,
            Some(&PathFilter::Contains("bbox=".to_string())),
            None
        ),
        2
    );
}

/// The Matches path filter narrows by regex over the recorded
/// path-and-query: only the request the pattern accepts counts.
#[test]
#[cfg(feature = "http")]
fn matching_regex_narrows() {
    let requests = vec![wire_get("/orders/42"), wire_get("/health")];
    assert_eq!(
        matching_requests(
            &requests,
            None,
            Some(&PathFilter::Matches("^/orders/\\d+$".to_string())),
            None
        ),
        1
    );
}

/// The query subset filter decodes the request's query (percent and
/// `+` forms) and compares pair-wise: every declared pair must appear
/// among the decoded pairs, in any position order.
#[test]
#[cfg(feature = "http")]
fn matching_query_subset_decodes_and_ignores_order() {
    let requests = vec![wire_get("/q?b=2&a=1%2B1")];
    let query = BTreeMap::from([
        ("a".to_string(), "1+1".to_string()),
        ("b".to_string(), "2".to_string()),
    ]);
    assert_eq!(matching_requests(&requests, None, None, Some(&query)), 1);
}

/// The query subset filter is a subset, not an equality: a declared
/// pair absent from the request's query excludes the request.
#[test]
#[cfg(feature = "http")]
fn matching_query_subset_absent_pair_excludes() {
    let requests = vec![wire_get("/q?a=1")];
    let query = BTreeMap::from([
        ("a".to_string(), "1".to_string()),
        ("c".to_string(), "3".to_string()),
    ]);
    assert_eq!(matching_requests(&requests, None, None, Some(&query)), 0);
}

/// The method and query subset filters combine conjunctively: the
/// declared lowercase method folds ASCII case onto the uppercased
/// wire records, and only the one request that passes both counts.
#[test]
#[cfg(feature = "http")]
fn matching_method_composes_with_query() {
    let requests = vec![wire("POST", "/q?a=1"), wire("GET", "/q?a=1")];
    let query = BTreeMap::from([("a".to_string(), "1".to_string())]);
    assert_eq!(
        matching_requests(&requests, Some("post"), None, Some(&query)),
        1
    );
}

/// The immediate snapshot: exact equality passes without a deadline,
/// and a mismatch names the partner URI and both counts. The receive
/// synchronizes the send's spawned client-lane exchange, so the
/// validates read a settled recorder.
#[tokio::test]
#[cfg(feature = "http")]
async fn immediate_count_passes_and_mismatch_names_counts() {
    let partner = HttpPartner::start_permissive(200)
        .await
        .expect("partner must bind 127.0.0.1:0");
    let router = orders_router(partner);
    let doc = doc_with(vec![
        orders_send(),
        ScenarioAction::Receive {
            from: endpoint(ORDERS),
            deadline: Duration::from_secs(5),
            extract: None,
        },
        partner_validate(1, None, None, None),
        partner_validate(2, None, None, None),
    ]);
    let mut vars = ScenarioVars::new();
    let outcome = run_scenario_document(&doc, &router, &mut vars, None).await;

    assert!(
        matches!(outcome.per_action[2], Ok(ScenarioVerdict::Pass)),
        "the exact immediate count must pass: {outcome:?}"
    );
    assert_eq!(outcome.verdict, None, "the count: 2 validate must fail");
    let ScenarioFailure::ValidationMismatch { detail, .. } = first_failure(&outcome) else {
        panic!(
            "expected ValidationMismatch, got {:?}",
            first_failure(&outcome)
        );
    };
    assert!(
        detail.contains("partner http://127.0.0.1:0/orders"),
        "the mismatch must name the partner URI: {detail}"
    );
    assert!(
        detail.contains("expected 2, actual 1"),
        "the mismatch must name both counts: {detail}"
    );
}

/// A filtered count mismatch names the applied filter clauses: the one
/// recorded POST to `/orders` matches both filters, so an expectation
/// of 2 fails with `method post, path /orders` spelled out in the
/// detail.
#[tokio::test]
#[cfg(feature = "http")]
async fn filtered_mismatch_names_method_and_path_clauses() {
    let partner = HttpPartner::start_permissive(200)
        .await
        .expect("partner must bind 127.0.0.1:0");
    let router = orders_router(partner);
    let doc = doc_with(vec![
        orders_send(),
        ScenarioAction::Receive {
            from: endpoint(ORDERS),
            deadline: Duration::from_secs(5),
            extract: None,
        },
        partner_validate(2, Some("post"), Some("/orders"), None),
    ]);
    let mut vars = ScenarioVars::new();
    let outcome = run_scenario_document(&doc, &router, &mut vars, None).await;

    assert_eq!(outcome.verdict, None, "the filtered count must fail");
    let ScenarioFailure::ValidationMismatch { detail, .. } = first_failure(&outcome) else {
        panic!(
            "expected ValidationMismatch, got {:?}",
            first_failure(&outcome)
        );
    };
    assert!(
        detail.contains("method post"),
        "the mismatch must name the method filter: {detail}"
    );
    assert!(
        detail.contains("path /orders"),
        "the mismatch must name the path filter: {detail}"
    );
    assert!(
        detail.contains("expected 2, actual 1"),
        "the mismatch must name both counts: {detail}"
    );
}

/// The polled snapshot settles: one arrival lands before the run, two
/// more at 300 ms while the validate polls, and the count reaches its
/// expectation long before the 5 s deadline — the pass comes from a
/// poll seeing the settle, not from waiting the deadline out.
#[tokio::test]
#[cfg(feature = "http")]
async fn poll_passes_once_count_settles() {
    let partner = HttpPartner::start_permissive(200)
        .await
        .expect("partner must bind 127.0.0.1:0");
    let authority = partner.bound_addr().to_string();
    // One arrival before the run: every early snapshot reads 1, below
    // the expectation, so the validate must keep polling.
    raw_request(&authority, "POST", "/orders").await;
    let router = orders_router(partner);
    let settling = tokio::spawn({
        let authority = authority.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            raw_request(&authority, "POST", "/orders").await;
            raw_request(&authority, "POST", "/orders").await;
        }
    });
    let doc = doc_with(vec![partner_validate(
        3,
        None,
        None,
        Some(Duration::from_secs(5)),
    )]);
    let mut vars = ScenarioVars::new();
    let outcome = run_scenario_document(&doc, &router, &mut vars, None).await;
    settling.await.expect("the settling task must finish");

    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "the polled count must settle to 3: {outcome:?}"
    );
}

/// A count above the expectation never passes: arrivals only add, so
/// every polled snapshot and the final one read 4 against an
/// expectation of 3.
#[tokio::test]
#[cfg(feature = "http")]
async fn overshoot_never_passes() {
    let partner = HttpPartner::start_permissive(200)
        .await
        .expect("partner must bind 127.0.0.1:0");
    let authority = partner.bound_addr().to_string();
    for _ in 0..4 {
        raw_request(&authority, "POST", "/orders").await;
    }
    let router = orders_router(partner);
    let doc = doc_with(vec![partner_validate(
        3,
        None,
        None,
        Some(Duration::from_secs(1)),
    )]);
    let mut vars = ScenarioVars::new();
    let outcome = run_scenario_document(&doc, &router, &mut vars, None).await;

    assert_eq!(
        outcome.verdict, None,
        "a count above the expectation must never pass: {outcome:?}"
    );
    let ScenarioFailure::ValidationMismatch { detail, .. } = first_failure(&outcome) else {
        panic!(
            "expected ValidationMismatch, got {:?}",
            first_failure(&outcome)
        );
    };
    assert!(
        detail.contains("expected 3, actual 4"),
        "the mismatch must name the final counts: {detail}"
    );
}

/// Deadline expiry reports the final snapshot's count as the actual:
/// one arrival, an expectation of 3, and after the 1 s deadline the
/// failure names actual 1.
#[tokio::test]
#[cfg(feature = "http")]
async fn deadline_expiry_reports_final_actual() {
    let partner = HttpPartner::start_permissive(200)
        .await
        .expect("partner must bind 127.0.0.1:0");
    let authority = partner.bound_addr().to_string();
    raw_request(&authority, "POST", "/orders").await;
    let router = orders_router(partner);
    let doc = doc_with(vec![partner_validate(
        3,
        None,
        None,
        Some(Duration::from_secs(1)),
    )]);
    let mut vars = ScenarioVars::new();
    let outcome = run_scenario_document(&doc, &router, &mut vars, None).await;

    assert_eq!(outcome.verdict, None, "the count must never reach 3");
    let ScenarioFailure::ValidationMismatch { detail, .. } = first_failure(&outcome) else {
        panic!(
            "expected ValidationMismatch, got {:?}",
            first_failure(&outcome)
        );
    };
    assert!(
        detail.contains("actual 1"),
        "the mismatch must report the final snapshot's count: {detail}"
    );
}

/// The partner count mismatch detail lists the recorded request
/// paths, not only the counts, so a failed assertion is diagnosable
/// from the failure text alone (spec: integration-tier, count
/// mismatch lists recorded paths).
#[test]
#[cfg(feature = "http")]
fn partner_mismatch_detail_lists_recorded_paths() {
    let expected = PartnerExpectation {
        bound: CountBound::Exact(2),
        method: None,
        path: None,
        query: None,
    };
    let detail = partner_mismatch_detail(
        "http://127.0.0.1:0/a",
        &expected,
        1,
        &["/a?b=1".to_string(), "/c".to_string()],
        &[],
    );
    assert!(
        detail.contains("expected 2, actual 1"),
        "the mismatch must name both counts: {detail}"
    );
    assert!(
        detail.contains("/a?b=1"),
        "must list the first path: {detail}"
    );
    assert!(detail.contains("/c"), "must list the second path: {detail}");
}

/// Secret-marked query keys redact in the partner count mismatch
/// detail (ADR-0051 positive secret rule): the partner URI header,
/// the `path` filter echo, and every recorded path mask the secret
/// value, a non-secret pair stays visible, and the secret value never
/// prints.
#[test]
#[cfg(feature = "http")]
fn count_mismatch_redacts_secrets() {
    let expected = PartnerExpectation {
        bound: CountBound::Exact(2),
        method: None,
        path: Some(PathFilter::Exact(
            "/login?authPassword=hunter2&x=1".to_string(),
        )),
        query: None,
    };
    let detail = partner_mismatch_detail(
        "http://127.0.0.1:0/login?authPassword=hunter2&x=1",
        &expected,
        1,
        &["/login?authPassword=hunter2&x=1".to_string()],
        &["authPassword".to_string()],
    );
    assert!(
        detail.contains("authPassword=***"),
        "the secret value must be masked: {detail}"
    );
    assert!(
        !detail.contains("hunter2"),
        "the secret must never print: {detail}"
    );
    assert!(
        detail.contains("x=1"),
        "non-secret pairs must stay visible: {detail}"
    );
    assert!(
        detail.contains("partner http://127.0.0.1:0/login?authPassword=***&x=1"),
        "the partner URI header must mask the secret too: {detail}"
    );
    assert!(
        detail.contains("path /login?authPassword=***"),
        "the path filter echo must mask the secret too: {detail}"
    );
}

/// The bound grammar of the mismatch detail: each bound kind renders
/// in its own words, and `Exact` keeps the historical `expected N`
/// phrasing the exact-count mismatch tests pin byte-for-byte.
#[test]
#[cfg(feature = "http")]
fn render_bound_grammar() {
    assert_eq!(render_bound(&CountBound::Exact(3)), "expected 3");
    assert_eq!(render_bound(&CountBound::AtLeast(3)), "expected at least 3");
    assert_eq!(render_bound(&CountBound::AtMost(2)), "expected at most 2");
    assert_eq!(
        render_bound(&CountBound::Range(2, 4)),
        "expected between 2 and 4"
    );
}

/// Filter rendering redacts secret query pairs and elides pattern
/// payloads (ADR-0051 extended to filter payloads): the declared
/// secret pair masks its value, the non-secret pair stays visible,
/// and a `pathContains` pattern renders by kind only — neither the
/// secret value nor the pattern bytes print.
#[test]
#[cfg(feature = "http")]
fn render_filters_redacts_secret_query_and_elides_patterns() {
    let expected = PartnerExpectation {
        bound: CountBound::AtLeast(1),
        method: Some("GET".to_string()),
        path: Some(PathFilter::Contains("secret".to_string())),
        query: Some(BTreeMap::from([
            ("bbox".to_string(), "1,2".to_string()),
            ("token".to_string(), "abc".to_string()),
        ])),
    };
    let rendered = render_filters(&expected, &["token".to_string()]);
    assert!(
        rendered.contains("token=<redacted>"),
        "the secret pair must mask its value: {rendered}"
    );
    assert!(
        rendered.contains("bbox=1,2"),
        "the non-secret pair must stay visible: {rendered}"
    );
    assert!(
        rendered.contains("method GET"),
        "the method clause must render: {rendered}"
    );
    assert!(
        rendered.contains("pathContains <pattern elided>"),
        "the pattern must render by kind only: {rendered}"
    );
    assert!(
        !rendered.contains("abc"),
        "the secret value must never print: {rendered}"
    );
    assert!(
        !rendered.contains("secret"),
        "the pattern payload must never print: {rendered}"
    );
}

/// An adapter whose receive times out with a canned endpoint and
/// lane evidence, handed over exactly as the adapter rendered them —
/// pre-redacted at the harness construction sites, or RAW from a
/// third-party adapter (ADR-0051).
#[cfg(feature = "http")]
struct CannedTimeout {
    endpoint: String,
    lanes_recorded: Vec<String>,
}

#[cfg(feature = "http")]
impl PartnerAdapter for CannedTimeout {
    fn receive<'a>(
        &'a self,
        _lane_key: &'a str,
        _source_uri: &'a str,
        deadline: Duration,
    ) -> BoxFuture<'a, Result<IncomingMessage, ReceiveError>> {
        Box::pin(async move {
            Err(ReceiveError::Timeout(ReceiveTimeout {
                endpoint: self.endpoint.clone(),
                deadline,
                elapsed: Duration::ZERO,
                lanes_recorded: self.lanes_recorded.clone(),
            }))
        })
    }
}

/// An adapter whose send fails with a canned lane FIFO overflow,
/// handing the lane key over exactly as the adapter rendered it —
/// RAW from a third-party adapter (ADR-0051).
#[cfg(feature = "http")]
struct CannedOverflow {
    lane_key: String,
    bound: usize,
}

#[cfg(feature = "http")]
impl PartnerAdapter for CannedOverflow {
    fn send<'a>(
        &'a self,
        _lane_key: &'a str,
        _target_uri: &'a str,
        msg: OutgoingMessage,
    ) -> BoxFuture<'a, Result<Option<Exchange>, TransportError>> {
        let _ = msg;
        Box::pin(async move {
            Err(TransportError::LaneFifoOverflow {
                lane_key: self.lane_key.clone(),
                bound: self.bound,
            })
        })
    }

    fn receive<'a>(
        &'a self,
        _lane_key: &'a str,
        _source_uri: &'a str,
        _deadline: Duration,
    ) -> BoxFuture<'a, Result<IncomingMessage, ReceiveError>> {
        Box::pin(async move {
            Err(ReceiveError::Transport(TransportError::Other {
                message: "adapter does not implement server-role receives".to_string(),
            }))
        })
    }
}

/// The printed receive-timeout failure carries the REDACTED endpoint
/// the construction site built — never the raw declared endpoint —
/// so a query-bearing declaration leaks no secret value into the CLI
/// FAIL line or the JUnit artifacts (ADR-0051).
#[tokio::test]
#[cfg(feature = "http")]
async fn receive_timeout_failure_carries_redacted_endpoint() {
    let declared = "http://host/login?authPassword=hunter2&x=1";
    let router = PartnerRouter::new(BTreeMap::from([(
        declared.to_string(),
        Box::new(CannedTimeout {
            endpoint: "http://host/login?authPassword=***&x=1".to_string(),
            lanes_recorded: vec!["/login?authPassword=***&x=1".to_string()],
        }) as Box<dyn PartnerAdapter>,
    )]));
    let doc = doc_with(vec![ScenarioAction::Receive {
        from: endpoint(declared),
        deadline: Duration::from_millis(50),
        extract: None,
    }]);
    let mut vars = ScenarioVars::new();
    let failure = run_scenario(&doc, &router, &mut vars)
        .await
        .expect_err("the receive must time out");
    let text = failure.to_string();
    assert!(
        !text.contains("hunter2"),
        "the raw secret must never print: {text}"
    );
    assert!(
        text.contains("authPassword=***"),
        "the redacted form must print: {text}"
    );
    assert!(
        text.contains("x=1"),
        "non-secret query keys must stay visible: {text}"
    );
}

/// Render-site defense for third-party adapters: a receive-timeout
/// handed over with a RAW endpoint and RAW lane evidence (no
/// adapter-side redaction) still prints masked, because the runner
/// holds the secret set and redaction is idempotent on already-masked
/// output (ADR-0051).
#[tokio::test]
#[cfg(feature = "http")]
async fn raw_adapter_timeout_redacts_at_the_mapping() {
    let declared = "http://host/login?authPassword=hunter2&x=1";
    let router = PartnerRouter::new(BTreeMap::from([(
        declared.to_string(),
        Box::new(CannedTimeout {
            endpoint: "http://host/login?authPassword=hunter2&x=1".to_string(),
            lanes_recorded: vec!["/login?authPassword=hunter2&x=1".to_string()],
        }) as Box<dyn PartnerAdapter>,
    )]));
    router.set_secret_query_keys(vec!["authPassword".to_string()]);
    let doc = doc_with(vec![ScenarioAction::Receive {
        from: endpoint(declared),
        deadline: Duration::from_millis(50),
        extract: None,
    }]);
    let mut vars = ScenarioVars::new();
    let failure = run_scenario(&doc, &router, &mut vars)
        .await
        .expect_err("the receive must time out");
    let text = failure.to_string();
    assert!(
        !text.contains("hunter2"),
        "the raw secret must never print: {text}"
    );
    assert!(
        text.contains("authPassword=***"),
        "the redacted form must print: {text}"
    );
    assert!(
        text.contains("x=1"),
        "non-secret query keys must stay visible: {text}"
    );
}

/// The lane FIFO overflow on send prints the REDACTED lane key: the
/// runner's render-site mapping holds the secret set, and a
/// third-party adapter may hand the raw endpoint URI over in the
/// overflow (ADR-0051).
#[tokio::test]
#[cfg(feature = "http")]
async fn raw_lane_fifo_overflow_redacts_at_the_mapping() {
    let declared = "http://host/login?authPassword=hunter2&x=1";
    let router = PartnerRouter::new(BTreeMap::from([(
        declared.to_string(),
        Box::new(CannedOverflow {
            lane_key: "http://host/login?authPassword=hunter2&x=1".to_string(),
            bound: 64,
        }) as Box<dyn PartnerAdapter>,
    )]));
    router.set_secret_query_keys(vec!["authPassword".to_string()]);
    let doc = doc_with(vec![ScenarioAction::Send {
        to: endpoint(declared),
        body: None,
        headers: None,
        method: "POST".to_string(),
        expect_reply: None,
    }]);
    let mut vars = ScenarioVars::new();
    let failure = run_scenario(&doc, &router, &mut vars)
        .await
        .expect_err("the send must fail at the transport");
    let text = failure.to_string();
    assert!(
        !text.contains("hunter2"),
        "the raw secret must never print: {text}"
    );
    assert!(
        text.contains("authPassword=***"),
        "the redacted form must print: {text}"
    );
    assert!(
        text.contains("x=1"),
        "non-secret query keys must stay visible: {text}"
    );
}

/// A body validation on a query-bearing declaration prints the
/// REDACTED subject — never the raw declared endpoint — so a
/// successful receive followed by a failing `received:` body check
/// leaks no secret value into the FAIL line (ADR-0051).
#[tokio::test]
async fn body_validation_failure_carries_redacted_subject() {
    let declared = "http://host/login?authPassword=hunter2&x=1";
    let fake = FakeAdapter::scripted(vec![text_message("mismatch-me")]);
    let router = router_for(declared, fake);
    router.set_secret_query_keys(vec!["authPassword".to_string()]);
    let doc = doc_with(vec![
        ScenarioAction::Receive {
            from: endpoint(declared),
            deadline: Duration::from_secs(1),
            extract: None,
        },
        ScenarioAction::Validate {
            target: ScenarioTarget::LastReceived(endpoint(declared)),
            expectation: ValidateExpectation::Message(Expectation::Equals(Value::String(
                "expected".to_string(),
            ))),
            deadline: None,
            elapsed_at_least: None,
        },
    ]);
    let mut vars = ScenarioVars::new();
    let failure = run_scenario(&doc, &router, &mut vars)
        .await
        .expect_err("the body validation must fail");
    let text = failure.to_string();
    assert!(
        !text.contains("hunter2"),
        "the raw secret must never print: {text}"
    );
    assert!(
        text.contains("authPassword=***"),
        "the redacted form must print: {text}"
    );
    assert!(
        text.contains("x=1"),
        "non-secret query keys must stay visible: {text}"
    );
}

/// A lastReceived validation before any receive prints the REDACTED
/// endpoint in its "no message has been received" detail (ADR-0051).
#[tokio::test]
async fn unreceived_validate_carries_redacted_subject() {
    let declared = "http://host/login?authPassword=hunter2&x=1";
    let router = router_for(declared, FakeAdapter::scripted(vec![]));
    router.set_secret_query_keys(vec!["authPassword".to_string()]);
    let doc = doc_with(vec![ScenarioAction::Validate {
        target: ScenarioTarget::LastReceived(endpoint(declared)),
        expectation: ValidateExpectation::Message(Expectation::Exists),
        deadline: None,
        elapsed_at_least: None,
    }]);
    let mut vars = ScenarioVars::new();
    let failure = run_scenario(&doc, &router, &mut vars)
        .await
        .expect_err("the validation must find no message");
    let text = failure.to_string();
    assert!(
        !text.contains("hunter2"),
        "the raw secret must never print: {text}"
    );
    assert!(
        text.contains("authPassword=***"),
        "the redacted form must print: {text}"
    );
    assert!(
        text.contains("x=1"),
        "non-secret keys must stay visible: {text}"
    );
}

/// The transport `Unbound` backstop on receive renders the declared
/// endpoint redacted when the router holds a secret set (ADR-0051).
#[tokio::test]
async fn unbound_receive_failure_carries_redacted_endpoint() {
    let declared = "http://host/login?authPassword=hunter2&x=1";
    let router = PartnerRouter::new(BTreeMap::new());
    router.set_secret_query_keys(vec!["authPassword".to_string()]);
    let error = router
        .receive(declared, declared, Duration::from_millis(1))
        .await
        .expect_err("no adapter is registered");
    let ReceiveError::Transport(TransportError::Unbound { endpoint }) = error else {
        panic!("expected an Unbound transport failure, got {error:?}");
    };
    assert!(
        !endpoint.contains("hunter2"),
        "the raw secret must never print: {endpoint}"
    );
    assert!(
        endpoint.contains("authPassword=***"),
        "the redacted form must print: {endpoint}"
    );
}

/// The transport `Unbound` backstop on send renders the declared
/// endpoint redacted when the router holds a secret set (ADR-0051).
#[tokio::test]
async fn unbound_send_failure_carries_redacted_endpoint() {
    let declared = "http://host/login?authPassword=hunter2&x=1";
    let router = PartnerRouter::new(BTreeMap::new());
    router.set_secret_query_keys(vec!["authPassword".to_string()]);
    let error = router
        .send(
            declared,
            "partner://nowhere",
            OutgoingMessage {
                body: Value::Null,
                headers: BTreeMap::new(),
                method: "GET".to_string(),
            },
        )
        .await
        .expect_err("no adapter is registered");
    let TransportError::Unbound { endpoint } = error else {
        panic!("expected an Unbound transport failure, got {error:?}");
    };
    assert!(
        !endpoint.contains("hunter2"),
        "the raw secret must never print: {endpoint}"
    );
    assert!(
        endpoint.contains("authPassword=***"),
        "the redacted form must print: {endpoint}"
    );
}

// -------------------------------------------------------------------------
// Scenario `sql:` action end-to-end (bd rc-25lup.1)
//
// Every test here is project-based like `boot_scenario_test`: a
// temporary `Camel.toml` with a datasource, the minimal route file,
// and a `.test.yaml` document parsed through
// `parse_scenario_document`. The boot owns the catalog, so the run
// exercises the exact production wiring: boot → `run.boot
// .datasource_catalog()` → `run_scenario_document`.
// -------------------------------------------------------------------------

/// The shared-cache in-memory URL every sql e2e test boots with. The
/// boot-time lint rejects the bare form. `max_connections = 1` keeps
/// every statement on one connection so CREATE/INSERT state cannot
/// split across pooled connections (the camel-sql `:memory:` test
/// precedent: consumer.rs, health.rs, producer.rs, and the executor
/// stub in `sql_action_test`).
#[cfg(feature = "sql")]
const SQL_E2E_DATASOURCE: &str = r#"
[datasources.appdb]
db_url = "sqlite::memory:?cache=shared"
max_connections = 1
"#;

/// The minimal route file: one unconsumed `direct:` route, so the
/// boot starts exactly one route and the `sql:` action is the only
/// scenario behavior.
#[cfg(feature = "sql")]
const SQL_E2E_ROUTE: &str = r#"
routes:
  - id: boot-route
    from: direct:start
    steps:
      - to: log:info
"#;

/// Writes the temporary sql e2e project (the datasource `Camel.toml`,
/// the minimal route file, and the `.test.yaml` document) and returns
/// the directory plus the parsed document. Only for VALID documents:
/// parsing panics on a rejected one, so the load-error tests write
/// their files directly.
#[cfg(feature = "sql")]
fn sql_project(doc: &str) -> (tempfile::TempDir, ScenarioDocument) {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("Camel.toml"), SQL_E2E_DATASOURCE).expect("write Camel.toml");
    std::fs::write(dir.path().join("routes.yaml"), SQL_E2E_ROUTE).expect("write route file");
    let doc_path = dir.path().join("case.test.yaml");
    std::fs::write(&doc_path, doc).expect("write document");
    let document = crate::parse_scenario_document(&doc_path).expect("document parses");
    (dir, document)
}

/// Writes the sql e2e project's `Camel.toml` and route file into
/// `dir`, for the load-error tests that hand-write an invalid
/// document.
#[cfg(feature = "sql")]
fn sql_project_files(dir: &tempfile::TempDir, doc: &str) -> std::path::PathBuf {
    std::fs::write(dir.path().join("Camel.toml"), SQL_E2E_DATASOURCE).expect("write Camel.toml");
    std::fs::write(dir.path().join("routes.yaml"), SQL_E2E_ROUTE).expect("write route file");
    let doc_path = dir.path().join("case.test.yaml");
    std::fs::write(&doc_path, doc).expect("write document");
    doc_path
}

/// Boots the project with an empty layered environment and runs its
/// document through [`run_scenario_document`] with the boot's own
/// datasource catalog. The run is returned so a test can verify the
/// seeded state through the same catalog before shutdown.
#[cfg(feature = "sql")]
async fn sql_e2e_run(
    dir: &tempfile::TempDir,
    doc: &ScenarioDocument,
) -> (
    crate::boot_scenario::ScenarioRun,
    DocumentOutcome,
    std::sync::Arc<dyn camel_api::datasource::DatasourceCatalog>,
) {
    use crate::env_layers::{LayeredEnv, ambient_std};
    let env = LayeredEnv::new(BTreeMap::new(), BTreeMap::new(), Vec::new(), ambient_std());
    let run = crate::boot_scenario::boot_scenario(doc, dir.path(), &env)
        .await
        .expect("the sql project must boot");
    let catalog = run.boot.datasource_catalog();
    let router = PartnerRouter::new(BTreeMap::new());
    let mut vars = ScenarioVars::new();
    let outcome = run_scenario_document(doc, &router, &mut vars, Some(&catalog)).await;
    (run, outcome, catalog)
}

/// The full happy path: one `sql:` action seeds a table and the run
/// passes; the test-side read through the SAME catalog the boot
/// handed the runner pins the single-catalog invariant — the seeds
/// landed in the pool the routes resolve.
#[tokio::test]
#[cfg(feature = "sql")]
async fn sql_prepare_seeds_and_proceeds() {
    use sqlx::Row;
    let (dir, doc) = sql_project(
        r#"
routeFiles: [routes.yaml]
scenario:
- sql:
    datasource: appdb
    prepare:
    - CREATE TABLE t (v TEXT)
    - INSERT INTO t VALUES ('seed')
"#,
    );
    let (mut run, outcome, catalog) = sql_e2e_run(&dir, &doc).await;
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "the seeded scenario must pass: {outcome:?}"
    );
    let handle = catalog.get_pool("appdb").await.expect("pool resolves");
    let pool = handle.downcast::<sqlx::AnyPool>().expect("any pool");
    let row = sqlx::query("SELECT COUNT(*) AS n FROM t")
        .fetch_one(&*pool)
        .await
        .expect("the seeded table must be readable");
    let n: i64 = row.get("n");
    assert_eq!(n, 1, "exactly one seed row must exist");
    run.boot.shutdown(&mut run.ctx).await.expect("shutdown");
}

/// Item 0 is a read (`select` prefix): doc-validation names the action
/// index and the statement index, and the document never boots.
#[tokio::test]
#[cfg(feature = "sql")]
async fn sql_read_statement_is_load_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    let doc_path = sql_project_files(
        &dir,
        r#"
routeFiles: [routes.yaml]
scenario:
- sql:
    datasource: appdb
    prepare:
    - (SELECT 1)
"#,
    );
    let err =
        crate::parse_scenario_document(&doc_path).expect_err("a read prepare statement must fail");
    match err {
        crate::DocError::Validation { index, message } => {
            assert_eq!(index, 0, "the error must name the action index");
            assert!(
                message.contains("statement 0"),
                "the error must name the statement index: {message}"
            );
        }
        other => panic!("expected Validation, got {other:?}"),
    }
}

/// A CTE read (`with` prefix) is a read too: the same load-error
/// shape as the `select` prefix.
#[tokio::test]
#[cfg(feature = "sql")]
async fn sql_with_statement_is_load_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    let doc_path = sql_project_files(
        &dir,
        r#"
routeFiles: [routes.yaml]
scenario:
- sql:
    datasource: appdb
    prepare:
    - with cte as (select 1) select * from cte
"#,
    );
    let err = crate::parse_scenario_document(&doc_path).expect_err("a CTE read must fail");
    match err {
        crate::DocError::Validation { index, message } => {
            assert_eq!(index, 0, "the error must name the action index");
            assert!(
                message.contains("statement 0"),
                "the error must name the statement index: {message}"
            );
        }
        other => panic!("expected Validation, got {other:?}"),
    }
}

/// An empty prepare list names the action index at load.
#[tokio::test]
#[cfg(feature = "sql")]
async fn sql_empty_prepare_is_load_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    let doc_path = sql_project_files(
        &dir,
        r#"
routeFiles: [routes.yaml]
scenario:
- sql:
    datasource: appdb
    prepare: []
"#,
    );
    let err =
        crate::parse_scenario_document(&doc_path).expect_err("an empty prepare list must fail");
    match err {
        crate::DocError::Validation { index, message } => {
            assert_eq!(index, 0, "the error must name the action index");
            assert!(
                message.contains("prepare list must not be empty"),
                "the error must name the empty prepare list: {message}"
            );
        }
        other => panic!("expected Validation, got {other:?}"),
    }
}

/// A UNIQUE violation on statement [2] stops the run, names the
/// statement index, and redacts both the datasource URL and the row
/// value the statement carried (ADR-0051): the diagnostic must carry
/// neither the configured db_url nor the seeded literal.
#[tokio::test]
#[cfg(feature = "sql")]
async fn sql_failure_redacts_and_stops() {
    let (dir, doc) = sql_project(
        r#"
routeFiles: [routes.yaml]
scenario:
- sql:
    datasource: appdb
    prepare:
    - CREATE TABLE t (v TEXT UNIQUE)
    - INSERT INTO t VALUES ('LEAKROW7')
    - INSERT INTO t VALUES ('LEAKROW7')
"#,
    );
    let (mut run, outcome, _catalog) = sql_e2e_run(&dir, &doc).await;
    assert_eq!(outcome.verdict, None, "the duplicate insert must fail");
    assert_eq!(
        outcome.per_action.len(),
        1,
        "only the failing action's outcome is recorded"
    );
    let Err(failure) = &outcome.per_action[0] else {
        panic!("expected a failure, got {:?}", outcome.per_action[0]);
    };
    let text = failure.to_string();
    assert!(
        text.contains("statement [2]"),
        "the failure must name the statement index: {text}"
    );
    assert!(
        !text.contains("sqlite::memory:"),
        "the datasource URL must be redacted: {text}"
    );
    assert!(
        !text.contains("LEAKROW7"),
        "the failing statement's literal must not print: {text}"
    );
    run.boot.shutdown(&mut run.ctx).await.expect("shutdown");
}

/// An unknown datasource fails closed: the failure names the
/// datasource and carries nothing URL-shaped.
#[tokio::test]
#[cfg(feature = "sql")]
async fn sql_unknown_datasource_fails_closed() {
    let (dir, doc) = sql_project(
        r#"
routeFiles: [routes.yaml]
scenario:
- sql:
    datasource: nosuch
    prepare:
    - CREATE TABLE t (v TEXT)
"#,
    );
    let (mut run, outcome, _catalog) = sql_e2e_run(&dir, &doc).await;
    assert_eq!(outcome.verdict, None, "the unknown datasource must fail");
    let Err(failure) = &outcome.per_action[0] else {
        panic!("expected a failure, got {:?}", outcome.per_action[0]);
    };
    let text = failure.to_string();
    assert!(
        text.contains("nosuch"),
        "the failure must name the datasource: {text}"
    );
    assert!(
        !text.contains("sqlite::memory:"),
        "no URL may leak through a failed datasource lookup: {text}"
    );
    run.boot.shutdown(&mut run.ctx).await.expect("shutdown");
}
