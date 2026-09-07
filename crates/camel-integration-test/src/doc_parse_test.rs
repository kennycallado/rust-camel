//! Scenario document parser tests (ADR-0069 sections 1-2).
//!
//! Unit-test module of the lib target, declared in `src/lib.rs` under
//! `#[cfg(test)]`; `cargo test -p camel-integration-test --lib` runs
//! these tests and nothing else. Every test is path-based: it writes a
//! temporary `.test.yaml` document and parses it through
//! [`crate::parse_scenario_document`].

use std::collections::BTreeMap;
use std::time::Duration;

use crate::document::is_http_token;
use crate::{
    CountBound, DocError, Expectation, PartnerExpectation, Provisioning, ScenarioAction,
    ScenarioDocument, ScenarioTarget, ValidateExpectation, parse_scenario_document,
};

/// Writes `text` to a fresh temporary `case.test.yaml` and parses it.
fn parse_case(text: &str) -> Result<ScenarioDocument, DocError> {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("case.test.yaml");
    std::fs::write(&path, text).expect("write case file");
    parse_scenario_document(&path)
}

#[test]
fn mixed_vocabulary_rejected() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
inputs:
- to: direct:start
  body: hello
"#,
    )
    .expect_err("parse must fail");
    assert!(
        matches!(err, DocError::MixedVocabulary { .. }),
        "expected MixedVocabulary, got {err}"
    );
    assert!(
        err.to_string().contains("doc-validation"),
        "error must name the doc-validation class: {err}"
    );
}

#[test]
fn empty_scenario_rejected() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario: []
"#,
    )
    .expect_err("parse must fail");
    assert!(
        matches!(err, DocError::Validation { .. }),
        "expected Validation, got {err}"
    );
    let display = err.to_string();
    assert!(
        display.contains("doc-validation"),
        "error must name the doc-validation class: {display}"
    );
    assert!(
        display.contains("scenario"),
        "error must name the `scenario` section: {display}"
    );
}

#[test]
fn scenario_with_expects_rejected() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
expects:
  mock:result:
    count: 1
"#,
    )
    .expect_err("parse must fail");
    assert!(
        matches!(err, DocError::MixedVocabulary { .. }),
        "expected MixedVocabulary, got {err}"
    );
}

#[test]
fn receive_without_deadline_rejected() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
- receive:
    from: http://127.0.0.1:18080/hook
"#,
    )
    .expect_err("parse must fail");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 1, "error must name the action index");
            assert!(
                message.contains("deadline"),
                "message must name the missing deadline: {message}"
            );
        }
        other => panic!("expected Validation, got {other}"),
    }
}

#[test]
fn invalid_send_deadline_is_load_error() {
    let err = parse_case(
        r#"
sendDeadline: soon
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
"#,
    )
    .expect_err("parse must fail");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 0, "the section, not an action, failed");
            assert!(
                message.contains("sendDeadline"),
                "message must name the `sendDeadline` field: {message}"
            );
        }
        other => panic!("expected Validation, got {other}"),
    }
}

#[test]
fn send_method_explicit_put_resolves() {
    let doc = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
    method: PUT
"#,
    )
    .expect("parse must succeed");
    let action = doc.scenario.first().expect("one action");
    match action {
        ScenarioAction::Send { method, .. } => {
            assert_eq!(method, "PUT", "explicit method must resolve verbatim");
        }
        other => panic!("expected Send, got {other:?}"),
    }
}

#[test]
fn send_method_inferred_post_with_body() {
    let doc = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: http://127.0.0.1:9999/hook
    body: hello
"#,
    )
    .expect("parse must succeed");
    let action = doc.scenario.first().expect("one action");
    match action {
        ScenarioAction::Send { method, .. } => {
            assert_eq!(method, "POST", "a send with a body infers POST");
        }
        other => panic!("expected Send, got {other:?}"),
    }
}

#[test]
fn send_method_inferred_get_bodyless() {
    let doc = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: http://127.0.0.1:9999/hook
"#,
    )
    .expect("parse must succeed");
    let action = doc.scenario.first().expect("one action");
    match action {
        ScenarioAction::Send { method, .. } => {
            assert_eq!(method, "GET", "a bodyless send infers GET");
        }
        other => panic!("expected Send, got {other:?}"),
    }
}

