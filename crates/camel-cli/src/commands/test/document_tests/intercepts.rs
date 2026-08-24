use super::*;

#[test]
fn intercepts_skip_to_parses() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
intercepts:
  kafka:orders:
    skipTo: mock:orders
"#;
    let doc = parse_test_document(yaml).expect("valid intercept doc should parse"); // allow-unwrap
    let rules = doc.intercept_rules().expect("intercept_rules is Some"); // allow-unwrap
    assert_eq!(
        rules.lookup("kafka:orders"),
        Some(&camel_core::intercept::InterceptAction::SkipTo {
            uri: "mock:orders".to_string()
        })
    );
}

#[test]
fn intercepts_divert_copy_to_parses() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
intercepts:
  seda:audit:
    divertCopyTo: mock:audit
"#;
    let doc = parse_test_document(yaml).expect("valid intercept doc should parse"); // allow-unwrap
    let rules = doc.intercept_rules().expect("intercept_rules is Some"); // allow-unwrap
    assert_eq!(
        rules.lookup("seda:audit"),
        Some(&camel_core::intercept::InterceptAction::DivertCopyTo {
            uri: "mock:audit".to_string()
        })
    );
}

#[test]
fn intercept_action_both_keys_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
intercepts:
  kafka:orders:
    skipTo: mock:orders
    divertCopyTo: mock:audit
"#;
    let err = err_of(yaml);
    assert!(
        matches!(
            err,
            TestDocError::InterceptActionKeys { ref key, problem: "both" } if key == "kafka:orders"
        ),
        "expected InterceptActionKeys both for kafka:orders, got: {err:?}"
    );
    assert!(err.to_string().contains("kafka:orders"));
}

#[test]
fn intercept_action_neither_key_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
intercepts:
  kafka:orders: {}
"#;
    let err = err_of(yaml);
    assert!(
        matches!(
            err,
            TestDocError::InterceptActionKeys { ref key, problem: "neither" } if key == "kafka:orders"
        ),
        "expected InterceptActionKeys neither for kafka:orders, got: {err:?}"
    );
    assert!(err.to_string().contains("kafka:orders"));
}

#[test]
fn intercept_target_non_mock_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
intercepts:
  kafka:orders:
    skipTo: direct:orders
"#;
    let err = err_of(yaml);
    let msg = err.to_string();
    assert!(
        matches!(err, TestDocError::InterceptInvalid(_)),
        "expected InterceptInvalid, got: {err:?}"
    );
    assert!(
        msg.contains("must start with 'mock:'"),
        "expected Stage A fragment, got: {msg}"
    );
    assert!(
        msg.contains("direct:orders"),
        "expected target in msg, got: {msg}"
    );
}

#[test]
fn intercept_source_mock_scheme_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
intercepts:
  mock:a:
    skipTo: mock:b
"#;
    let err = err_of(yaml);
    assert!(
        matches!(err, TestDocError::InterceptMockSource { ref key } if key == "mock:a"),
        "expected InterceptMockSource for mock:a, got: {err:?}"
    );
    assert!(err.to_string().contains("mock:a"));
}

#[test]
fn intercept_source_empty_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
intercepts:
  "":
    skipTo: mock:orders
"#;
    let err = err_of(yaml);
    assert!(
        matches!(err, TestDocError::InterceptEmptySource),
        "expected InterceptEmptySource, got: {err:?}"
    );
}

#[test]
fn intercept_target_empty_path_rejected() {
    for target in ["mock:", "mock:?x=1"] {
        let yaml = format!(
            r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
intercepts:
  kafka:orders:
    skipTo: "{target}"
"#
        );
        let err = err_of(&yaml);
        assert!(
            matches!(err, TestDocError::InterceptEmptyTargetPath { ref key } if key == "kafka:orders"),
            "target `{target}` should be InterceptEmptyTargetPath, got: {err:?}"
        );
        assert!(
            err.to_string().contains("kafka:orders"),
            "display must name source key, got: {err}"
        );
    }
}

#[test]
fn intercept_action_unknown_field_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
intercepts:
  kafka:orders:
    replaceWith: mock:x
"#;
    let err = err_of(yaml);
    assert!(
        matches!(err, TestDocError::UnknownField(ref msg) if msg.contains("replaceWith")),
        "expected UnknownField naming replaceWith, got: {err:?}"
    );
}

#[test]
fn intercepts_absent_keeps_behavior() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("doc without intercepts should parse"); // allow-unwrap
    assert!(doc.intercept_rules().is_none());
    assert!(doc.intercepts.is_none());
}
