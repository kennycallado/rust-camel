//! Scenario document parser tests (ADR-0069 sections 1-2).
//!
//! Unit-test module of the lib target, declared in `src/lib.rs` under
//! `#[cfg(test)]`; `cargo test -p camel-integration-test --lib` runs
//! these tests and nothing else. Most tests are path-based: they write
//! a temporary `.test.yaml` document and parse it through
//! [`crate::parse_scenario_document`]. Matcher-grammar tests call the
//! in-crate `expectation_from_value` parser directly (shared dual
//! grammar, same verb set as the path-based readers).

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use camel_matchers::expectation_matches;
use serde_json::json;

use crate::document::{
    RowsExpectation, expectation_from_value, is_http_token, sql_expectation_from_value,
    sql_query_lacks_order_by,
};
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
fn object_form_partner_target_parses_with_bind_var() {
    let doc = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://upstream/tiles
      provisioning: harness
- validate:
    target:
      partner:
        endpoint: http://upstream/tiles
        provisioning: harness
        bindVar: upstream
    expectation:
      count: 1
"#,
    )
    .expect("parse must succeed");
    let action = doc.scenario.get(1).expect("two actions");
    match action {
        ScenarioAction::Validate { target, .. } => match target {
            ScenarioTarget::Partner(endpoint) => {
                assert_eq!(
                    endpoint.endpoint, "http://upstream/tiles",
                    "target must keep the partner endpoint reference"
                );
                assert_eq!(
                    endpoint.provisioning,
                    Some(Provisioning::Harness),
                    "object form must carry harness provisioning"
                );
                assert_eq!(
                    endpoint.bind_var.as_deref(),
                    Some("upstream"),
                    "object form must carry the bindVar"
                );
            }
            other => panic!("expected Partner, got {other:?}"),
        },
        other => panic!("expected Validate, got {other:?}"),
    }
}

#[test]
fn object_form_partner_self_declares_with_partners_entry() {
    parse_case(
        r#"
routeFiles: [routes.yaml]
partners:
  http://upstream/tiles:
  - response:
      status: 200
scenario:
- send:
    to: direct:start
- validate:
    target:
      partner:
        endpoint: http://upstream/tiles
        provisioning: harness
    expectation:
      count: 1
"#,
    )
    .expect(
        "parse must succeed: the object-form target self-declares \
         through its `partners:` entry",
    );
}

#[test]
fn bare_map_partner_without_provisioning_is_not_self_declaration() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
partners:
  http://upstream/tiles:
  - response:
      status: 200
scenario:
- send:
    to: direct:start
- validate:
    target:
      partner:
        endpoint: http://upstream/tiles
        provisioning: harness
        bindVar: upstream
    expectation:
      count: 1
- validate:
    target:
      partner:
        endpoint: http://upstream/tiles
    expectation:
      count: 1
"#,
    )
    .expect_err("parse must fail");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 2, "error must name the bare-map action index");
            assert!(
                message.contains("http://upstream/tiles"),
                "message must name the unmatched URI: {message}"
            );
        }
        other => panic!("expected Validation, got {other}"),
    }
}

#[test]
fn object_form_partner_without_partners_entry_requires_send_receive() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
- validate:
    target:
      partner:
        endpoint: http://upstream/tiles
        provisioning: harness
    expectation:
      count: 1
"#,
    )
    .expect_err("parse must fail");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 1, "error must name the action index");
            assert!(
                message.contains("http://upstream/tiles"),
                "message must name the unmatched URI: {message}"
            );
            assert!(
                message.contains("partners:"),
                "message must teach the `partners:` escape: {message}"
            );
            assert!(
                message.contains("provisioning"),
                "message must teach the `provisioning: harness` escape: {message}"
            );
            assert!(
                message.contains("send"),
                "message must teach the `send`/`receive` declaration escape: {message}"
            );
        }
        other => panic!("expected Validation, got {other}"),
    }
}

#[test]
fn non_http_partner_scheme_is_not_self_declaration() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
partners:
  https://upstream/tiles:
  - response:
      status: 200
scenario:
- send:
    to: direct:start