#[test]
fn send_method_lowercase_normalizes() {
    let doc = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: http://127.0.0.1:9999/hook
    method: delete
"#,
    )
    .expect("parse must succeed");
    let action = doc.scenario.first().expect("one action");
    match action {
        ScenarioAction::Send { method, .. } => {
            assert_eq!(method, "DELETE", "method must normalize to uppercase");
        }
        other => panic!("expected Send, got {other:?}"),
    }
}

#[test]
fn send_method_invalid_token_is_doc_validation() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
    method: "P UT"
"#,
    )
    .expect_err("parse must fail");
    let rendered = err.to_string();
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 0, "error must name the action index");
            assert!(
                message.contains("HTTP method name"),
                "message must name the HTTP method name requirement: {message}"
            );
            assert!(
                message.contains("P UT"),
                "message must name the offending value: {message}"
            );
        }
        other => panic!("expected Validation, got {other}"),
    }
    assert!(
        rendered.contains("doc-validation"),
        "rendered error must name the doc-validation class: {rendered}"
    );
}

#[test]
fn send_method_token_predicate_accepts_and_rejects() {
    for accepted in ["PUT", "X-Custom", "PATCH2"] {
        assert!(
            is_http_token(accepted),
            "predicate must accept `{accepted}`"
        );
    }
    for rejected in ["", "P UT", "PUT/", "put;", "café"] {
        assert!(
            !is_http_token(rejected),
            "predicate must reject `{rejected}`"
        );
    }
}

#[test]
fn scenario_with_env_accepted() {
    let doc = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
env:
  HTTP_PORT: "18080"
"#,
    )
    .expect("parse must succeed");
    let env = doc.env.expect("env map must be present");
    assert_eq!(
        env.get("HTTP_PORT").map(String::as_str),
        Some("18080"),
        "env map must carry HTTP_PORT"
    );
}

#[test]
fn expect_reply_on_partner_send_is_load_error() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: http://127.0.0.1:9999/hook
    expectReply:
      contains: x
"#,
    )
    .expect_err("parse must fail");
    let rendered = err.to_string();
    match err {
        DocError::ExpectReplyOnUnsupportedSend { index, scheme } => {
            assert_eq!(index, 0, "error must name the action index");
            assert_eq!(scheme.as_str(), "http", "error must name the send's scheme");
        }
        other => panic!("expected ExpectReplyOnUnsupportedSend, got {other}"),
    }
    assert!(
        rendered.contains("scenario[0]"),
        "rendered error must name the action index: {rendered}"
    );
    assert!(
        rendered.contains("http"),
        "rendered error must name the scheme: {rendered}"
    );
    assert!(
        rendered.contains("expectReply"),
        "rendered error must name the literal `expectReply` field: {rendered}"
    );
    assert!(
        rendered.contains("doc-validation"),
        "rendered error must name the doc-validation class: {rendered}"
    );
}

#[test]
fn expect_reply_on_fake_send_is_load_error() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: fake:orders
    expectReply:
      contains: x
"#,
    )
    .expect_err("parse must fail");
    match err {
        DocError::ExpectReplyOnUnsupportedSend { index, ref scheme } => {
            assert_eq!(index, 0, "error must name the action index");
            assert_eq!(scheme.as_str(), "fake", "error must name the send's scheme");
        }
        other => panic!("expected ExpectReplyOnUnsupportedSend, got {other}"),
    }
    assert!(
        err.to_string().contains("expectReply"),
        "rendered error must name the literal `expectReply` field: {err}"
    );
}

#[test]
fn expect_reply_on_schemeless_send_is_load_error() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: orders
    expectReply:
      contains: x
"#,
    )
    .expect_err("parse must fail");
    let rendered = err.to_string();
    match err {
        DocError::ExpectReplyOnUnsupportedSend { index, .. } => {
            assert_eq!(index, 0, "error must name the action index");
        }
        other => panic!("expected ExpectReplyOnUnsupportedSend, got {other}"),
    }
    assert!(
        rendered.contains("no scheme"),
        "a scheme-less reference must render an explicit phrase, not an\
         empty or pseudo scheme name: {rendered}"
    );
}

