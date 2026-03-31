//! Tests that YAML routes with on_complete / on_failure fields parse correctly
//! and that routes without UoW config are unaffected.

use camel_dsl::parse_yaml;

#[test]
fn yaml_route_with_on_complete_and_on_failure_parses_correctly() {
    let yaml = r#"
routes:
  - id: uow-route
    from: "timer:tick"
    on_complete: "log:complete"
    on_failure: "log:failed"
    steps:
      - log: "hello"
"#;
    let routes = parse_yaml(yaml).expect("YAML should parse");
    assert_eq!(routes.len(), 1);
    let route = &routes[0];
    let uow = route.unit_of_work_config().expect("should have UoW config");
    assert_eq!(uow.on_complete.as_deref(), Some("log:complete"));
    assert_eq!(uow.on_failure.as_deref(), Some("log:failed"));
}

#[test]
fn yaml_route_with_only_on_complete_parses_correctly() {
    let yaml = r#"
routes:
  - id: uow-partial
    from: "timer:tick"
    on_complete: "log:done"
    steps: []
"#;
    let routes = parse_yaml(yaml).expect("YAML should parse");
    let uow = routes[0]
        .unit_of_work_config()
        .expect("should have UoW config");
    assert_eq!(uow.on_complete.as_deref(), Some("log:done"));
    assert!(uow.on_failure.is_none());
}

#[test]
fn yaml_route_without_uow_has_no_uow_config() {
    let yaml = r#"
routes:
  - id: plain-route
    from: "timer:tick"
    steps: []
"#;
    let routes = parse_yaml(yaml).expect("YAML should parse");
    assert!(routes[0].unit_of_work_config().is_none());
}