- validate:
    target:
      partner:
        endpoint: https://upstream/tiles
        provisioning: harness
        bindVar: upstream
    expectation:
      count: 1
"#,
    )
    .expect_err("parse must fail");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 1, "error must name the action index");
            assert!(
                message.contains("https://upstream/tiles"),
                "message must name the unmatched URI: {message}"
            );
        }
        other => panic!("expected Validation, got {other}"),
    }
}

#[test]
fn validate_partner_bind_var_reserves_env_key() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
env:
  upstream: http://anywhere
scenario:
- send:
    to:
      endpoint: http://upstream/tiles
      provisioning: harness
- validate:
    target:
      partner:
        endpoint: http://upstream/tiles
        provisioning: harness
        bindVar: upstream
    expectation:
      count: 1
"#,
    )
    .expect_err("parse must fail");
    match err {
        DocError::ReservedEnvKey { key, endpoint } => {
            assert_eq!(key, "upstream", "error must name the reserved key");
            assert_eq!(
                endpoint, "http://upstream/tiles",
                "error must name the endpoint that reserved the key"
            );
        }
        other => panic!("expected ReservedEnvKey, got {other}"),
    }
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

// -------------------------------------------------------------------------
// The `sql:` action in a build WITHOUT the `sql` feature (bd rc-25lup.1).
//
// Ordering mandate: validation (read/empty-prepare defects) runs BEFORE
// the demand gate, so document defects keep their statement-level
// diagnostics in this configuration too; only a structurally valid
// action reaches the named demand-gate error.
// -------------------------------------------------------------------------

/// A structurally valid `sql:` action is a named demand-gate error: it
/// names the `sql` feature and the rebuild instruction.
#[cfg(not(feature = "sql"))]
#[test]
fn sql_action_without_feature_is_named_error() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- sql:
    datasource: appdb
    prepare:
    - CREATE TABLE t (v TEXT)
"#,
    )
    .expect_err("parse must fail without the sql feature");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 0, "the error must name the action index");
            assert!(
                message.contains("`sql` requires the `sql` feature"),
                "the error must name the demand gate: {message}"
            );
            assert!(
                message.contains("--features sql"),
                "the error must carry the rebuild instruction: {message}"
            );
        }
        other => panic!("expected Validation, got {other:?}"),
    }
}

/// A read prepare statement fails doc-validation (statement-level),
/// not the demand gate: the document defect precedes activation.
#[cfg(not(feature = "sql"))]
#[test]
fn sql_read_rejected_without_feature() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- sql:
    datasource: appdb
    prepare:
    - (SELECT 1)
"#,
    )
    .expect_err("parse must fail");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 0, "the error must name the action index");
            assert!(
                message.contains("statement 0"),
                "the error must name the statement index: {message}"
            );
            assert!(
                !message.contains("feature"),
                "validation must fire before the demand gate: {message}"
            );
        }
        other => panic!("expected Validation, got {other:?}"),
    }
}

/// A CTE read is a read too: the same validation-before-demand-gate
/// ordering without the feature.
#[cfg(not(feature = "sql"))]
#[test]
fn sql_with_rejected_without_feature() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- sql:
    datasource: appdb
    prepare:
    - with cte as (select 1) select * from cte
"#,
    )
    .expect_err("parse must fail");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 0, "the error must name the action index");
            assert!(
                message.contains("statement 0"),
                "the error must name the statement index: {message}"
            );
            assert!(
                !message.contains("feature"),
                "validation must fire before the demand gate: {message}"
            );
        }
        other => panic!("expected Validation, got {other:?}"),
    }
}

/// An empty prepare list names the action index before the demand
/// gate fires.
#[cfg(not(feature = "sql"))]
#[test]
fn sql_empty_prepare_rejected_without_feature() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- sql:
    datasource: appdb
    prepare: []
