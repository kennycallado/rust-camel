//! Unit tests for the `execute:` job document model.

use std::path::Path;

use crate::commands::test::document::TestDocError;

use super::document::{self, JobDocError};

/// A `.job.yaml` path inside a tempdir-free constant (the parser only
/// inspects the suffix).
fn doc_path() -> std::path::PathBuf {
    Path::new("fixtures/job.job.yaml").to_path_buf()
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
    assert_eq!(doc.execute.mode, document::JobMode::OneShot);
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
fn batch_mode_parses() {
    let text = VALID_ONE_SHOT.replace("one-shot", "batch");
    let doc = document::parse_job_document(&doc_path(), &text).expect("batch mode parses");
    assert_eq!(doc.execute.mode, document::JobMode::Batch);
}

#[test]
fn garbage_mode_is_rejected() {
    let text = VALID_ONE_SHOT.replace("one-shot", "sometimes");
    let err = document::parse_job_document(&doc_path(), &text).unwrap_err();
    match &err {
        JobDocError::UnsupportedMode(mode) => assert_eq!(mode, "sometimes"),
        other => panic!("expected UnsupportedMode, got {other:?}"),
    }
    let message = err.to_string();
    assert!(
        message.contains("one-shot") && message.contains("batch"),
        "unsupported-mode error must name both accepted modes; got: {message}"
    );
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
fn non_job_suffix_is_rejected() {
    let err =
        document::parse_job_document(Path::new("fixtures/job.yaml"), VALID_ONE_SHOT).unwrap_err();
    assert!(
        matches!(err, JobDocError::NotJobSuffix { .. }),
        "got {err:?}"
    );
}

#[test]
fn test_suffix_document_is_rejected_with_rename_guidance() {
    // A `.test.yaml` declaring `execute:` is a load error pointing at the
    // rename: the test suffix names a camel test document.
    let err = document::parse_job_document(Path::new("fixtures/job.test.yaml"), VALID_ONE_SHOT)
        .unwrap_err();
    match err {
        JobDocError::NotJobSuffix { ref path } => {
            assert!(path.ends_with("job.test.yaml"), "path was: {path}");
        }
        other => panic!("expected NotJobSuffix, got {other:?}"),
    }
    let msg = err.to_string();
    assert!(
        msg.contains("must use the reserved .job.yaml"),
        "display was: {msg}"
    );
}

#[test]
fn description_is_optional_and_accepted() {
    let text = "
description: create a user via direct:in
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routeFiles:
  - routes/job-route.yaml
";
    let doc = document::parse_job_document(&doc_path(), text).expect("description parses");
    let _ = doc;
}

#[test]
fn description_non_string_is_rejected() {
    let text = "
description:
  a: 1
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routeFiles:
  - routes/job-route.yaml
";
    let err = document::parse_job_document(&doc_path(), text).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("description"),
        "non-string description must name the field; got: {msg}"
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
fn consumer_gate_rejects_timer_and_cron() {
    // Scheduled recurring producers would auto-fire after the trigger
    // send and perturb the batch drain verdict, so the gate fails
    // closed at load — the reject message must name the scheme.
    for (uri, scheme) in [("timer:1s", "timer"), ("cron:0/5 * * * * ?", "cron")] {
        let err = document::validate_consumer_uri(uri).unwrap_err();
        assert!(
            err.contains(&format!("scheme `{scheme}` is rejected")),
            "{uri} reject must name the scheme; got: {err}"
        );
    }
}

#[test]
fn job_gate_accepts_stream_in() {
    assert!(document::validate_consumer_uri("stream:in").is_ok());
    assert!(document::validate_consumer_uri("stream:in?frame=line").is_ok());
}

#[test]
fn job_gate_rejects_stream_out_as_consumer() {
    let err = document::validate_consumer_uri("stream:out").unwrap_err();
    assert!(err.contains("stream:in"), "got {err}");
}

#[test]
fn job_gate_rejects_stream_err_as_consumer() {
    let err = document::validate_consumer_uri("stream:err").unwrap_err();
    assert!(err.contains("stream:in"), "got {err}");
}

#[test]
fn job_gate_rejects_bare_and_unknown_stream_paths() {
    for uri in ["stream:", "stream:foo"] {
        let err = document::validate_consumer_uri(uri).unwrap_err();
        assert!(err.contains("stream:in"), "{uri}: {err}");
    }
}

#[test]
fn job_gate_still_rejects_kafka() {
    assert!(document::validate_consumer_uri("kafka:topic").is_err());
}

#[test]
fn job_gate_still_accepts_direct() {
    assert!(document::validate_consumer_uri("direct:in").is_ok());
}

#[test]
fn job_gate_producers_unrestricted_with_stream_sinks() {
    let text = r#"
execute:
  mode: one-shot
  timeout: 5s
  send:
    to: direct:in
routes:
  - id: echo
    from: "direct:in"
    steps:
      - to: "stream:out"
      - to: "stream:err"
"#;
    let doc = document::parse_job_document(&doc_path(), text).expect("document loads");
    let source =
        document::resolve_route_source(&doc, Path::new("")).expect("route source resolves");
    let document::JobRouteSource::Inline(routes_yaml) = source else {
        panic!("expected inline routes source");
    };
    let defs = camel_dsl::parse_routes_with_env(&routes_yaml, &|_| None).expect("routes parse");
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].from_uri(), "direct:in");
    assert!(
        document::validate_consumer_uri(defs[0].from_uri()).is_ok(),
        "consumer gate must pass for direct:in with stream: sinks"
    );
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

#[test]
fn missing_send_is_rejected() {
    let text = VALID_ONE_SHOT.replace(
        "  send:\n    to: direct:transform\n    body: \"ping\"\n    headers:\n      X-Job: cli\n",
        "",
    );
    let err = document::parse_job_document(&doc_path(), &text).unwrap_err();
    match err {
        JobDocError::Yaml(ref msg) if msg.contains("execute.send is required") => {}
        other => panic!("expected Yaml naming execute.send, got {other:?}"),
    }
}

#[test]
fn capture_reply_defaults_to_false() {
    let text = VALID_ONE_SHOT.replace("  capture-reply: true\n", "");
    let doc =
        document::parse_job_document(&doc_path(), &text).expect("parses without capture-reply");
    assert!(!doc.execute.capture_reply);
}

#[test]
fn route_files_from_root_without_camel_toml_is_rejected() {
    let doc = document::JobDocument {
        execute: document::ExecuteSection {
            mode: document::JobMode::OneShot,
            send: document::JobSendAction {
                to: "direct:transform".to_string(),
                body: None,
                headers: None,
            },
            capture_reply: false,
            timeout: std::time::Duration::from_secs(30),
        },
        args: None,
        route_files: None,
        route_files_from_root: Some(vec!["routes/a.yaml".to_string()]),
        routes: None,
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let err = match document::resolve_route_source(&doc, dir.path()) {
        Ok(_) => panic!("expected NoProjectRoot error"),
        Err(e) => e,
    };
    match err {
        JobDocError::RouteSource(TestDocError::NoProjectRoot { doc_dir }) => {
            assert!(
                doc_dir.contains(dir.path().display().to_string().as_str()),
                "doc_dir was: {doc_dir}"
            );
        }
        other => panic!("expected NoProjectRoot, got {other:?}"),
    }
}

#[test]
fn parse_declared_job_args() {
    // Arrange: a `.job.yaml` with a top-level `args` map beside
    // `execute`, exercising every allowed declaration field.
    let text = r#"
description: create a customer
args:
  name:
    required: true
    description: Customer name
  tier:
    default: gold
    description: Service tier
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
    body: "ping"
routeFiles:
  - routes/job-route.yaml
"#;
    // Act
    let doc = document::parse_job_document(&doc_path(), text).expect("declared args parse");
    // Assert: declarations preserve required/default/description.
    let args = doc.args.as_ref().expect("declarations carried");
    let name = args.entries.get("name").expect("name declaration");
    assert!(name.required);
    assert_eq!(name.default, None);
    assert_eq!(name.description.as_deref(), Some("Customer name"));
    let tier = args.entries.get("tier").expect("tier declaration");
    assert!(!tier.required);
    assert_eq!(tier.default.as_deref(), Some("gold"));
    assert_eq!(tier.description.as_deref(), Some("Service tier"));
}

#[test]
fn reject_unknown_argument_field() {
    // Arrange: a declaration with the malformed field `requried`.
    let text = r#"
args:
  name:
    requried: true
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routes:
  - id: r
    from: direct:transform
"#;
    // Act
    let err = document::parse_job_document(&doc_path(), text).unwrap_err();
    // Assert: the diagnostic names the unknown field; the CLI maps
    // every job-document load failure to exit 2 before boot.
    match &err {
        JobDocError::UnknownArgumentField { argument, field } => {
            assert_eq!(argument, "name");
            assert_eq!(field, "requried");
        }
        other => panic!("expected UnknownArgumentField, got {other:?}"),
    }
    let msg = err.to_string();
    assert!(
        msg.contains("unknown field") && msg.contains("requried"),
        "unknown-field diagnostic must name the field; got: {msg}"
    );
}

#[test]
fn reject_invalid_argument_name() {
    // Arrange: a declaration keyed by a non-identifier name.
    let text = r#"
args:
  customer-id:
    required: true
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routes:
  - id: r
    from: direct:transform
"#;
    // Act
    let err = document::parse_job_document(&doc_path(), text).unwrap_err();
    // Assert: the argument-name diagnostic names the offending name.
    match &err {
        JobDocError::InvalidArgumentName { name } => assert_eq!(name, "customer-id"),
        other => panic!("expected InvalidArgumentName, got {other:?}"),
    }
    let msg = err.to_string();
    assert!(
        msg.contains("customer-id"),
        "name diagnostic must name the argument; got: {msg}"
    );
}

#[test]
fn reserved_argument_name_rejected_all_four() {
    // Arrange: a declaration keyed by each static job-subcommand flag,
    // one at a time.
    for name in ["help", "config", "report", "arg"] {
        let text = format!(
            r#"
args:
  {name}:
    required: true
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routes:
  - id: r
    from: direct:transform
"#
        );
        // Act
        let err = document::parse_job_document_with_args(&doc_path(), &text, &[]).unwrap_err();
        // Assert: the diagnostic names the reserved name; the CLI maps
        // every job-document load failure to exit 2 before boot.
        match &err {
            JobDocError::ReservedArgumentName { name: reserved } => assert_eq!(reserved, name),
            other => panic!("expected ReservedArgumentName, got {other:?}"),
        }
        let msg = err.to_string();
        assert!(
            msg.contains("reserved") && msg.contains(name),
            "reserved-name diagnostic must name the argument; got: {msg}"
        );
    }
}

#[test]
fn reserved_argument_name_rejected_on_help_parse() {
    // Arrange: the same declaration for `report`.
    let text = r#"
args:
  report:
    required: true
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routes:
  - id: r
    from: direct:transform
"#;
    // Act
    let err = document::parse_job_document_for_help(&doc_path(), text).unwrap_err();
    // Assert: both parse paths reject identically.
    match &err {
        JobDocError::ReservedArgumentName { name } => assert_eq!(name, "report"),
        other => panic!("expected ReservedArgumentName, got {other:?}"),
    }
}

#[test]
fn non_reserved_flag_like_name_accepted() {
    // Arrange: a declaration whose name merely CONTAINS a reserved word.
    let text = r#"
args:
  helpers:
    required: true
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routes:
  - id: r
    from: direct:transform
"#;
    // Act
    let doc = document::parse_job_document(&doc_path(), text).expect("parses");
    // Assert: the guard is exact-match only.
    assert!(
        doc.args
            .as_ref()
            .expect("declarations carried")
            .entries
            .contains_key("helpers")
    );
}

#[test]
fn reserved_argument_name_rejected_at_compile_validation() {
    // Arrange: an `args:` block declaring the static flag name `config`.
    let text = r#"
args:
  config: {}
"#;
    // Act
    let err = document::validate_job_declarations_for_compile(text).unwrap_err();
    // Assert: the compile-time declaration check rejects the reserved
    // name through the same shared normalization.
    match &err {
        JobDocError::ReservedArgumentName { name } => assert_eq!(name, "config"),
        other => panic!("expected ReservedArgumentName, got {other:?}"),
    }
}

#[test]
fn reject_non_boolean_required() {
    // Arrange: a declaration whose `required` field is a string.
    let text = r#"
args:
  name:
    required: "yes"
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routes:
  - id: r
    from: direct:transform
"#;
    // Act
    let err = document::parse_job_document(&doc_path(), text).unwrap_err();
    // Assert: the declaration diagnostic names the argument and the
    // offending field type.
    match &err {
        JobDocError::InvalidArgumentDeclaration { argument, detail } => {
            assert_eq!(argument, "name");
            assert!(
                detail.contains("boolean"),
                "detail must name the type; got: {detail}"
            );
        }
        other => panic!("expected InvalidArgumentDeclaration, got {other:?}"),
    }
    let msg = err.to_string();
    assert!(
        msg.contains("name") && msg.contains("boolean"),
        "declaration diagnostic must name the argument and type; got: {msg}"
    );
}

#[test]
fn reject_non_string_default() {
    // Arrange: a declaration whose `default` field is a number.
    let text = r#"
args:
  tier:
    default: 42
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routes:
  - id: r
    from: direct:transform
"#;
    // Act
    let err = document::parse_job_document(&doc_path(), text).unwrap_err();
    // Assert: the declaration diagnostic names the argument and the
    // offending field type.
    match &err {
        JobDocError::InvalidArgumentDeclaration { argument, detail } => {
            assert_eq!(argument, "tier");
            assert!(
                detail.contains("string"),
                "detail must name the type; got: {detail}"
            );
        }
        other => panic!("expected InvalidArgumentDeclaration, got {other:?}"),
    }
    let msg = err.to_string();
    assert!(
        msg.contains("tier") && msg.contains("string"),
        "declaration diagnostic must name the argument and type; got: {msg}"
    );
}

#[test]
fn reject_scalar_argument_declaration() {
    // Arrange: a declaration that is a plain scalar, not a mapping.
    let text = r#"
args:
  name: "just a string"
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routes:
  - id: r
    from: direct:transform
"#;
    // Act
    let err = document::parse_job_document(&doc_path(), text).unwrap_err();
    // Assert: the declaration diagnostic names the argument and the
    // expected mapping shape.
    match &err {
        JobDocError::InvalidArgumentDeclaration { argument, detail } => {
            assert_eq!(argument, "name");
            assert!(
                detail.contains("mapping"),
                "detail must name the shape; got: {detail}"
            );
        }
        other => panic!("expected InvalidArgumentDeclaration, got {other:?}"),
    }
    let msg = err.to_string();
    assert!(
        msg.contains("name") && msg.contains("mapping"),
        "declaration diagnostic must name the argument and shape; got: {msg}"
    );
}

#[test]
fn empty_args_select_declared_mode() {
    // Arrange: `args: {}` — the operator would pass a CLI pair (e.g.
    // `--arg name=John`); the parser only records the declared mode.
    let text = r#"
args: {}
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routes:
  - id: r
    from: direct:transform
"#;
    // Act
    let doc = document::parse_job_document(&doc_path(), text).expect("empty args parse");
    // Assert: declarations are present and legacy-header mode is false.
    let args = doc.args.as_ref().expect("declared mode selected");
    assert!(args.entries.is_empty());
    assert!(!doc.legacy_arg_headers());
}

#[test]
fn absent_args_select_legacy_mode() {
    // Arrange: a document without any top-level `args:` block.
    // Act
    let doc =
        document::parse_job_document(&doc_path(), VALID_ONE_SHOT).expect("legacy document parses");
    // Assert: no declarations and legacy-header mode stays on.
    assert!(doc.args.is_none());
    assert!(doc.legacy_arg_headers());
}

#[test]
fn help_parse_accepts_arg_tokens_in_to_and_timeout() {
    // Arrange: a declared document whose `to`/`timeout` hold
    // `${arg:...}` tokens; the help projection must not interpolate or
    // validate the duration text.
    let text = r#"
args:
  target:
    required: true
execute:
  mode: one-shot
  timeout: "${arg:wait}"
  send:
    to: "${arg:target}"
routeFiles:
  - routes/job-route.yaml
"#;
    // Act
    let info = document::parse_job_document_for_help(&doc_path(), text).expect("help parse");
    // Assert: raw token text, the validated mode spelling, and the
    // normalized declaration.
    assert_eq!(info.send_to, "${arg:target}");
    assert_eq!(info.mode, "one-shot");
    let args = info.args.as_ref().expect("declarations carried");
    let target = args.entries.get("target").expect("target declaration");
    assert!(target.required);
}

#[test]
fn help_parse_accepts_unsatisfiable_required_argument() {
    // Arrange: a `required: true` argument with no default and no
    // `--arg` pairs available; help renders the interface, so nothing
    // may demand a value.
    let text = r#"
args:
  name:
    required: true
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routeFiles:
  - routes/job-route.yaml
"#;
    // Act
    let info = document::parse_job_document_for_help(&doc_path(), text);
    // Assert: no MissingRequiredArgument can fire at help time.
    assert!(info.is_ok(), "got {info:?}");
}

#[test]
fn help_parse_rejects_structural_errors() {
    // Arrange: one text per structural failure class.
    let unknown_top_level = r#"
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routeFiles:
  - routes/job-route.yaml
surprise: true
"#;
    let unknown_argument_field = r#"
args:
  name:
    requried: true
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routeFiles:
  - routes/job-route.yaml
"#;
    let missing_execute = "routeFiles:\n  - routes/job-route.yaml\n";
    // Act + Assert: the same variants the execution parser produces.
    match document::parse_job_document_for_help(&doc_path(), unknown_top_level) {
        Err(JobDocError::UnknownField(_)) => {}
        other => panic!("expected UnknownField, got {other:?}"),
    }
    match document::parse_job_document_for_help(&doc_path(), unknown_argument_field) {
        Err(JobDocError::UnknownArgumentField { argument, field }) => {
            assert_eq!(argument, "name");
            assert_eq!(field, "requried");
        }
        other => panic!("expected UnknownArgumentField, got {other:?}"),
    }
    let err = document::parse_job_document_for_help(&doc_path(), missing_execute).unwrap_err();
    assert!(matches!(err, JobDocError::MissingExecute), "got {err:?}");
}

#[test]
fn help_parse_requires_send_and_timeout_presence() {
    // Arrange: one text without `execute.send`, one without
    // `execute.timeout`.
    let no_send = r#"
execute:
  mode: one-shot
  timeout: 30s
routeFiles:
  - routes/job-route.yaml
"#;
    let no_timeout = r#"
execute:
  mode: one-shot
  send:
    to: direct:transform
routeFiles:
  - routes/job-route.yaml
"#;
    // Both missing: parity with the execution parser — timeout
    // presence is checked first, so this reports MissingTimeout.
    let neither = r#"
execute:
  mode: one-shot
routeFiles:
  - routes/job-route.yaml
"#;
    // Act
    let send_err = document::parse_job_document_for_help(&doc_path(), no_send).unwrap_err();
    let timeout_err = document::parse_job_document_for_help(&doc_path(), no_timeout).unwrap_err();
    let both_err = document::parse_job_document_for_help(&doc_path(), neither).unwrap_err();
    // Assert: the send-presence error is the shared `Yaml` spelling;
    // the timeout-presence error is its own variant (no duration parse).
    match send_err {
        JobDocError::Yaml(msg) => assert_eq!(
            msg, "execute.send is required: exactly one send action",
            "send-presence payload must match the execution parser verbatim"
        ),
        other => panic!("expected Yaml naming execute.send, got {other:?}"),
    }
    assert!(
        matches!(timeout_err, JobDocError::MissingTimeout),
        "got {timeout_err:?}"
    );
    assert!(
        matches!(both_err, JobDocError::MissingTimeout),
        "both-missing must report MissingTimeout (impl check order); got {both_err:?}"
    );
}

#[test]
fn help_parse_covers_structural_prefix_errors() {
    // Arrange: one minimal failing input per copied structural-prefix
    // branch. Parity form: each error must equal what the execution
    // parser yields for the SAME input, so the copies cannot drift
    // from `parse_job_document_impl`.
    let valid = r#"
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routeFiles:
  - routes/job-route.yaml
"#;
    let cases: Vec<(&str, std::path::PathBuf, String)> = vec![
        (
            "NotJobSuffix",
            Path::new("fixtures/job.yaml").to_path_buf(),
            valid.to_string(),
        ),
        (
            "ExclusiveWithScenario",
            doc_path(),
            format!("{valid}scenario:\n  actions: []\n"),
        ),
        (
            "MixedVocabulary",
            doc_path(),
            format!("{valid}expects:\n  mock:out:\n    count: 1\n"),
        ),
        (
            "RouteSource",
            doc_path(),
            format!("{valid}routes:\n  - id: r\n    from: direct:transform\n"),
        ),
        (
            "UnsupportedMode",
            doc_path(),
            valid.replace("one-shot", "sometimes"),
        ),
    ];
    for (label, path, text) in cases {
        // Act
        let help = document::parse_job_document_for_help(&path, &text);
        let exec = document::parse_job_document(&path, &text);
        // Assert
        match (help, exec) {
            (Err(help_err), Err(exec_err)) => assert_eq!(
                format!("{help_err:?}"),
                format!("{exec_err:?}"),
                "help projection must match the execution parser for {label}"
            ),
            (help_result, exec_result) => panic!(
                "expected both parsers to reject the {label} input; help {help_result:?}, exec {exec_result:?}"
            ),
        }
    }
}

#[test]
fn arg_type_each_spelling_accepted() {
    // Arrange: one argument per accepted `type` spelling, plus an
    // untyped sibling to compare against.
    let text = r#"
args:
  a:
    type: string
  b:
    type: int
  c:
    type: bool
  d:
    type: "enum[x,y]"
  u:
    default: gold
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routes:
  - id: r
    from: direct:transform
"#;
    // Act
    let doc = document::parse_job_document(&doc_path(), text).expect("typed args parse");
    // Assert: four declarations carry the matching type model.
    let args = doc.args.as_ref().expect("declarations carried");
    assert_eq!(
        args.entries.get("a").expect("a declaration").arg_type,
        document::JobArgType::String
    );
    assert_eq!(
        args.entries.get("b").expect("b declaration").arg_type,
        document::JobArgType::Int
    );
    assert_eq!(
        args.entries.get("c").expect("c declaration").arg_type,
        document::JobArgType::Bool
    );
    assert_eq!(
        args.entries.get("d").expect("d declaration").arg_type,
        document::JobArgType::Enum(vec!["x".to_string(), "y".to_string()])
    );
    // Assert: `a` is indistinguishable from the untyped declaration.
    let untyped = args.entries.get("u").expect("u declaration");
    assert_eq!(
        args.entries.get("a").expect("a declaration").arg_type,
        untyped.arg_type
    );
}

#[test]
fn arg_type_unknown_word_rejected() {
    // Arrange: an unknown type word.
    let text = r#"
args:
  count:
    type: flot
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routes:
  - id: r
    from: direct:transform
"#;
    // Act
    let err = document::parse_job_document(&doc_path(), text).unwrap_err();
    // Assert: the diagnostic names the argument and the raw value.
    match &err {
        JobDocError::InvalidArgumentType { argument, raw } => {
            assert_eq!(argument, "count");
            assert_eq!(raw, "flot");
        }
        other => panic!("expected InvalidArgumentType, got {other:?}"),
    }
    let msg = err.to_string();
    assert!(
        msg.contains("flot") && msg.contains("count"),
        "type diagnostic must name the argument and raw value; got: {msg}"
    );
}

#[test]
fn arg_type_malformed_enum_grammar_rejected() {
    // Arrange: one malformed enum value per rejection rule (empty
    // list, empty member after trim, duplicate member after trim,
    // forbidden `[`, `]`, LF, and CR inside a member).
    for raw in [
        "enum[]",
        "enum[a,,b]",
        "enum[a,a]",
        "enum[a[b]",
        "enum[a]b]",
        "enum[a\nb]",
        "enum[a\rb]",
    ] {
        // LF/CR must reach the parser as YAML escape sequences inside
        // the double-quoted scalar; a literal line break would be a
        // YAML continuation error before the type grammar runs.
        let escaped = raw.replace('\n', "\\n").replace('\r', "\\r");
        let text = format!(
            r#"
args:
  count:
    type: "{escaped}"
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routes:
  - id: r
    from: direct:transform
"#
        );
        // Act
        let err = document::parse_job_document(&doc_path(), &text).unwrap_err();
        // Assert: the diagnostic names the argument and the raw value.
        match &err {
            JobDocError::InvalidArgumentType {
                argument,
                raw: value,
            } => {
                assert_eq!(argument, "count");
                assert_eq!(value, raw);
            }
            other => panic!("expected InvalidArgumentType for {raw}, got {other:?}"),
        }
        let msg = err.to_string();
        assert!(
            msg.contains("count") && msg.contains(raw),
            "enum diagnostic must name the argument and raw value; got: {msg}"
        );
    }
}

#[test]
fn arg_type_enum_members_trimmed_and_case_preserved() {
    // Arrange: an enum whose members carry padding whitespace.
    let text = r#"
args:
  tier:
    type: "enum[gold, silver]"
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routes:
  - id: r
    from: direct:transform
"#;
    // Act
    let doc = document::parse_job_document(&doc_path(), text).expect("enum type parses");
    // Assert: members are trimmed, and their case is preserved.
    let args = doc.args.as_ref().expect("declarations carried");
    assert_eq!(
        args.entries.get("tier").expect("tier declaration").arg_type,
        document::JobArgType::Enum(vec!["gold".to_string(), "silver".to_string()])
    );
}

#[test]
fn arg_type_non_string_scalar_rejected() {
    // Arrange: a `type` holding a YAML integer.
    let text = r#"
args:
  count:
    type: 42
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routes:
  - id: r
    from: direct:transform
"#;
    // Act
    let err = document::parse_job_document(&doc_path(), text).unwrap_err();
    // Assert: the declaration diagnostic says `type` must be a string.
    match &err {
        JobDocError::InvalidArgumentDeclaration { argument, detail } => {
            assert_eq!(argument, "count");
            assert!(
                detail.contains("type") && detail.contains("string"),
                "detail must say type must be a string; got: {detail}"
            );
        }
        other => panic!("expected InvalidArgumentDeclaration, got {other:?}"),
    }
}

#[test]
fn arg_type_typed_default_failing_coercion_fails_load() {
    // Arrange: a typed declaration whose default cannot coerce.
    let text = r#"
args:
  count:
    type: int
    default: "abc"
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routes:
  - id: r
    from: direct:transform
"#;
    // Act
    let err = document::parse_job_document(&doc_path(), text).unwrap_err();
    // Assert: the coercion diagnostic names the argument, the
    // expected type, and the raw value.
    match &err {
        JobDocError::ArgumentCoercion {
            name,
            expected,
            raw,
        } => {
            assert_eq!(name, "count");
            assert_eq!(*expected, document::JobArgType::Int);
            assert_eq!(raw, "abc");
        }
        other => panic!("expected ArgumentCoercion, got {other:?}"),
    }
    let msg = err.to_string();
    assert!(
        msg.contains("count") && msg.contains("int") && msg.contains("abc"),
        "coercion diagnostic must name the argument, type, and raw value; got: {msg}"
    );
}

#[test]
fn arg_type_omitted_defaults_to_string() {
    // Arrange: a declaration without a `type` key whose default text
    // would NOT survive an int coercion.
    let text = r#"
args:
  count:
    default: "007"
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routes:
  - id: r
    from: direct:transform
"#;
    // Act
    let doc = document::parse_job_document(&doc_path(), text).expect("untyped args parse");
    // Assert: the type defaults to string and the raw default text
    // survives verbatim.
    let args = doc.args.as_ref().expect("declarations carried");
    let count = args.entries.get("count").expect("count declaration");
    assert_eq!(count.arg_type, document::JobArgType::String);
    assert_eq!(count.default.as_deref(), Some("007"));
}

/// Parse an `args:` YAML fragment into normalized declarations — the
/// shared arrangement for the resolution tests (the surrounding
/// document is the minimal valid one).
fn parse_declarations(args_block: &str) -> document::JobArgumentDeclarations {
    let text = format!(
        "args:\n{args_block}\nexecute:\n  mode: one-shot\n  timeout: 30s\n  \
         send:\n    to: direct:transform\nroutes:\n  - id: r\n    from: direct:transform\n"
    );
    let doc = document::parse_job_document(&doc_path(), &text).expect("declarations parse");
    doc.args.expect("declared mode")
}

/// Resolve successfully and return the lookup map (declared mode
/// always yields one).
fn resolve_ok(
    decls: &document::JobArgumentDeclarations,
    pairs: &[(String, String)],
) -> std::collections::BTreeMap<String, String> {
    document::resolve_job_args(Some(decls), pairs)
        .expect("resolves")
        .expect("declared mode yields a map")
}

#[test]
fn resolve_int_coerces_canonical_form() {
    // Arrange: an `int` declaration and a pair in a non-canonical but
    // parsable form.
    let decls = parse_declarations("  count:\n    type: int");
    let pairs = vec![("count".to_string(), "007".to_string())];
    // Act
    let resolved = resolve_ok(&decls, &pairs);
    // Assert: the canonical plain-decimal form substitutes.
    assert_eq!(resolved.get("count").map(String::as_str), Some("7"));
}

#[test]
fn resolve_int_rejects_non_integer() {
    // Arrange: an `int` declaration; one failing raw per rejection rule
    // (non-numeric, float, surrounding whitespace).
    let decls = parse_declarations("  count:\n    type: int");
    for raw in ["abc", "3.5", " 42"] {
        let pairs = vec![("count".to_string(), raw.to_string())];
        // Act
        let err = document::resolve_job_args(Some(&decls), &pairs).unwrap_err();
        // Assert: the coercion diagnostic names the argument, the
        // expected type, and the raw value.
        match &err {
            JobDocError::ArgumentCoercion {
                name,
                expected,
                raw: value,
            } => {
                assert_eq!(name, "count");
                assert_eq!(*expected, document::JobArgType::Int);
                assert_eq!(value, raw);
            }
            other => panic!("expected ArgumentCoercion for {raw:?}, got {other:?}"),
        }
        let msg = err.to_string();
        assert!(
            msg.contains("count") && msg.contains("int") && msg.contains(raw),
            "coercion diagnostic must name the argument, type, and raw value; got: {msg}"
        );
    }
}

#[test]
fn resolve_int_plus_sign_and_overflow() {
    // Arrange: an `int` declaration; a sign-carrying value and one
    // beyond the i64 range.
    let decls = parse_declarations("  count:\n    type: int");
    let plus = vec![("count".to_string(), "+5".to_string())];
    // Act + Assert: an explicit `+` is accepted and stripped.
    let resolved = resolve_ok(&decls, &plus);
    assert_eq!(resolved.get("count").map(String::as_str), Some("5"));
    // Act + Assert: overflow rejects with ArgumentCoercion.
    let overflow = vec![("count".to_string(), "99999999999999999999".to_string())];
    let err = document::resolve_job_args(Some(&decls), &overflow).unwrap_err();
    match &err {
        JobDocError::ArgumentCoercion {
            name,
            expected,
            raw,
        } => {
            assert_eq!(name, "count");
            assert_eq!(*expected, document::JobArgType::Int);
            assert_eq!(raw, "99999999999999999999");
        }
        other => panic!("expected ArgumentCoercion, got {other:?}"),
    }
}

#[test]
fn resolve_bool_case_insensitive_canonical_lowercase() {
    // Arrange: a `bool` declaration; both accepted spellings in mixed
    // case.
    let decls = parse_declarations("  verbose:\n    type: bool");
    let t = vec![("verbose".to_string(), "TRUE".to_string())];
    // Act + Assert: uppercase TRUE canonicalizes lowercase.
    let resolved = resolve_ok(&decls, &t);
    assert_eq!(resolved.get("verbose").map(String::as_str), Some("true"));
    let f = vec![("verbose".to_string(), "False".to_string())];
    let resolved = resolve_ok(&decls, &f);
    assert_eq!(resolved.get("verbose").map(String::as_str), Some("false"));
}

#[test]
fn resolve_bool_rejects_numeric_and_unknown() {
    // Arrange: a `bool` declaration; `1`/`0` are NOT bool spellings.
    let decls = parse_declarations("  verbose:\n    type: bool");
    for raw in ["1", "0", "yes"] {
        let pairs = vec![("verbose".to_string(), raw.to_string())];
        // Act
        let err = document::resolve_job_args(Some(&decls), &pairs).unwrap_err();
        // Assert: the coercion diagnostic names the argument, the
        // expected type, and the raw value.
        match &err {
            JobDocError::ArgumentCoercion {
                name,
                expected,
                raw: value,
            } => {
                assert_eq!(name, "verbose");
                assert_eq!(*expected, document::JobArgType::Bool);
                assert_eq!(value, raw);
            }
            other => panic!("expected ArgumentCoercion for {raw:?}, got {other:?}"),
        }
        let msg = err.to_string();
        assert!(
            msg.contains("verbose") && msg.contains("bool") && msg.contains(raw),
            "coercion diagnostic must name the argument, type, and raw value; got: {msg}"
        );
    }
}

#[test]
fn resolve_enum_member_verbatim_and_outsider_lists_members() {
    // Arrange: an `enum[bronze,gold]` declaration.
    let decls = parse_declarations("  tier:\n    type: \"enum[bronze,gold]\"");
    let gold = vec![("tier".to_string(), "gold".to_string())];
    // Act + Assert: a member passes verbatim.
    let resolved = resolve_ok(&decls, &gold);
    assert_eq!(resolved.get("tier").map(String::as_str), Some("gold"));
    // Act + Assert: an outsider AND a case mismatch both reject, and
    // the rendered message lists the allowed members verbatim.
    for raw in ["silver", "Gold"] {
        let pairs = vec![("tier".to_string(), raw.to_string())];
        let err = document::resolve_job_args(Some(&decls), &pairs).unwrap_err();
        match &err {
            JobDocError::ArgumentCoercion {
                name,
                expected,
                raw: value,
            } => {
                assert_eq!(name, "tier");
                assert_eq!(
                    *expected,
                    document::JobArgType::Enum(vec!["bronze".to_string(), "gold".to_string()])
                );
                assert_eq!(value, raw);
            }
            other => panic!("expected ArgumentCoercion for {raw:?}, got {other:?}"),
        }
        let msg = err.to_string();
        assert!(
            msg.contains("enum[bronze,gold]"),
            "enum coercion diagnostic must list the members; got: {msg}"
        );
    }
}

#[test]
fn resolve_typed_default_canonicalizes_without_pair() {
    // Arrange: a typed default and NO `--arg` pairs at all.
    let decls = parse_declarations("  count:\n    type: int\n    default: \"007\"");
    let pairs: Vec<(String, String)> = Vec::new();
    // Act
    let resolved = resolve_ok(&decls, &pairs);
    // Assert: the applied default is canonicalized at resolution too.
    assert_eq!(resolved.get("count").map(String::as_str), Some("7"));
}

#[test]
fn resolve_unknown_name_precedes_coercion() {
    // Arrange: one typed declaration; the pairs carry an unknown name
    // AND an uncoercible value.
    let decls = parse_declarations("  count:\n    type: int");
    let pairs = vec![
        ("ghost".to_string(), "1".to_string()),
        ("count".to_string(), "abc".to_string()),
    ];
    // Act
    let err = document::resolve_job_args(Some(&decls), &pairs).unwrap_err();
    // Assert: unknown-name wins over coercion.
    match &err {
        JobDocError::UnknownArgumentName { name } => assert_eq!(name, "ghost"),
        other => panic!("expected UnknownArgumentName, got {other:?}"),
    }
}

#[test]
fn resolve_missing_required_precedes_coercion() {
    // Arrange: a required `name` without a default plus a typed
    // `count`; the only pair carries an uncoercible `count` value.
    let decls = parse_declarations(
        "  name:\n    type: string\n    required: true\n  count:\n    type: int",
    );
    let pairs = vec![("count".to_string(), "abc".to_string())];
    // Act
    let err = document::resolve_job_args(Some(&decls), &pairs).unwrap_err();
    // Assert: missing-required wins over coercion.
    match &err {
        JobDocError::MissingRequiredArgument { name } => assert_eq!(name, "name"),
        other => panic!("expected MissingRequiredArgument, got {other:?}"),
    }
}

#[test]
fn resolve_untyped_values_stay_verbatim() {
    // Arrange: an untyped declaration (A2 behavior).
    let decls = parse_declarations("  tier:\n    default: gold");
    let pairs = vec![("tier".to_string(), "007".to_string())];
    // Act
    let resolved = resolve_ok(&decls, &pairs);
    // Assert: verbatim, bit-identical to A2.
    assert_eq!(resolved.get("tier").map(String::as_str), Some("007"));
}
