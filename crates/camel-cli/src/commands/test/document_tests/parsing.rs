use super::*;

#[test]
fn valid_reference_doc_parses() {
    let yaml = r#"
routeFiles: [config/routes.yaml]
expects:
  mock:result:
    count: 3
"#;
    let doc = parse_test_document(yaml).expect("valid document should parse"); // allow-unwrap
    assert_eq!(
        doc.route_files,
        Some(vec!["config/routes.yaml".to_string()])
    );
    assert!(doc.routes.is_none());
    // Key normalized: `mock:` prefix stripped to bare endpoint name.
    let set = doc
        .expects
        .get("result")
        .expect("normalized key `result` present"); // allow-unwrap
    assert_eq!(set.count, Some(3));
    assert!(!doc.expects.contains_key("mock:result"));
}

#[test]
fn valid_inline_routes_parses() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    minCount: 2
"#;
    let doc = parse_test_document(yaml).expect("valid document should parse"); // allow-unwrap
    assert!(doc.route_files.is_none());
    assert!(doc.routes.is_some());
    let set = doc
        .expects
        .get("result")
        .expect("normalized key `result` present"); // allow-unwrap
    assert_eq!(set.min_count, Some(2));
}

#[test]
fn unknown_field_rejected() {
    let yaml = r#"
bogus: 1
routes:
  - id: r1
    from: "direct:start"
expects:
  mock:result:
    count: 1
"#;
    let err = err_of(yaml);
    assert!(
        matches!(err, TestDocError::UnknownField(ref msg) if msg.contains("bogus")),
        "expected UnknownField naming `bogus`, got: {err:?}"
    );
}

#[test]
fn empty_expects_rejected() {
    let explicit = r#"
routes:
  - id: r1
    from: "timer:tick"
expects: {}
"#;
    assert!(
        matches!(err_of(explicit), TestDocError::ExpectsEmpty),
        "explicit empty expects must be rejected"
    );
    // No `expects` key at all: serde default yields an empty map that
    // reaches validation.
    let missing = r#"
routeFiles: [config/routes.yaml]
"#;
    assert!(
        matches!(err_of(missing), TestDocError::ExpectsEmpty),
        "missing expects must be rejected"
    );
}

#[test]
fn bare_expect_key_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
expects:
  result:
    count: 1
"#;
    assert!(matches!(
        err_of(yaml),
        TestDocError::ExpectKeyMissingScheme { ref key } if key == "result"
    ));
}

#[test]
fn route_source_conflict_rejected() {
    let both = r#"
routeFiles: [config/routes.yaml]
routes:
  - id: r1
    from: "direct:start"
expects:
  mock:result:
    count: 1
"#;
    assert!(matches!(
        err_of(both),
        TestDocError::RouteSourceConflict { .. }
    ));
    // Neither source present is the same conflict.
    let neither = r#"
expects:
  mock:result:
    count: 1
"#;
    assert!(matches!(
        err_of(neither),
        TestDocError::RouteSourceConflict { .. }
    ));
}

#[test]
fn route_files_from_root_parses() {
    let yaml = r#"
routeFilesFromRoot: [config/routes.yaml]
expects:
  mock:result:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("valid document should parse"); // allow-unwrap
    assert_eq!(
        doc.route_files_from_root,
        Some(vec!["config/routes.yaml".to_string()])
    );
    assert!(doc.route_files.is_none());
    assert!(doc.routes.is_none());
}

#[test]
fn route_files_from_root_plus_route_files_rejected() {
    let yaml = r#"
routeFiles: [config/routes.yaml]
routeFilesFromRoot: [config/routes.yaml]
expects:
  mock:result:
    count: 1
"#;
    let err = err_of(yaml);
    let msg = err.to_string();
    assert!(
        matches!(
            err,
            TestDocError::RouteSourceConflict { ref present }
                if *present == ["routeFiles", "routeFilesFromRoot"]
        ),
        "expected RouteSourceConflict listing routeFiles + routeFilesFromRoot, got: {err:?}"
    );
    assert!(
        msg.contains("routeFiles") && msg.contains("routeFilesFromRoot"),
        "display must name both keys, got: {msg}"
    );
}

#[test]
fn route_files_from_root_plus_routes_rejected() {
    let yaml = r#"
routeFilesFromRoot: [config/routes.yaml]
routes:
  - id: r1
    from: "direct:start"
expects:
  mock:result:
    count: 1
"#;
    let err = err_of(yaml);
    let msg = err.to_string();
    assert!(
        matches!(
            err,
            TestDocError::RouteSourceConflict { ref present }
                if *present == ["routeFilesFromRoot", "routes"]
        ),
        "expected RouteSourceConflict listing routeFilesFromRoot + routes, got: {err:?}"
    );
    assert!(
        msg.contains("routeFilesFromRoot") && msg.contains("routes"),
        "display must name both keys, got: {msg}"
    );
}

#[test]
fn no_route_source_rejected() {
    let yaml = r#"
expects:
  mock:result:
    count: 1
"#;
    let err = err_of(yaml);
    let msg = err.to_string();
    assert!(
        matches!(
            err,
            TestDocError::RouteSourceConflict { ref present } if present.is_empty()
        ),
        "expected RouteSourceConflict with empty present, got: {err:?}"
    );
    assert!(
        msg.contains("exactly one") && msg.contains("required"),
        "display must state exactly one route source is required, got: {msg}"
    );
}

