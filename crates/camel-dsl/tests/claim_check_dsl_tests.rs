use camel_dsl::model::DeclarativeStep;
use camel_dsl::yaml;

fn claim_check_yaml(step_yaml: &str) -> String {
    format!(
        r#"
routes:
  - id: claim-check-test
    from: direct:in
    steps:
      - {step_yaml}
"#
    )
}

#[test]
fn claim_check_yaml_parses_and_lowers() {
    let yaml = claim_check_yaml(
        r#"claim_check:
            repository: memory
            operation: set
            key: "${header.claimKey}""#,
    );
    let routes = yaml::parse_yaml_to_declarative(&yaml).unwrap();
    assert_eq!(routes.len(), 1);
    let route = &routes[0];
    assert_eq!(route.steps.len(), 1);
    match &route.steps[0] {
        DeclarativeStep::ClaimCheck(def) => {
            assert_eq!(def.repository, "memory");
            assert_eq!(def.operation, "set");
            assert_eq!(def.key.language, "simple");
            assert_eq!(def.key.source, "${header.claimKey}");
        }
        other => panic!("expected ClaimCheck step, got {other:?}"),
    }
}

#[test]
fn claim_check_yaml_parses_all_operations() {
    for op in &["set", "get", "get_and_remove", "push", "pop"] {
        let yaml = claim_check_yaml(&format!(
            r#"claim_check:
                repository: memory
                operation: {op}
                key: mykey"#
        ));
        let routes = yaml::parse_yaml_to_declarative(&yaml).unwrap();
        assert_eq!(routes.len(), 1);
        let route = &routes[0];
        assert_eq!(route.steps.len(), 1);
        match &route.steps[0] {
            DeclarativeStep::ClaimCheck(def) => {
                assert_eq!(def.operation, *op);
            }
            other => panic!("expected ClaimCheck step, got {other:?}"),
        }
    }
}

#[test]
fn claim_check_empty_repository_rejected() {
    let yaml = claim_check_yaml(
        r#"claim_check:
            repository: ""
            operation: set
            key: mykey"#,
    );
    let err = yaml::parse_yaml_to_declarative(&yaml).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("repository is required"),
        "expected validation error about repository, got: {msg}"
    );
}

#[test]
fn claim_check_empty_operation_rejected() {
    let yaml = claim_check_yaml(
        r#"claim_check:
            repository: memory
            operation: ""
            key: mykey"#,
    );
    let err = yaml::parse_yaml_to_declarative(&yaml).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("operation is required"),
        "expected validation error about operation, got: {msg}"
    );
}

#[test]
fn claim_check_empty_key_rejected() {
    let yaml = claim_check_yaml(
        r#"claim_check:
            repository: memory
            operation: set
            key: "" "#,
    );
    let err = yaml::parse_yaml_to_declarative(&yaml).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("key is required"),
        "expected validation error about key, got: {msg}"
    );
}

#[test]
fn claim_check_unknown_operation_rejected() {
    let yaml = claim_check_yaml(
        r#"claim_check:
            repository: memory
            operation: bogus
            key: mykey"#,
    );
    let err = yaml::parse_yaml_to_declarative(&yaml).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("unknown operation"),
        "expected validation error about unknown operation, got: {msg}"
    );
}

#[test]
fn claim_check_filter_accepted_and_stored() {
    let yaml = claim_check_yaml(
        r#"claim_check:
            repository: memory
            operation: set
            key: "${header.claimKey}"
            filter: "body,header:foo*""#,
    );
    let routes = yaml::parse_yaml_to_declarative(&yaml).unwrap();
    assert_eq!(routes.len(), 1);
    let route = &routes[0];
    assert_eq!(route.steps.len(), 1);
    match &route.steps[0] {
        DeclarativeStep::ClaimCheck(def) => {
            assert_eq!(def.filter.as_deref(), Some("body,header:foo*"));
        }
        other => panic!("expected ClaimCheck step, got {other:?}"),
    }
}