#[test]
fn reserved_provisioning_rejected() {
    for value in ["testcontainer", "user-provided"] {
        let text = format!(
            r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:9999/hook
      provisioning: {value}
"#
        );
        let err = parse_case(&text).expect_err("parse must fail");
        let rendered = err.to_string();
        match err {
            DocError::UnsupportedProvisioning {
                value: seen,
                endpoint,
            } => {
                assert_eq!(seen, value, "error must name the reserved value");
                assert_eq!(
                    endpoint, "http://127.0.0.1:9999/hook",
                    "error must name the endpoint"
                );
            }
            other => panic!("expected UnsupportedProvisioning, got {other}"),
        }
        assert!(
            rendered.contains(value),
            "rendered error must name the value: {rendered}"
        );
    }
}

#[test]
fn inline_routes_rejected_at_load() {
    let err = parse_case(
        r#"
routes:
  - id: inline-route
    from: direct:start
    steps:
      - to: log:info
scenario:
  - sleep:
      duration: 1s
"#,
    )
    .expect_err("parse must fail");
    assert!(
        matches!(err, DocError::InlineRoutesRejected),
        "expected InlineRoutesRejected, got {err}"
    );
    let display = err.to_string();
    assert!(
        display.contains("routeFiles"),
        "error must direct the author to `routeFiles`: {display}"
    );
    assert!(
        display.contains("doc-validation"),
        "error must name the doc-validation class: {display}"
    );
}

#[test]
fn harness_provisioning_direct_bindvar_rejected_at_load() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: direct:start
      provisioning: harness
      bindVar: P
"#,
    )
    .expect_err("parse must fail");
    let rendered = err.to_string();
    match err {
        DocError::ProvisioningWithoutAuthority {
            endpoint,
            ref_scheme,
        } => {
            assert_eq!(
                endpoint, "direct:start",
                "error must name the endpoint: {endpoint}"
            );
            assert_eq!(ref_scheme, "direct", "error must name the ref scheme");
        }
        other => panic!("expected ProvisioningWithoutAuthority, got {other}"),
    }
    assert!(
        rendered.contains("direct:start"),
        "rendered error must name the endpoint: {rendered}"
    );
    assert!(
        rendered.contains("direct"),
        "rendered error must name the scheme: {rendered}"
    );
    assert!(
        rendered.contains("bindVar"),
        "rendered error must name the missing bound authority: {rendered}"
    );
}

#[test]
fn harness_provisioning_fake_without_bindvar_loads() {
    let doc = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: fake:x
      provisioning: harness
"#,
    )
    .expect("parse must succeed");
    let action = doc.scenario.first().expect("one action");
    match action {
        ScenarioAction::Send { to, .. } => {
            assert_eq!(to.endpoint, "fake:x", "endpoint must parse verbatim");
            assert_eq!(
                to.provisioning,
                Some(Provisioning::Harness),
                "harness provisioning must parse"
            );
            assert!(to.bind_var.is_none(), "no bindVar was declared");
        }
        other => panic!("expected Send, got {other:?}"),
    }
}

#[test]
fn validate_last_received_target_parses() {
    let doc = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      lastReceived: http://127.0.0.1:9999/hook
    expectation:
      equals: ok
"#,
    )
    .expect("parse must succeed");
    let action = doc.scenario.first().expect("one action");
    match action {
        ScenarioAction::Validate {
            target,
            expectation,
            ..
        } => {
            match target {
                ScenarioTarget::LastReceived(endpoint) => assert_eq!(
                    endpoint.endpoint, "http://127.0.0.1:9999/hook",
                    "target must keep the endpoint reference"
                ),
                other => panic!("expected LastReceived, got {other:?}"),
            }
            assert_eq!(
                expectation,
                &ValidateExpectation::Message(Expectation::Equals(camel_api::Value::String(
                    "ok".into()
                ))),
                "expectation must parse as a literal equals"
            );
        }
        other => panic!("expected Validate, got {other:?}"),
    }
}