"#,
    )
    .expect_err("parse must fail");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 0, "the error must name the action index");
            assert!(
                message.contains("prepare list must not be empty"),
                "the error must name the empty prepare list: {message}"
            );
            assert!(
                !message.contains("feature"),
                "validation must fire before the demand gate: {message}"
            );
        }
        other => panic!("expected Validation, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// `ignore` verb (Task 2.1, sql-validate-target): the shared dual grammar
// gains the wildcard verb, mirroring the `exists` no-argument law.
// ---------------------------------------------------------------------------

/// `{ignore: null}` parses to `Expectation::Any`: the wildcard verb,
/// the Citrus `@ignore@` equivalent (shared dual grammar, same verb
/// set as `exists`).
#[test]
fn ignore_verb_parses_to_any() {
    let expectation = expectation_from_value(&json!({ "ignore": null }), 0, "expectation")
        .expect("ignore with a null payload parses");
    assert!(
        matches!(expectation, Expectation::Any),
        "expected Expectation::Any, got {expectation:?}"
    );
}

/// A non-null payload is a grammar error mirroring the `exists` law:
/// the wildcard takes no argument.
#[test]
fn ignore_takes_no_argument() {
    let err = expectation_from_value(&json!({ "ignore": 5 }), 0, "expectation")
        .expect_err("ignore with a payload must fail");
    let display = err.to_string();
    assert!(
        display.contains("`ignore` takes no argument"),
        "error must name the no-argument law: {display}"
    );
}

/// Pins the verb's runtime meaning at the grammar layer: `Any` holds
/// for every value including null.
#[test]
fn ignore_matches_any_cell() {
    for value in [json!(null), json!(3), json!("x")] {
        assert!(
            expectation_matches(&Expectation::Any, &value),
            "Any must match {value:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// sql validate target (Task 2.2, sql-validate-target): the `sql` key,
// the sql expectation grammar, and the ORDER-BY nondeterminism advisory.
// ---------------------------------------------------------------------------

/// A validate action whose `sql` target parses keeps the datasource
/// name and the doc-authored read query, and pairs the target with the
/// row-shape grammar (`ValidateExpectation::Rows`).
#[test]
fn sql_target_parses() {
    let doc = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      sql:
        datasource: appdb
        query: SELECT id FROM t ORDER BY id
    expectation:
      rows: [[1]]
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
                ScenarioTarget::Sql(target) => {
                    assert_eq!(
                        target.datasource, "appdb",
                        "target must keep the datasource identifier"
                    );
                    assert_eq!(
                        target.query, "SELECT id FROM t ORDER BY id",
                        "target must keep the doc-authored read query"
                    );
                }
                other => panic!("expected Sql, got {other:?}"),
            }
            assert_eq!(
                expectation,
                &ValidateExpectation::Rows(RowsExpectation {
                    columns: None,
                    unordered: false,
                    rows: Some(vec![vec![Expectation::Equals(json!(1))]]),
                    bound: None,
                }),
                "expectation must parse as the sql row shape"
            );
        }
        other => panic!("expected Validate, got {other:?}"),
    }
}

/// A mutation query is a doc-validation error naming the action index
/// and the read rule: the validate sql target owns reads, the `sql:`
/// prepare action owns mutations.
#[test]
fn mutation_query_is_load_error() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      sql:
        datasource: appdb
        query: INSERT INTO t VALUES (1)
    expectation:
      count: 1
"#,
    )
    .expect_err("parse must fail");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 0, "error must name the action index");
            assert!(
                message.contains("reads belong to the validate sql target"),
                "error must state the read rule: {message}"
            );
        }
        other => panic!("expected Validation, got {other:?}"),
    }
}

/// A CTE read parses: the same prefix law the prepare-side lint uses.
/// The expectation stays a count bound so this test emits no ORDER-BY
/// advisory (the advisory owns its own windowed tests).
#[test]
fn with_prefix_query_accepts() {
    let doc = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      sql:
        datasource: appdb
        query: with c as (select 1) select * from c
    expectation:
      count: 1
"#,
    )
    .expect("parse must succeed");
    match doc.scenario.first().expect("one action") {
        ScenarioAction::Validate {
            target: ScenarioTarget::Sql(target),
            ..
        } => {
            assert_eq!(
                target.query, "with c as (select 1) select * from c",
                "the CTE read must parse verbatim"
            );
        }
        other => panic!("expected Validate with Sql target, got {other:?}"),
    }
}

