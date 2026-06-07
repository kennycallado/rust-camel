use camel_dsl::model::DeclarativeStep;

fn enrich_yaml(step_yaml: &str) -> String {
    format!(
        r#"
routes:
  - id: test
    from: "timer:tick?period=1000"
    steps:
      - {step_yaml}
"#
    )
}

#[test]
fn yaml_parses_enrich_step_with_kind() {
    let yaml = enrich_yaml(r#"enrich: "file:/tmp/data.json""#);
    let result = camel_dsl::yaml::parse_yaml_to_canonical(&yaml);
    // Enrich is not supported in canonical v1, but we still test that
    // the YAML is parsed into the correct DeclarativeStepKind.
    // The canonical error tells us parsing succeeded but compilation failed.
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("enrich") || err.contains("not supported"),
        "Expected canonical error mentioning enrich, got: {err}"
    );
}

#[test]
fn yaml_parses_poll_enrich_step_with_kind() {
    let yaml = enrich_yaml(r#"pollEnrich: "file:/tmp/inbox?timeout=5000""#);
    let result = camel_dsl::yaml::parse_yaml_to_canonical(&yaml);
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("poll_enrich") || err.contains("not supported"),
        "Expected canonical error mentioning poll_enrich, got: {err}"
    );
}

#[test]
fn yaml_parses_enrich_to_declarative_kind() {
    let yaml = enrich_yaml(r#"enrich: "file:/tmp/data.json""#);
    let routes = camel_dsl::yaml::parse_yaml_to_declarative(&yaml).unwrap();
    assert_eq!(routes.len(), 1);
    let route = &routes[0];
    assert_eq!(route.steps.len(), 1);
    // Verify the step is Enrich variant
    match &route.steps[0] {
        DeclarativeStep::Enrich(def) => {
            assert_eq!(def.uri, "file:/tmp/data.json");
            assert_eq!(def.strategy, None);
            assert_eq!(def.timeout_ms, None);
        }
        other => panic!("expected Enrich step, got {other:?}"),
    }
}

#[test]
fn yaml_parses_poll_enrich_to_declarative_kind() {
    let yaml = enrich_yaml(r#"pollEnrich: "file:/tmp/inbox?timeout=5000""#);
    let routes = camel_dsl::yaml::parse_yaml_to_declarative(&yaml).unwrap();
    assert_eq!(routes.len(), 1);
    let route = &routes[0];
    assert_eq!(route.steps.len(), 1);
    match &route.steps[0] {
        DeclarativeStep::PollEnrich(def) => {
            assert_eq!(def.uri, "file:/tmp/inbox?timeout=5000");
            assert_eq!(def.strategy, None);
            assert_eq!(def.timeout_ms, None);
        }
        other => panic!("expected PollEnrich step, got {other:?}"),
    }
}

#[test]
fn yaml_parses_enrich_with_strategy_and_timeout() {
    let yaml = r#"
routes:
  - id: test
    from: "timer:tick"
    steps:
      - enrich:
          uri: "file:/tmp/data.json"
          strategy: "myStrategy"
"#;
    let routes = camel_dsl::yaml::parse_yaml_to_declarative(yaml).unwrap();
    assert_eq!(routes.len(), 1);
    let route = &routes[0];
    assert_eq!(route.steps.len(), 1);
    match &route.steps[0] {
        DeclarativeStep::Enrich(def) => {
            assert_eq!(def.uri, "file:/tmp/data.json");
            assert_eq!(def.strategy.as_deref(), Some("myStrategy"));
            assert_eq!(def.timeout_ms, None);
        }
        other => panic!("expected Enrich step, got {other:?}"),
    }
}

#[test]
fn yaml_parses_poll_enrich_verbose_form() {
    let yaml = r#"
routes:
  - id: test
    from: "timer:tick"
    steps:
      - pollEnrich:
          uri: "file:/tmp/inbox"
          timeout: 5000
"#;
    let routes = camel_dsl::yaml::parse_yaml_to_declarative(yaml).unwrap();
    assert_eq!(routes.len(), 1);
    let route = &routes[0];
    assert_eq!(route.steps.len(), 1);
    match &route.steps[0] {
        DeclarativeStep::PollEnrich(def) => {
            assert_eq!(def.uri, "file:/tmp/inbox");
            assert_eq!(def.strategy, None);
            assert_eq!(def.timeout_ms, Some(5000));
        }
        other => panic!("expected PollEnrich step, got {other:?}"),
    }
}

#[test]
fn enrich_empty_uri_fails() {
    let yaml = enrich_yaml(r#"enrich: """#);
    let result = camel_dsl::yaml::parse_yaml_to_declarative(&yaml);
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("URI must not be empty"),
        "Expected empty URI error, got: {err}"
    );
}

#[test]
fn poll_enrich_empty_uri_fails() {
    let yaml = enrich_yaml(r#"pollEnrich: """#);
    let result = camel_dsl::yaml::parse_yaml_to_declarative(&yaml);
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("URI must not be empty"),
        "Expected empty URI error, got: {err}"
    );
}