#[test]
fn validate_variable_target_parses() {
    let doc = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      variable: AUTH_TOKEN
    expectation:
      regex: "^Bearer .+$"
"#,
    )
    .expect("parse must succeed");
    let action = doc.scenario.first().expect("one action");
    match action {
        ScenarioAction::Validate {
            target,
            expectation,
            ..
        } => {
            assert_eq!(
                target,
                &ScenarioTarget::Variable("AUTH_TOKEN".to_string()),
                "target must parse as a scenario variable"
            );
            assert_eq!(
                expectation,
                &ValidateExpectation::Message(Expectation::Regex("^Bearer .+$".to_string())),
                "expectation must parse as a regex matcher"
            );
        }
        other => panic!("expected Validate, got {other:?}"),
    }
}

#[test]
fn invalid_regex_rejected_at_load() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      variable: AUTH_TOKEN
    expectation:
      regex: "[unclosed"
"#,
    )
    .expect_err("parse must fail");
    let rendered = err.to_string();
    match err {
        DocError::Validation { index, ref message } => {
            assert_eq!(index, 0, "error must name the action index");
            assert!(
                message.contains("regex"),
                "message must name the regex problem: {message}"
            );
        }
        other => panic!("expected Validation, got {other}"),
    }
    assert!(
        rendered.contains("doc-validation"),
        "rendered error must name the doc-validation class: {rendered}"
    );
}

#[test]
fn reserved_env_key_rejected() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:9999/hook
      provisioning: harness
      bindVar: PARTNER
env:
  PARTNER: http://127.0.0.1:9999
"#,
    )
    .expect_err("parse must fail");
    match err {
        DocError::ReservedEnvKey { key, endpoint } => {
            assert_eq!(key, "PARTNER", "error must name the reserved key");
            assert_eq!(
                endpoint, "http://127.0.0.1:9999/hook",
                "error must name the endpoint that reserved the key"
            );
        }
        other => panic!("expected ReservedEnvKey, got {other}"),
    }
}

// -------------------------------------------------------------------------
// Partner validate grammar (task 2.1)
// -------------------------------------------------------------------------

#[test]
fn partner_target_with_count_parses() {
    let doc = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/order
      provisioning: harness
      bindVar: PARTNER_URL
- validate:
    target:
      partner: http://127.0.0.1:0/order
    expectation:
      count: 3
      method: POST
"#,
    )
    .expect("parse must succeed");
    let action = doc.scenario.get(1).expect("two actions");
    match action {
        ScenarioAction::Validate {
            target,
            expectation,
            deadline,
            ..
        } => {
            match target {
                ScenarioTarget::Partner(endpoint) => assert_eq!(
                    endpoint.endpoint, "http://127.0.0.1:0/order",
                    "target must keep the partner endpoint reference"
                ),
                other => panic!("expected Partner, got {other:?}"),
            }
            assert_eq!(
                expectation,
                &ValidateExpectation::Partner(PartnerExpectation {
                    bound: CountBound::Exact(3),
                    method: Some("POST".to_string()),
                    path: None,
                    query: None,
                }),
                "expectation must parse as a partner count with a method filter"
            );
            assert!(deadline.is_none(), "absent deadline means one snapshot");
        }
        other => panic!("expected Validate, got {other:?}"),
    }
}

#[test]
fn undeclared_partner_target_is_load_error() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
- validate:
    target:
      partner: http://127.0.0.1:9999/nowhere
    expectation:
      count: 1
"#,
    )
    .expect_err("parse must fail");
    let rendered = err.to_string();
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 1, "error must name the action index");
            assert!(
                message.contains("http://127.0.0.1:9999/nowhere"),
                "message must name the unmatched URI: {message}"
            );
        }
        other => panic!("expected Validation, got {other}"),
    }
    assert!(
        rendered.contains("doc-validation"),
        "rendered error must name the doc-validation class: {rendered}"
    );
}

#[test]
fn deadline_on_lastreceived_is_load_error() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      lastReceived: http://127.0.0.1:9999/hook
    expectation:
      equals: ok
    deadline: 5s
