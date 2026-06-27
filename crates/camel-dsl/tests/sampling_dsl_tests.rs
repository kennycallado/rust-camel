//! DSL round-trip tests for the Sampling EIP step.
//!
//! Verifies that a YAML route with a `sampling:` step can be parsed,
//! compiled, and produces the expected `BuilderStep::Sampling`.

use camel_core::BuilderStep;

fn parse_yaml(yaml: &str) -> Vec<camel_core::RouteDefinition> {
    camel_dsl::parse_yaml(yaml).expect("YAML should parse successfully")
}

#[test]
fn sampling_yaml_short_form_compiles() {
    let yaml = r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - sampling: 7
"#;
    let routes = parse_yaml(yaml);
    assert_eq!(routes.len(), 1);
    let steps = routes[0].steps();
    assert_eq!(steps.len(), 1);
    match &steps[0] {
        BuilderStep::Sampling { period } => {
            assert_eq!(*period, 7, "sampling period should be 7");
        }
        other => panic!("expected BuilderStep::Sampling, got {other:?}"),
    }
}

#[test]
fn sampling_yaml_full_form_compiles() {
    let yaml = r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - sampling:
          period: 3
"#;
    let routes = parse_yaml(yaml);
    assert_eq!(routes.len(), 1);
    let steps = routes[0].steps();
    assert_eq!(steps.len(), 1);
    match &steps[0] {
        BuilderStep::Sampling { period } => {
            assert_eq!(*period, 3, "sampling period should be 3");
        }
        other => panic!("expected BuilderStep::Sampling, got {other:?}"),
    }
}

#[test]
fn sampling_period_zero_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - sampling: 0
"#;
    match camel_dsl::parse_yaml(yaml) {
        Ok(_) => panic!("sampling with period 0 should be rejected"),
        Err(err) => {
            let msg = err.to_string();
            assert!(
                msg.contains("period must be > 0"),
                "error should mention period > 0: {msg}"
            );
        }
    }
}

#[test]
fn sampling_declarative_rejected_in_canonical() {
    let yaml = r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - sampling: 5
"#;
    match camel_dsl::parse_yaml_to_canonical(yaml, false) {
        Ok(_) => panic!("canonical v1 should reject sampling"),
        Err(err) => {
            let msg = err.to_string();
            assert!(
                msg.contains("canonical v1 does not support step `sampling`"),
                "error should mention canonical rejection: {msg}"
            );
        }
    }
}