/// A parenthesis-wrapped read parses: `is_read_statement` re-trims
/// after each dropped `(` (expectation stays a count bound — no
/// advisory).
#[test]
fn paren_wrapped_select_accepts() {
    let doc = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      sql:
        datasource: appdb
        query: (select 1)
    expectation:
      count: 1
"#,
    )
    .expect("parse must succeed");
    match doc.scenario.first().expect("one action") {
        ScenarioAction::Validate {
            target: ScenarioTarget::Sql(target),
            ..
        } => {
            assert_eq!(target.query, "(select 1)", "the wrapped read must parse");
        }
        other => panic!("expected Validate with Sql target, got {other:?}"),
    }
}

/// An unknown field inside the `sql` payload is rejected by serde's
/// deny-unknown rule, naming the offending key.
#[test]
fn sql_target_unknown_field_rejected() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      sql:
        datasource: appdb
        query: select 1
        extra: true
    expectation:
      count: 1
"#,
    )
    .expect_err("parse must fail");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 0, "error must name the action index");
            assert!(
                message.contains("unknown field") && message.contains("extra"),
                "serde deny-unknown error must name `extra`: {message}"
            );
        }
        other => panic!("expected Validation, got {other:?}"),
    }
}

/// Empty `datasource` and empty `query` are each doc-validation
/// errors: an identifier law with an empty name asserts nothing.
#[test]
fn empty_datasource_or_query_rejected() {
    let cases = [
        (
            "datasource",
            r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      sql:
        datasource: ""
        query: "select 1"
    expectation:
      count: 1
"#,
        ),
        (
            "query",
            r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      sql:
        datasource: "appdb"
        query: ""
    expectation:
      count: 1
"#,
        ),
    ];
    for (field, text) in cases {
        let err = parse_case(text).expect_err("parse must fail");
        match err {
            DocError::Validation { index, message } => {
                assert_eq!(index, 0, "error must name the action index");
                assert!(
                    message.contains(&format!("non-empty `{field}`")),
                    "error must name the empty `{field}`: {message}"
                );
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }
}

/// The poll deadline is valid on a sql target: the read runs against a
/// live datasource and may be worth bounding.
#[test]
fn deadline_on_sql_target_accepts() {
    let doc = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      sql:
        datasource: appdb
        query: select 1
    expectation:
      count: 1
    deadline: 2s
"#,
    )
    .expect("parse must succeed");
    match doc.scenario.first().expect("one action") {
        ScenarioAction::Validate { deadline, .. } => {
            assert_eq!(
                *deadline,
                Some(Duration::from_secs(2)),
                "deadline must parse as a humantime duration"
            );
        }
        other => panic!("expected Validate, got {other:?}"),
    }
}

/// Existing behavior stands: `deadline` on a `lastReceived` target is
/// still rejected, and the error now names both valid targets.
#[test]
fn deadline_on_last_received_still_rejected() {
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
                message.contains("`partner` or `sql`"),
                "message must name both valid targets: {message}"
            );
        }
        other => panic!("expected Validation, got {other:?}"),
    }
}

/// `elapsedAtLeast` stays `lastReceived`-only: a sql read carries no
/// wire arrival to measure.
#[test]
fn elapsed_at_least_on_sql_rejected() {
    let err = parse_case(
        r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      sql:
        datasource: appdb
        query: select 1
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
        other => panic!("expected Validation, got {other:?}"),
    }
}

/// Pins the ORDER-BY predicate: case-insensitive, `\s+` between the
/// words (newlines included), and the documented string-literal false
/// positive stays true (advisory-only).
#[test]
fn order_by_predicate() {
    assert!(sql_query_lacks_order_by("SELECT * FROM t"));
    assert!(!sql_query_lacks_order_by("select * from t order by id"));
    assert!(
        !sql_query_lacks_order_by("SELECT * FROM t ORDER\nBY id"),
        "`\\s+` must cover the newline between the words"
    );
    assert!(
        sql_query_lacks_order_by("select 'totally ordered by intent' from t"),
        "documented false positive: a string literal trips the token search"
    );
}