"#,
    )
    .expect_err("parse must fail");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 0, "error must name the action index");
            assert!(
                message.contains("deadline"),
                "message must name `deadline`: {message}"
            );
        }
        other => panic!("expected Validation, got {other}"),
    }
}

#[test]
fn partner_missing_bound_fails() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/order
      provisioning: harness
- validate:
    target:
      partner: http://127.0.0.1:0/order
    expectation:
      method: POST
"#,
    )
    .expect_err("parse must fail");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 1, "error must name the action index");
            assert!(
                message.contains("count") && message.contains("atLeast"),
                "message must name the bound forms: {message}"
            );
        }
        other => panic!("expected Validation, got {other}"),
    }
}

#[test]
fn negative_count_is_load_error() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/order
      provisioning: harness
- validate:
    target:
      partner: http://127.0.0.1:0/order
    expectation:
      count: -1
"#,
    )
    .expect_err("parse must fail");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 1, "error must name the action index");
            assert!(
                message.contains("count"),
                "message must name `count`: {message}"
            );
        }
        other => panic!("expected Validation, got {other}"),
    }
}

#[test]
fn unknown_expectation_field_is_load_error() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/order
      provisioning: harness
- validate:
    target:
      partner: http://127.0.0.1:0/order
    expectation: {count: 1, duration: 5s}
"#,
    )
    .expect_err("parse must fail");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 1, "error must name the action index");
            assert!(
                message.contains("duration"),
                "message must name the unknown field `duration`: {message}"
            );
        }
        other => panic!("expected Validation, got {other}"),
    }
}

#[test]
fn unparseable_deadline_is_load_error() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/order
      provisioning: harness
- validate:
    target:
      partner: http://127.0.0.1:0/order
    expectation:
      count: 1
    deadline: 5x
"#,
    )
    .expect_err("parse must fail");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 1, "error must name the action index");
            assert!(
                message.contains("deadline"),
                "message must name `deadline`: {message}"
            );
        }
        other => panic!("expected Validation, got {other}"),
    }
}

#[test]
fn elapsed_at_least_parses_on_last_received() {
    let doc = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      lastReceived: http://127.0.0.1:9999/hook
    expectation:
      equals: ok
    elapsedAtLeast: 250ms
"#,
    )
    .expect("parse must succeed");
    let action = doc.scenario.first().expect("one action");
    match action {
        ScenarioAction::Validate {
            elapsed_at_least, ..
        } => {
            assert_eq!(
                *elapsed_at_least,
                Some(Duration::from_millis(250)),
                "elapsedAtLeast must parse as a humantime duration"
            );
        }
        other => panic!("expected Validate, got {other:?}"),
    }
}

#[test]
fn elapsed_at_least_on_partner_target_fails() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      partner: http://127.0.0.1:0/order
    expectation:
      count: 1
    elapsedAtLeast: 1s
"#,
    )
    .expect_err("parse must fail");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 0, "error must name the action index");
            assert!(
                message.contains("elapsedAtLeast"),
                "message must name `elapsedAtLeast`: {message}"
            );
            assert!(
                message.contains("lastReceived"),
                "message must name the only valid target kind: {message}"
            );
        }
        other => panic!("expected Validation, got {other}"),
    }
}

#[test]
fn elapsed_at_least_unparseable_fails() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      lastReceived: http://127.0.0.1:9999/hook
    expectation:
      equals: ok
    elapsedAtLeast: not-a-duration
"#,
    )
    .expect_err("parse must fail");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 0, "error must name the action index");
            assert!(
                message.contains("elapsedAtLeast") && message.contains("not-a-duration"),
                "message must name the field and the value: {message}"
            );
        }
        other => panic!("expected Validation, got {other}"),
    }
}

#[test]
fn partner_count_mixed_with_at_least_fails() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/order
      provisioning: harness
- validate:
    target:
      partner: http://127.0.0.1:0/order
    expectation:
      count: 2
      atLeast: 1
