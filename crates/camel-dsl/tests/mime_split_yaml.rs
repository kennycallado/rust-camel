//! YAML parsing for the `format: mime` streaming split (Task 2.2).
//!
//! `StreamSplitConfig` gained MIME-only options (`max_parts`,
//! `max_part_size`, `content_types`) in Task 2.1; this suite pins that the
//! YAML surface forwards them and that the existing split formats keep
//! parsing unchanged.

use camel_api::StreamSplitFormat;
use camel_dsl::{DeclarativeRoute, DeclarativeStep, SplitExpressionDef, parse_yaml_to_declarative};

fn parse(doc: &str) -> DeclarativeRoute {
    let mut routes = parse_yaml_to_declarative(doc).expect("YAML must parse");
    assert_eq!(routes.len(), 1, "exactly one route");
    routes.remove(0)
}

fn stream_config(route: &DeclarativeRoute) -> camel_api::StreamSplitConfig {
    match &route.steps[0] {
        DeclarativeStep::Split(def) => match &def.expression {
            SplitExpressionDef::Stream(config) => config.clone(),
            other => panic!("expected Stream expression, got {other:?}"),
        },
        other => panic!("expected Split step, got {other:?}"),
    }
}

#[test]
fn parse_stream_split_mime_with_options() {
    let yaml = r#"
routes:
  - id: "mime-split"
    from: "direct:start"
    steps:
      - split:
          streaming: true
          stream:
            format: mime
            max_parts: 5
            content_types: [application/zip]
          steps:
            - to: "mock:out"
"#;
    let config = stream_config(&parse(yaml));
    assert_eq!(config.format, StreamSplitFormat::Mime);
    assert_eq!(config.max_parts, Some(5));
    assert_eq!(
        config.content_types,
        Some(vec!["application/zip".to_string()])
    );
}

#[test]
fn parse_stream_split_mime_defaults() {
    let yaml = r#"
routes:
  - id: "mime-split-defaults"
    from: "direct:start"
    steps:
      - split:
          streaming: true
          stream:
            format: mime
          steps:
            - to: "mock:out"
"#;
    let config = stream_config(&parse(yaml));
    assert_eq!(config.format, StreamSplitFormat::Mime);
    assert_eq!(config.max_parts, None);
    assert_eq!(config.max_part_size, None);
    assert_eq!(config.content_types, None);
}

#[test]
fn old_zip_yaml_still_parses() {
    let yaml = r#"
routes:
  - id: "zip-split"
    from: "direct:start"
    steps:
      - split:
          streaming: true
          stream:
            format: zip
          steps:
            - to: "mock:out"
"#;
    let config = stream_config(&parse(yaml));
    assert_eq!(config.format, StreamSplitFormat::Zip);
    assert_eq!(config.max_parts, None);
    assert_eq!(config.max_part_size, None);
    assert_eq!(config.content_types, None);
}

#[test]
fn mime_options_with_zip_rejected() {
    let yaml = r#"
routes:
  - id: "zip-split-with-mime-option"
    from: "direct:start"
    steps:
      - split:
          streaming: true
          stream:
            format: zip
            max_parts: 5
          steps:
            - to: "mock:out"
"#;
    let err = parse_yaml_to_declarative(yaml).expect_err("zip + max_parts must be rejected");
    assert!(
        err.to_string().contains("max_parts"),
        "error must name the offending option, got: {err}"
    );
}