/// The `unordered` flag carries through to `RowsExpectation.unordered`.
#[test]
fn unordered_flag_parses() {
    let rows = sql_expectation_from_value(&json!({ "unordered": true, "rows": [[1]] }), 0)
        .expect("unordered flag parses");
    assert!(rows.unordered, "unordered must carry through");
    assert!(rows.rows.is_some(), "the rows shape stays populated");
    assert_eq!(rows.bound, None, "the bound stays unset in the rows shape");
}

/// Row cells parse through the shared matcher grammar: literals, verb
/// maps, and the `ignore`/`equals` null verbs alike.
#[test]
fn sql_expectation_rows_cells() {
    let rows = sql_expectation_from_value(
        &json!({
            "rows": [
                [1, "a"],
                [2, {"contains": "b"}],
                [{"ignore": null}, {"equals": null}]
            ],
            "columns": ["id", "name"]
        }),
        0,
    )
    .expect("rows shape parses");
    assert_eq!(
        rows.rows.as_ref().map(Vec::len),
        Some(3),
        "three row patterns must parse: {:?}",
        rows.rows
    );
    assert_eq!(
        rows.columns,
        Some(vec!["id".to_string(), "name".to_string()]),
        "the column projection must carry the declared names"
    );
    assert_eq!(rows.bound, None, "the rows shape leaves the bound unset");
    let parsed = rows.rows.expect("rows populated");
    assert_eq!(
        parsed[1][1],
        Expectation::Contains("b".to_string()),
        "verb-map cells parse through the matcher grammar"
    );
    assert_eq!(
        parsed[2][0],
        Expectation::Any,
        "the ignore verb parses to the wildcard"
    );
}

/// The four count-key forms parse to their `CountBound` shapes with
/// the partner semantics.
#[test]
fn sql_expectation_count_bound_forms() {
    let bound = |value: serde_json::Value| {
        sql_expectation_from_value(&value, 0)
            .expect("count shape parses")
            .bound
    };
    assert_eq!(bound(json!({ "count": 3 })), Some(CountBound::Exact(3)));
    assert_eq!(bound(json!({ "atLeast": 2 })), Some(CountBound::AtLeast(2)));
    assert_eq!(bound(json!({ "atMost": 5 })), Some(CountBound::AtMost(5)));
    assert_eq!(
        bound(json!({ "atLeast": 1, "atMost": 3 })),
        Some(CountBound::Range(1, 3))
    );
}

/// `rows` and any count key are exclusive: the error names both keys.
#[test]
fn sql_expectation_rows_and_bound_exclusive() {
    let err = sql_expectation_from_value(&json!({ "rows": [[1]], "count": 1 }), 0)
        .expect_err("rows and a bound cannot coexist");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 0, "error must name the action index");
            assert!(
                message.contains("`rows`") && message.contains("`count`"),
                "error must name both keys: {message}"
            );
        }
        other => panic!("expected Validation, got {other:?}"),
    }
}

/// An empty `rows` list asserts nothing and is rejected.
#[test]
fn sql_expectation_empty_rows_rejected() {
    let err =
        sql_expectation_from_value(&json!({ "rows": [] }), 0).expect_err("empty rows must fail");
    let display = err.to_string();
    assert!(
        display.contains("`rows` must not be empty"),
        "error must name the empty rows list: {display}"
    );
}

/// A row whose width differs from the declared `columns` fails naming
/// the action index AND the row index.
#[test]
fn sql_expectation_row_length_names_row_index() {
    let err = sql_expectation_from_value(
        &json!({ "columns": ["id", "name"], "rows": [[1, "a"], [1, "a", "extra"]] }),
        0,
    )
    .expect_err("width mismatch must fail");
    match err {
        DocError::Validation { index, message } => {
            assert_eq!(index, 0, "error must name the action index");
            assert!(
                message.contains("row 1"),
                "error must name the offending row index: {message}"
            );
        }
        other => panic!("expected Validation, got {other:?}"),
    }
}