"#,
    )
    .expect_err("parse must fail");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 1, "error must name the action index");
            assert!(
                message.contains("count") && message.contains("atLeast"),
                "message must name both exclusive keys: {message}"
            );
            assert!(
                message.contains("exclusive"),
                "message must name the keys as exclusive: {message}"
            );
        }
        other => panic!("expected Validation, got {other}"),
    }
}

#[test]
fn partner_two_path_filters_fail() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/order
      provisioning: harness
- validate:
    target:
      partner: http://127.0.0.1:0/order
    expectation:
      count: 1
      path: /a
      pathContains: b
"#,
    )
    .expect_err("parse must fail");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 1, "error must name the action index");
            assert!(
                message.contains("path") && message.contains("pathContains"),
                "message must name both path keys: {message}"
            );
            assert!(
                message.contains("exclusive"),
                "message must name the path keys as exclusive: {message}"
            );
        }
        other => panic!("expected Validation, got {other}"),
    }
}

#[test]
fn partner_invalid_path_matches_fails() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/order
      provisioning: harness
- validate:
    target:
      partner: http://127.0.0.1:0/order
    expectation:
      count: 1
      pathMatches: "([unclosed"
"#,
    )
    .expect_err("parse must fail");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 1, "error must name the action index");
            assert!(
                message.contains("([unclosed"),
                "message must name the invalid pattern: {message}"
            );
            assert!(
                message.contains("regex"),
                "message must name the regex problem: {message}"
            );
        }
        other => panic!("expected Validation, got {other}"),
    }
}

#[test]
fn partner_query_non_string_value_fails() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/order
      provisioning: harness
- validate:
    target:
      partner: http://127.0.0.1:0/order
    expectation:
      atLeast: 1
      query: {a: 1}
"#,
    )
    .expect_err("parse must fail");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 1, "error must name the action index");
            assert!(
                message.contains("query") && message.contains("`a`"),
                "message must name `query` and the offending key `a`: {message}"
            );
            assert!(
                message.contains("string"),
                "message must name the string-value requirement: {message}"
            );
        }
        other => panic!("expected Validation, got {other}"),
    }
}

#[test]
fn partner_at_least_non_integer_fails() {
    for value in ["-1", "1.5"] {
        let text = format!(
            r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/order
      provisioning: harness
- validate:
    target:
      partner: http://127.0.0.1:0/order
    expectation:
      atLeast: {value}
"#
        );
        let err = parse_case(&text).expect_err("parse must fail");
        match err {
            DocError::Validation { index, message } => {
                assert_eq!(index, 1, "error must name the action index");
                assert!(
                    message.contains("atLeast"),
                    "message must name `atLeast`: {message}"
                );
                assert!(
                    message.contains("non-negative integer"),
                    "message must name the non-negative-integer requirement: {message}"
                );
            }
            other => panic!("expected Validation, got {other}"),
        }
    }
}

#[test]
fn partner_at_least_parses() {
    let doc = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/order
      provisioning: harness
- validate:
    target:
      partner: http://127.0.0.1:0/order
    expectation:
      atLeast: 3
"#,
    )
    .expect("parse must succeed");
    let action = doc.scenario.get(1).expect("two actions");
    match action {
        ScenarioAction::Validate { expectation, .. } => {
            assert_eq!(
                expectation,
                &ValidateExpectation::Partner(PartnerExpectation {
                    bound: CountBound::AtLeast(3),
                    method: None,
                    path: None,
                    query: None,
                }),
                "expectation must parse as an at-least bound"
            );
        }
        other => panic!("expected Validate, got {other:?}"),
    }
}

#[test]
fn partner_range_parses_and_inverted_fails() {
    let doc = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/order
      provisioning: harness
- validate:
    target:
      partner: http://127.0.0.1:0/order
    expectation:
      atLeast: 2
      atMost: 4
"#,
    )
    .expect("parse must succeed");
    let action = doc.scenario.get(1).expect("two actions");
    match &action {
        ScenarioAction::Validate { expectation, .. } => {
            assert_eq!(
                expectation,
                &ValidateExpectation::Partner(PartnerExpectation {
                    bound: CountBound::Range(2, 4),
                    method: None,
                    path: None,
                    query: None,
                }),
                "atLeast+atMost must combine into a range bound"
            );
        }
        other => panic!("expected Validate, got {other:?}"),
    }
    let inverted = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/order
      provisioning: harness
