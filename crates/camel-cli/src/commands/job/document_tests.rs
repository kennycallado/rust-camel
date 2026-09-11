//! Unit tests for the `execute:` job document model.

use std::path::Path;

use crate::commands::test::document::TestDocError;

use super::document::{self, JobDocError};

/// A `.test.yaml` path inside a tempdir-free constant (the parser only
/// inspects the suffix).
fn doc_path() -> std::path::PathBuf {
    Path::new("fixtures/job.test.yaml").to_path_buf()
}

const VALID_ONE_SHOT: &str = r#"
execute:
  mode: one-shot
  timeout: 30s
  capture-reply: true
  send:
    to: direct:transform
    body: "ping"
    headers:
      X-Job: cli
routeFiles:
  - routes/job-route.yaml
"#;

#[test]
fn valid_one_shot_parses() {
    let doc = document::parse_job_document(&doc_path(), VALID_ONE_SHOT).expect("parses");
    assert_eq!(doc.execute.mode, "one-shot");
    assert_eq!(doc.execute.timeout.as_secs(), 30);
    assert!(doc.execute.capture_reply);
    assert_eq!(doc.execute.send.to, "direct:transform");
    assert_eq!(
        doc.execute.send.body.as_ref().map(|b| match b {
            document::JobBody::Text(s) => s.clone(),
            document::JobBody::Json(v) => v.to_string(),
        }),
        Some("ping".to_string())
    );
    assert_eq!(
        doc.execute
            .send
            .headers
            .as_ref()
            .and_then(|h| h.get("X-Job")),
        Some(&serde_json::json!("cli"))
    );
    assert_eq!(
        doc.route_files,
        Some(vec!["routes/job-route.yaml".to_string()])
    );
}

#[test]
fn missing_timeout_is_rejected() {
    let text = r#"
execute:
  mode: one-shot
  send:
    to: direct:transform
routes:
  - id: r
    from: direct:transform
"#;
    let err = document::parse_job_document(&doc_path(), text).unwrap_err();
    assert!(matches!(err, JobDocError::MissingTimeout), "got {err:?}");
}

#[test]
fn non_positive_timeout_is_rejected() {
    let text = r#"
execute:
  mode: one-shot
  timeout: 0s
  send:
    to: direct:transform
routes:
  - id: r
    from: direct:transform
"#;
    let err = document::parse_job_document(&doc_path(), text).unwrap_err();
    assert!(matches!(err, JobDocError::InvalidTimeout(_)), "got {err:?}");
}

#[test]
fn batch_mode_is_reserved_and_rejected() {
    let text = VALID_ONE_SHOT.replace("one-shot", "batch");
    let err = document::parse_job_document(&doc_path(), &text).unwrap_err();
    assert!(matches!(err, JobDocError::BatchReserved), "got {err:?}");
}

#[test]
fn garbage_mode_is_rejected() {
    let text = VALID_ONE_SHOT.replace("one-shot", "sometimes");
    let err = document::parse_job_document(&doc_path(), &text).unwrap_err();
    match err {
        JobDocError::UnsupportedMode(mode) => assert_eq!(mode, "sometimes"),
        other => panic!("expected UnsupportedMode, got {other:?}"),
    }
}

#[test]
fn missing_execute_section_is_rejected() {
    let text = "routeFiles:\n  - routes/r.yaml\n";
    let err = document::parse_job_document(&doc_path(), text).unwrap_err();
    assert!(matches!(err, JobDocError::MissingExecute), "got {err:?}");
}

#[test]
fn scenario_section_is_exclusive() {
    let text = format!("{VALID_ONE_SHOT}scenario:\n  actions: []\n");
    let err = document::parse_job_document(&doc_path(), &text).unwrap_err();
    assert!(
        matches!(err, JobDocError::ExclusiveWithScenario),
        "got {err:?}"
    );
}

#[test]
fn unit_tier_sections_are_exclusive() {
    let text = format!("{VALID_ONE_SHOT}expects:\n  mock:out:\n    count: 1\n");
    let err = document::parse_job_document(&doc_path(), &text).unwrap_err();
    match err {
        JobDocError::MixedVocabulary { sections } => assert_eq!(sections, vec!["expects"]),
        other => panic!("expected MixedVocabulary, got {other:?}"),
    }
}

