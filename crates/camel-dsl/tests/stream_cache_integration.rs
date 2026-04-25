use camel_core::route::BuilderStep;
use camel_dsl::{
    DeclarativeRoute, DeclarativeStep,
    yaml::parse_yaml_to_declarative,
};
use camel_dsl::compile::compile_declarative_step;

fn parse_routes(yaml: &str) -> Vec<DeclarativeRoute> {
    parse_yaml_to_declarative(yaml).expect("YAML should parse")
}

#[test]
fn stream_cache_step_compiles_to_pipeline() {
    let yaml = r#"
routes:
  - id: "sc-basic"
    from: "direct:start"
    steps:
      - stream_cache: true
      - to: "log:out"
"#;
    let routes = parse_routes(yaml);
    assert_eq!(routes.len(), 1);
    assert!(matches!(&routes[0].steps[0], DeclarativeStep::StreamCache(_)));
}

#[test]
fn convert_body_to_after_stream_source() {
    let yaml = r#"
routes:
  - id: "sc-convert"
    from: "direct:start"
    steps:
      - stream_cache: true
      - convert_body_to: "text"
      - to: "log:out"
"#;
    let routes = parse_routes(yaml);
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].steps.len(), 3);
}

#[test]
fn stream_cache_with_custom_threshold() {
    let yaml = r#"
routes:
  - id: "sc-threshold"
    from: "direct:start"
    steps:
      - stream_cache: { threshold: 65536 }
      - to: "log:out"
"#;
    let routes = parse_routes(yaml);
    assert_eq!(routes.len(), 1);
    match &routes[0].steps[0] {
        DeclarativeStep::StreamCache(def) => assert_eq!(def.threshold, Some(65536)),
        other => panic!("expected StreamCache, got {:?}", other),
    }
}

#[test]
fn stream_cache_with_unmarshal() {
    let yaml = r#"
routes:
  - id: "sc-unmarshal"
    from: "direct:start"
    steps:
      - stream_cache: true
      - unmarshal: "json"
      - to: "log:out"
"#;
    let routes = parse_routes(yaml);
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].steps.len(), 3);
}

#[test]
fn stream_cache_false_rejected() {
    let yaml = r#"
routes:
  - id: "sc-false"
    from: "direct:start"
    steps:
      - stream_cache: false
"#;
    let result = parse_yaml_to_declarative(yaml);
    let err = result.expect_err("stream_cache: false should be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("stream_cache: false"),
        "error should mention stream_cache: false, got: {msg}"
    );
}

#[test]
fn stream_cache_compiles_to_processor() {
    use camel_dsl::model::StreamCacheStepDef;
    let step = DeclarativeStep::StreamCache(StreamCacheStepDef { threshold: None });
    let result = compile_declarative_step(step);
    assert!(result.is_ok());
    assert!(matches!(result.unwrap(), BuilderStep::Processor(_)));
}

#[test]
fn stream_cache_full_pipeline() {
    let yaml = r#"
routes:
  - id: "sc-full"
    from: "direct:start"
    steps:
      - stream_cache: true
      - convert_body_to: "text"
      - set_header:
          key: "processed"
          value: "true"
      - to: "log:out"
"#;
    let routes = parse_routes(yaml);
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].steps.len(), 4);
}