- validate:
    target:
      partner: http://127.0.0.1:0/order
    expectation:
      atLeast: 4
      atMost: 3
"#,
    )
    .expect_err("inverted range must fail");
    match inverted {
        DocError::Validation { index, message } => {
            assert_eq!(index, 1, "error must name the action index");
            assert!(
                message.contains("4") && message.contains("3"),
                "message must name both bound values: {message}"
            );
        }
        other => panic!("expected Validation, got {other}"),
    }
}

// -------------------------------------------------------------------------
// Inbound listener section (task 4.1, rc-5yon)
// -------------------------------------------------------------------------

#[test]
fn inbound_unknown_field_is_load_error() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
inbound:
  bindVar: INBOUND
  bogus: 1
"#,
    )
    .expect_err("parse must fail");
    let rendered = err.to_string();
    assert!(
        matches!(err, DocError::Validation { .. }),
        "expected Validation, got {err}"
    );
    assert!(
        rendered.contains("bogus"),
        "error must name the unknown field `bogus`: {rendered}"
    );
    assert!(
        rendered.contains("inbound"),
        "error must name the `inbound` section: {rendered}"
    );
}

#[cfg(not(feature = "http"))]
#[test]
fn inbound_without_feature_rejected_named() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
inbound:
  bindVar: INBOUND
"#,
    )
    .expect_err("parse must fail");
    let rendered = err.to_string();
    assert!(
        matches!(err, DocError::Validation { .. }),
        "expected Validation, got {err}"
    );
    assert!(
        rendered.contains("inbound"),
        "error must name the `inbound` section: {rendered}"
    );
    assert!(
        rendered.contains("http"),
        "error must name the `http` feature gate: {rendered}"
    );
}

/// Reserved-key symmetry (rc-5yon): the inbound listener's bindVar is
/// a harness binding, so a document `env` key of the same name is
/// rejected at load, exactly like an endpoint bindVar collision.
/// Feature-gated: without `http` the declaration itself is rejected
/// first (demand-gated activation), so the collision never surfaces.
#[cfg(feature = "http")]
#[test]
fn inbound_bindvar_env_collision_is_load_error() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
inbound:
  bindVar: INBOUND
env:
  INBOUND: "http://127.0.0.1:18080"
"#,
    )
    .expect_err("parse must fail");
    let rendered = err.to_string();
    match err {
        DocError::ReservedEnvKey { key, endpoint } => {
            assert_eq!(key, "INBOUND", "error must name the reserved key");
            assert_eq!(
                endpoint, "inbound",
                "error must name the reserving `inbound:` section"
            );
        }
        other => panic!("expected ReservedEnvKey, got {other}"),
    }
    assert!(
        rendered.contains("INBOUND"),
        "rendered error must name the reserved key: {rendered}"
    );
    assert!(
        rendered.contains("doc-validation"),
        "rendered error must name the doc-validation class: {rendered}"
    );
}

#[test]
fn inbound_section_optional() {
    let doc = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
"#,
    )
    .expect("parse must succeed");
    assert!(doc.inbound.is_none(), "no `inbound:` was declared");
}

#[test]
fn partner_query_subset_parses() {
    let doc = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/order
      provisioning: harness
- validate:
    target:
      partner: http://127.0.0.1:0/order
    expectation:
      atLeast: 1
      query: {a: "1+1", b: "2"}
"#,
    )
    .expect("parse must succeed");
    let action = doc.scenario.get(1).expect("two actions");
    match action {
        ScenarioAction::Validate { expectation, .. } => {
            assert_eq!(
                expectation,
                &ValidateExpectation::Partner(PartnerExpectation {
                    bound: CountBound::AtLeast(1),
                    method: None,
                    path: None,
                    query: Some(BTreeMap::from([
                        ("a".to_string(), "1+1".to_string()),
                        ("b".to_string(), "2".to_string()),
                    ])),
                }),
                "expectation must parse the query subset map"
            );
        }
        other => panic!("expected Validate, got {other:?}"),
    }
}