#[test]
fn non_test_suffix_is_rejected() {
    let err =
        document::parse_job_document(Path::new("fixtures/job.yaml"), VALID_ONE_SHOT).unwrap_err();
    assert!(
        matches!(err, JobDocError::NotTestSuffix { .. }),
        "got {err:?}"
    );
}

#[test]
fn route_source_conflict_is_the_family_rule() {
    let text = r#"
execute:
  mode: one-shot
  timeout: 5s
  send:
    to: direct:transform
routeFiles:
  - routes/a.yaml
routeFilesFromRoot:
  - routes/b.yaml
"#;
    let err = document::parse_job_document(&doc_path(), text).unwrap_err();
    match err {
        JobDocError::RouteSource(TestDocError::RouteSourceConflict { present }) => {
            assert_eq!(present, vec!["routeFiles", "routeFilesFromRoot"]);
        }
        other => panic!("expected RouteSourceConflict, got {other:?}"),
    }
}

#[test]
fn send_scheme_must_be_direct_or_seda() {
    let text = VALID_ONE_SHOT.replace("direct:transform", "http://example.com/api");
    let err = document::parse_job_document(&doc_path(), &text).unwrap_err();
    assert!(
        matches!(err, JobDocError::UnsupportedSendScheme { .. }),
        "got {err:?}"
    );
}

#[test]
fn body_scalars_are_rejected() {
    let text = VALID_ONE_SHOT.replace("body: \"ping\"", "body: 42");
    let err = document::parse_job_document(&doc_path(), &text).unwrap_err();
    match err {
        JobDocError::UnsupportedBodyScalar(scalar) => assert_eq!(scalar, "42"),
        other => panic!("expected UnsupportedBodyScalar, got {other:?}"),
    }
}

#[test]
fn consumer_gate_allows_only_job_safe_schemes() {
    for uri in ["direct:x", "seda:q", "log:out", "mock:endpoint"] {
        assert!(document::validate_consumer_uri(uri).is_ok(), "{uri}");
    }
    for uri in [
        "http:x",
        "kafka:topic",
        "timer:tick",
        "grpc:svc",
        "noscheme",
    ] {
        assert!(document::validate_consumer_uri(uri).is_err(), "{uri}");
    }
}

#[test]
fn uri_base_strips_query_options() {
    assert_eq!(document::uri_base("direct:x?a=1&b=2"), "direct:x");
    assert_eq!(document::uri_base("seda:q"), "seda:q");
}

#[test]
fn seda_send_uri_forces_wait_always() {
    assert_eq!(
        document::seda_send_uri("seda:q"),
        "seda:q?waitForTaskToComplete=Always"
    );
    assert_eq!(
        document::seda_send_uri("seda:q?size=10"),
        "seda:q?size=10&waitForTaskToComplete=Always"
    );
    // An explicit fire-and-forget value is replaced: verdict fidelity
    // outranks the author's wait preference in v1.
    assert_eq!(
        document::seda_send_uri("seda:q?waitForTaskToComplete=Never"),
        "seda:q?waitForTaskToComplete=Always"
    );
    assert_eq!(
        document::seda_send_uri("seda:q?size=10&waitForTaskToComplete=never"),
        "seda:q?size=10&waitForTaskToComplete=Always"
    );
}

#[test]
fn target_route_ids_detects_ambiguity() {
    let text = r#"
routes:
  - id: "dup-a"
    from: "direct:same"
    steps:
      - set_body:
          value: "a"
  - id: "dup-b"
    from: "direct:same"
    steps:
      - set_body:
          value: "b"
  - id: "other"
    from: "direct:other"
    steps:
      - set_body:
          value: "c"
"#;
    let defs = camel_dsl::parse_routes_with_env(text, &|_| None).expect("routes parse");
    assert_eq!(
        document::target_route_ids(&defs, "direct:same"),
        vec!["dup-a".to_string(), "dup-b".to_string()]
    );
    assert_eq!(
        document::target_route_ids(&defs, "direct:other"),
        vec!["other".to_string()]
    );
    assert!(document::target_route_ids(&defs, "direct:missing").is_empty());
}
