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
#[cfg(all(test, feature = "sql"))]
use camel_matchers::RowsExpectation;
use futures::future::BoxFuture;

#[cfg(feature = "http")]
use crate::adapters::ReceiveTimeout;
use crate::adapters::{
    ArrivalLaneOverflow, FakeAdapter, IncomingMessage, OutgoingMessage, PartnerAdapter,
    PartnerRouter, ReceiveError, TransportError,
};
use crate::document::{
    EndpointRef, Expectation, Provisioning, RouteSource, ScenarioAction, ScenarioDocument,
    ScenarioTarget, SqlTarget, ValidateExpectation,
};
use crate::runner::{
    DocumentOutcome, ScenarioFailure, ScenarioVars, ScenarioVerdict, effective_send_deadline,
    fill_bind_vars, interpolate_value, reply_body_value, resolve_placeholders, run_scenario,
    run_scenario_document,
};
#[cfg(all(test, feature = "sql"))]
use crate::sql_stub::{seed, sqlite_catalog};

#[cfg(feature = "http")]
use crate::adapters::http::HttpPartner;

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

/// A raw third-party overflow key carrying a space inside a secret
/// query value stays ONE value at the runner's render site: the
/// pre-rendered two-half shape is the only string that redacts per
/// half, so splitting can never sever a secret value and print its
/// tail (ADR-0051 fail-safe).
#[tokio::test]
#[cfg(feature = "http")]
async fn raw_overflow_key_with_space_redacts_whole() {
    let declared = "http://host/login";
    let router = PartnerRouter::new(BTreeMap::from([(
        declared.to_string(),
        Box::new(CannedOverflow {
            lane_key: "http://host/login?authPassword=hunter 2&x=1".to_string(),
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
        !text.contains("hunter") && !text.contains(" 2"),
        "no fragment of the severed secret value may print: {text}"
    );
    assert!(
        text.contains("authPassword=***"),
        "the whole value masks: {text}"
    );
}

/// The pre-rendered two-half overflow form ("key path", the http
/// lane's own render) redacts per half: a secret in the key half
/// masks, and the path half stays visible instead of merging into
/// the key half's query span (ADR-0051).
#[tokio::test]
#[cfg(feature = "http")]
async fn prerendered_overflow_form_redacts_per_half() {
    let declared = "http://host/login";
    let router = PartnerRouter::new(BTreeMap::from([(
        declared.to_string(),
        Box::new(CannedOverflow {
            lane_key: "http://host/login?authPassword=hunter2 /orders?x=1".to_string(),
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
        "the key half's secret masks: {text}"
    );
    assert!(
        text.contains("authPassword=***"),
        "the key half keeps its redacted shape: {text}"
    );
    assert!(
        text.contains("/orders?x=1"),
        "the path half stays visible, not swallowed: {text}"
    );
}
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

// -------------------------------------------------------------------------
// `validate` sql-target runner dispatch (bd rc-25lup.2, task 3.2)
//
// The dispatch under test is `run_action`'s target/expectation
// pairing, exercised at the runner level through the public entries:
// the catalog rides `run_scenario_document`'s `datasource_catalog`
// parameter exactly as the production boot hands it over, and
// `run_scenario` always passes `None`. The unit-level executor
// behaviors (poll lattice, projection, redaction) live in
// `sql_validate_test.rs` (task 3.1); the stub catalog + seed helpers
// live in `crate::sql_stub` (bd rc-mu3aq).
// -------------------------------------------------------------------------

/// The runner-level happy path: a `sql` target paired with the rows
/// grammar routes through the dispatch into the catalog-backed
/// executor, and the seeded ordered rows pass.
#[tokio::test]
#[cfg(all(test, feature = "sql"))]
async fn validate_sql_routes_through_catalog() {
    let catalog = sqlite_catalog("appdb");
    seed(
        &catalog,
        "appdb",
        &[
            "CREATE TABLE t_dispatch (id INTEGER, name TEXT)",
            "INSERT INTO t_dispatch VALUES (1, 'alice')",
            "INSERT INTO t_dispatch VALUES (2, 'bob')",
        ],
    )
    .await;
    let doc = doc_with(vec![ScenarioAction::Validate {
        target: ScenarioTarget::Sql(SqlTarget {
            datasource: "appdb".to_string(),
            query: "SELECT id, name FROM t_dispatch ORDER BY id".to_string(),
        }),
        expectation: ValidateExpectation::Rows(RowsExpectation {
            columns: Some(vec!["id".to_string(), "name".to_string()]),
            unordered: false,
            rows: Some(vec![
                vec![
                    Expectation::Equals(Value::Number(1.into())),
                    Expectation::Equals(Value::String("alice".to_string())),
                ],
                vec![
                    Expectation::Equals(Value::Number(2.into())),
                    Expectation::Equals(Value::String("bob".to_string())),
                ],
            ]),
            bound: None,
        }),
        deadline: None,
        elapsed_at_least: None,
    }]);
    let router = PartnerRouter::new(BTreeMap::new());
    let mut vars = ScenarioVars::new();
    let outcome = run_scenario_document(&doc, &router, &mut vars, Some(&catalog)).await;
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "the seeded sql validate must pass: {outcome:?}"
    );
}

/// The fail-closed backstop at the runner level: the same action with
/// no catalog in hand is an apparatus-class failure naming the
/// missing catalog, never a silently skipped assertion.
#[tokio::test]
#[cfg(all(test, feature = "sql"))]
async fn validate_sql_without_catalog_fails_closed() {
    let doc = doc_with(vec![ScenarioAction::Validate {
        target: ScenarioTarget::Sql(SqlTarget {
            datasource: "appdb".to_string(),
            query: "SELECT id FROM t_dispatch_nocat".to_string(),
        }),
        expectation: ValidateExpectation::Rows(RowsExpectation {
            columns: None,
            unordered: false,
            rows: Some(vec![vec![Expectation::Equals(Value::Number(1.into()))]]),
            bound: None,
        }),
        deadline: None,
        elapsed_at_least: None,
    }]);
    let router = PartnerRouter::new(BTreeMap::new());
    let mut vars = ScenarioVars::new();
    let outcome = run_scenario_document(&doc, &router, &mut vars, None).await;
    assert_eq!(
        outcome.verdict, None,
        "the missing catalog must fail the run: {outcome:?}"
    );
    match &outcome.per_action[0] {
        Err(ScenarioFailure::ActionTransport { source, .. }) => {
            let crate::adapters::TransportError::Other { message } = source else {
                panic!("expected TransportError::Other, got {source:?}");
            };
            assert!(
                message.contains("no datasource catalog"),
                "the failure must name the missing catalog: {message}"
            );
        }
        other => panic!("expected ActionTransport on action 0, got {other:?}"),
    }
}

/// A `sql` target paired with the message grammar is a pairing the
/// parser never produces: the runner fails closed with the
/// unpaired-validate detail naming the sql/rows rule. The action is
/// constructed directly — no catalog is ever touched, so the test
/// compiles and runs in both feature configurations.
#[tokio::test]
async fn unpaired_validate_sql_message() {
    let doc = doc_with(vec![ScenarioAction::Validate {
        target: ScenarioTarget::Sql(SqlTarget {
            datasource: "appdb".to_string(),
            query: "SELECT 1".to_string(),
        }),
        expectation: ValidateExpectation::Message(Expectation::Exists),
        deadline: None,
        elapsed_at_least: None,
    }]);
    let router = PartnerRouter::new(BTreeMap::new());
    let mut vars = ScenarioVars::new();
    let failure = run_scenario(&doc, &router, &mut vars)
        .await
        .expect_err("the unpaired validate must fail the scenario");
    let ScenarioFailure::ValidationMismatch { action: 0, detail } = failure else {
        panic!("expected ValidationMismatch, got {failure:?}");
    };
    assert!(
        detail.contains("sql"),
        "the detail must name the sql target rule: {detail}"
    );
    assert!(
        detail.contains("rows"),
        "the detail must name the rows grammar rule: {detail}"
    );
}