/// An unknown sql-expectation field fails listing the recognized keys.
#[test]
fn sql_expectation_unknown_field_rejected() {
    let err = sql_expectation_from_value(&json!({ "rows": [[1]], "foo": 1 }), 0)
        .expect_err("unknown field must fail");
    let display = err.to_string();
    assert!(
        display.contains("unknown field `foo`"),
        "error must name the unknown field: {display}"
    );
    for key in ["rows", "columns", "unordered", "count", "atLeast", "atMost"] {
        assert!(
            display.contains(&format!("`{key}`")),
            "error must list the recognized key `{key}`: {display}"
        );
    }
}

/// `columns` must be a non-empty list of strings.
#[test]
fn sql_expectation_columns_must_be_nonempty_strings() {
    let cases = [
        json!({ "columns": [], "rows": [[1]] }),
        json!({ "columns": [1], "rows": [[1]] }),
    ];
    for case in cases {
        let err =
            sql_expectation_from_value(&case, 0).expect_err("columns must be non-empty strings");
        let display = err.to_string();
        assert!(
            display.contains("`columns`"),
            "error must name `columns`: {display}"
        );
    }
}

/// Duplicate column names are rejected naming the duplicated column.
#[test]
fn sql_expectation_columns_duplicate_rejected() {
    let err = sql_expectation_from_value(&json!({ "columns": ["id", "id"], "rows": [[1, 2]] }), 0)
        .expect_err("duplicate columns must fail");
    let display = err.to_string();
    assert!(
        display.contains("duplicate column name `id`"),
        "error must name the duplicated column: {display}"
    );
}

/// Serializes the two advisory tests' window sections: a window is
/// open to every event in its interval, so a parallel sibling's
/// advisory would land in the window under count.
static ADVISORY_LOCK: Mutex<()> = Mutex::new(());

/// An ordered `rows` assertion over a query without `ORDER BY` emits
/// exactly one advisory warn naming the action index and the
/// nondeterminism.
///
/// The parse runs under a scoped capture dispatch: the process-global
/// subscriber seat is first-wins, and another module's test (a boot
/// path) may own it by the time this runs — the scoped dispatch makes
/// the capture deterministic whatever the seat's state.
#[test]
fn ordered_without_order_by_warns_once() {
    let _guard = ADVISORY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let dispatch = crate::log_capture::scoped_capture_dispatch();
    let window = crate::log_capture::open_window();
    let _doc = tracing::dispatcher::with_default(&dispatch, || {
        parse_case(
            r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      sql:
        datasource: appdb
        query: SELECT id FROM t
    expectation:
      rows: [[1]]
"#,
        )
        .expect("parse must succeed")
    });
    let events = window.close();
    let advisories: Vec<_> = events
        .iter()
        .filter(|event| event.level == tracing::Level::WARN && event.message.contains("ORDER BY"))
        .collect();
    assert_eq!(
        advisories.len(),
        1,
        "exactly one advisory warn must fire: {events:?}"
    );
    assert!(
        advisories[0].message.contains("action 0"),
        "the advisory names the action index: {}",
        advisories[0].message
    );
    assert!(
        advisories[0].message.contains("nondeterministic"),
        "the advisory states the nondeterminism: {}",
        advisories[0].message
    );
}

/// An `unordered` shape never advises: the flag is the documented fix.
/// Same scoped-dispatch capture as the ordered twin.
#[test]
fn unordered_without_order_by_does_not_warn() {
    let _guard = ADVISORY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let dispatch = crate::log_capture::scoped_capture_dispatch();
    let window = crate::log_capture::open_window();
    let _doc = tracing::dispatcher::with_default(&dispatch, || {
        parse_case(
            r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      sql:
        datasource: appdb
        query: SELECT id FROM t
    expectation:
      unordered: true
      rows: [[1]]
"#,
        )
        .expect("parse must succeed")
    });
    let events = window.close();
    assert!(
        !events
            .iter()
            .any(|event| event.level == tracing::Level::WARN && event.message.contains("ORDER BY")),
        "an unordered shape never advises: {events:?}"
    );
}