#[test]
fn all_three_route_sources_rejected() {
    let yaml = r#"
routeFiles: [config/routes.yaml]
routeFilesFromRoot: [config/routes.yaml]
routes:
  - id: r1
    from: "direct:start"
expects:
  mock:result:
    count: 1
"#;
    let err = err_of(yaml);
    assert!(
        matches!(
            err,
            TestDocError::RouteSourceConflict { ref present }
                if *present == ["routeFiles", "routeFilesFromRoot", "routes"]
        ),
        "expected RouteSourceConflict listing all three keys, got: {err:?}"
    );
}

#[test]
fn route_files_and_routes_still_rejected() {
    let yaml = r#"
routeFiles: [config/routes.yaml]
routes:
  - id: r1
    from: "direct:start"
expects:
  mock:result:
    count: 1
"#;
    let err = err_of(yaml);
    let msg = err.to_string();
    assert!(
        matches!(
            err,
            TestDocError::RouteSourceConflict { ref present }
                if *present == ["routeFiles", "routes"]
        ),
        "expected RouteSourceConflict listing routeFiles + routes, got: {err:?}"
    );
    assert!(
        msg.contains("routeFiles") && msg.contains("routes"),
        "display must name both keys, got: {msg}"
    );
}

#[test]
fn count_and_min_count_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
expects:
  mock:result:
    count: 1
    minCount: 2
"#;
    assert!(matches!(
        err_of(yaml),
        TestDocError::CountAndMinCount(ref endpoint) if endpoint == "result"
    ));
}

#[test]
fn settle_zero_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
expects:
  mock:result:
    count: 1
settle: "0ms"
"#;
    assert!(
        matches!(err_of(yaml), TestDocError::SettleOutOfRange(ref raw) if raw == "0ms"),
        "settle 0ms must be out of range"
    );
}

#[test]
fn settle_over_5s_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
expects:
  mock:result:
    count: 1
settle: "10s"
"#;
    assert!(
        matches!(err_of(yaml), TestDocError::SettleOutOfRange(ref raw) if raw == "10s"),
        "settle 10s must be out of range"
    );
}

#[test]
fn settle_boundaries_accepted() {
    fn doc_with_settle(raw: &str) -> String {
        format!(
            r#"
routes:
  - id: r1
    from: "direct:start"
expects:
  mock:result:
    count: 1
settle: "{raw}"
"#
        )
    }
    let doc = parse_test_document(&doc_with_settle("1ms")).expect("1ms is inside the range"); // allow-unwrap
    assert_eq!(doc.settle_duration(), Some(Duration::from_millis(1)));
    let doc = parse_test_document(&doc_with_settle("5s")).expect("5s is the inclusive upper bound"); // allow-unwrap
    assert_eq!(doc.settle_duration(), Some(Duration::from_secs(5)));
}

#[test]
fn non_direct_input_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
inputs:
  - to: "seda:q"
    body: x
expects:
  mock:result:
    count: 1
"#;
    assert!(matches!(
        err_of(yaml),
        TestDocError::UnsupportedInputScheme { ref target } if target == "seda:q"
    ));
}

#[test]
fn body_scalars_rejected() {
    fn doc_with_body(body: &str) -> String {
        format!(
            r#"
routes:
  - id: r1
    from: "direct:start"
inputs:
  - to: "direct:start"
    body: {body}
expects:
  mock:result:
    count: 1
"#
        )
    }
    for (raw, expected) in [("null", "null"), ("true", "true"), ("7", "7")] {
        let err = err_of(&doc_with_body(raw));
        assert!(
            matches!(err, TestDocError::UnsupportedBodyScalar(ref s) if s == expected),
            "body `{raw}` must surface UnsupportedBodyScalar(\"{expected}\"), got: {err:?}"
        );
    }
}

#[test]
fn body_forms_accepted() {
    fn doc_with_body(body: &str) -> String {
        format!(
            r#"
routes:
  - id: r1
    from: "direct:start"
inputs:
  - to: "direct:start"
    body: {body}
expects:
  mock:result:
    count: 1
"#
        )
    }
    let doc = parse_test_document(&doc_with_body("x")).expect("string body parses"); // allow-unwrap
    assert!(matches!(
        doc.inputs[0].body,
        Some(InputBody::Text(ref s)) if s == "x"
    ));
    let doc = parse_test_document(&doc_with_body("{a: 1}")).expect("object body parses"); // allow-unwrap
    assert!(matches!(
        doc.inputs[0].body,
        Some(InputBody::Json(ref v)) if v.get("a") == Some(&serde_json::json!(1))
    ));
    let doc = parse_test_document(&doc_with_body("[1, 2]")).expect("array body parses"); // allow-unwrap
    assert!(matches!(
        doc.inputs[0].body,
        Some(InputBody::Json(ref v)) if *v == serde_json::json!([1, 2])
    ));
    // A missing body key is legal (None), unlike an explicit null.
    let no_body = r#"
routes:
  - id: r1
    from: "direct:start"
inputs:
  - to: "direct:start"
expects:
  mock:result:
    count: 1
"#;
    let doc = parse_test_document(no_body).expect("input without body parses"); // allow-unwrap
    assert!(doc.inputs[0].body.is_none());
}
