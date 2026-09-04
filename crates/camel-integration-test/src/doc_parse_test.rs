//! Scenario document parser tests (ADR-0069 sections 1-2).
//!
//! Unit-test module of the lib target, declared in `src/lib.rs` under
//! `#[cfg(test)]`; `cargo test -p camel-integration-test --lib` runs
//! these tests and nothing else. Every test is path-based: it writes a
//! temporary `.test.yaml` document and parses it through
//! [`crate::parse_scenario_document`].

use crate::{
    DocError, Expectation, ScenarioAction, ScenarioDocument, ScenarioTarget,
    parse_scenario_document,
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
                &Expectation::Equals(camel_api::Value::String("ok".into())),
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
        } => {
            assert_eq!(
                target,
                &ScenarioTarget::Variable("AUTH_TOKEN".to_string()),
                "target must parse as a scenario variable"
            );
            assert_eq!(
                expectation,
                &Expectation::Regex("^Bearer .+$".to_string()),
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
