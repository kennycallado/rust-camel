use super::*;

use camel_api::Body;
use camel_component_mock::{BodyMatcher, HeaderMatcher};

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

// --- mock-matchers grammar tests (ported from monolith during rebase) ---

fn doc_with_body_entry(entry: &str) -> String {
    format!(
        r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
    bodies:
      - {entry}
"#
    )
}

fn doc_with_expects_header(value: &str) -> String {
    format!(
        r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
    headers:
      X-Trace: {value}
"#
    )
}

#[test]
fn bare_string_body_stays_exact() {
    let doc =
        parse_test_document(&doc_with_body_entry("plain")).expect("bare string body should parse"); // allow-unwrap
    let bodies = doc.expects["result"]
        .bodies
        .as_ref()
        .expect("bodies present"); // allow-unwrap
    assert_eq!(bodies.len(), 1);
    assert!(matches!(
        bodies[0],
        BodyMatcher::Equals(Body::Text(ref s)) if s == "plain"
    ));
}

#[test]
fn matcher_map_body_accepted() {
    let doc = parse_test_document(&doc_with_body_entry(r#"{regex: "^order-[0-9]+$"}"#))
        .expect("regex matcher map should parse"); // allow-unwrap
    let bodies = doc.expects["result"]
        .bodies
        .as_ref()
        .expect("bodies present"); // allow-unwrap
    assert!(matches!(
        bodies[0],
        BodyMatcher::Regex(ref p) if p == "^order-[0-9]+$"
    ));
}

#[test]
fn matcher_map_header_accepted() {
    let doc = parse_test_document(&doc_with_expects_header(r#"{regex: "^[a-f0-9]{8}$"}"#))
        .expect("header regex matcher should parse"); // allow-unwrap
    let headers = doc.expects["result"]
        .headers
        .as_ref()
        .expect("headers present"); // allow-unwrap
    assert!(matches!(
        headers.get("X-Trace"),
        Some(HeaderMatcher::Regex(p)) if p == "^[a-f0-9]{8}$"
    ));
}

#[test]
fn header_literal_object_stays_equals() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
    headers:
      mode: {batch: 1, predicate: "raw"}
"#;
    let doc = parse_test_document(yaml).expect("multi-key literal header should parse"); // allow-unwrap
    let headers = doc.expects["result"]
        .headers
        .as_ref()
        .expect("headers present"); // allow-unwrap
    // A multi-key object is not a matcher map; it stays a literal `equals`.
    assert!(matches!(
        headers.get("mode"),
        Some(HeaderMatcher::Equals(v))
            if v.get("batch") == Some(&serde_json::json!(1))
                && v.get("predicate") == Some(&serde_json::json!("raw"))
    ));
}

#[test]
fn unknown_matcher_key_rejected() {
    let err = err_of(&doc_with_body_entry(r#"{xpath: "//id"}"#));
    let msg = err.to_string();
    assert!(
        matches!(err, TestDocError::InvalidMatcher(_)),
        "unknown matcher key must fail parsing, got: {err:?}"
    );
    assert!(
        msg.contains("bodies") && msg.contains("xpath"),
        "message must name the field and the unknown key, got: {msg}"
    );
}

#[test]
fn reserved_predicate_key_rejected_bodies() {
    let err = err_of(&doc_with_body_entry(r#"{predicate: "x"}"#));
    let msg = err.to_string();
    assert!(
        matches!(err, TestDocError::InvalidMatcher(_)),
        "predicate in bodies must fail parsing, got: {err:?}"
    );
    assert!(
        msg.contains("predicate matchers are not supported"),
        "message must reject predicate matchers, got: {msg}"
    );
}

#[test]
fn reserved_predicate_key_rejected_header() {
    let err = err_of(&doc_with_expects_header(r#"{predicate: "x"}"#));
    let msg = err.to_string();
    assert!(
        matches!(err, TestDocError::InvalidMatcher(_)),
        "predicate in headers must fail parsing, got: {err:?}"
    );
    assert!(
        msg.contains("predicate matchers are not supported"),
        "message must reject predicate matchers, got: {msg}"
    );
}

#[test]
fn matcher_map_wrong_key_count_rejected() {
    for entry in ["{}", r#"{regex: "a", contains: "b"}"#] {
        let err = err_of(&doc_with_body_entry(entry));
        let msg = err.to_string();
        assert!(
            matches!(err, TestDocError::InvalidMatcher(_)),
            "body entry `{entry}` must fail parsing, got: {err:?}"
        );
        assert!(
            msg.contains("exactly one key"),
            "message must state the one-key rule for `{entry}`, got: {msg}"
        );
    }
}

#[test]
fn bare_scalar_bodies_rejected() {
    let err = err_of(&doc_with_body_entry("7"));
    let msg = err.to_string();
    assert!(
        matches!(err, TestDocError::InvalidMatcher(_)),
        "bare scalar body entry must fail parsing, got: {err:?}"
    );
    assert!(
        msg.contains("bodies entries must be strings or matcher maps"),
        "message must state the entry shape rule, got: {msg}"
    );
}

#[test]
fn bare_array_bodies_rejected() {
    let err = err_of(&doc_with_body_entry("[1, 2]"));
    let msg = err.to_string();
    assert!(
        matches!(err, TestDocError::InvalidMatcher(_)),
        "bare array body entry must fail parsing, got: {err:?}"
    );
    assert!(
        msg.contains("bodies entries must be strings or matcher maps"),
        "message must state the entry shape rule, got: {msg}"
    );
}

#[test]
fn exists_non_null_payload_rejected_bodies() {
    let err = err_of(&doc_with_body_entry(r#"{exists: "x"}"#));
    let msg = err.to_string();
    assert!(
        matches!(err, TestDocError::InvalidMatcher(_)),
        "exists with a payload must fail parsing, got: {err:?}"
    );
    assert!(
        msg.contains("`exists` takes no argument"),
        "message must reject the exists payload, got: {msg}"
    );
}

#[test]
fn exists_non_null_payload_rejected_header() {
    let err = err_of(&doc_with_expects_header(r#"{exists: "y"}"#));
    let msg = err.to_string();
    assert!(
        matches!(err, TestDocError::InvalidMatcher(_)),
        "header exists with a payload must fail parsing, got: {err:?}"
    );
    assert!(
        msg.contains("`exists` takes no argument"),
        "message must reject the exists payload, got: {msg}"
    );
}

#[test]
fn invalid_regex_rejected_at_parse_bodies() {
    let err = err_of(&doc_with_body_entry(r#"{regex: "(unclosed"}"#));
    let msg = err.to_string();
    assert!(
        matches!(err, TestDocError::InvalidMatcher(_)),
        "expected InvalidMatcher, got: {err:?}"
    );
    assert!(
        msg.contains("bodies") && msg.contains("regex"),
        "message must name the field and the regex error, got: {msg}"
    );
    assert!(
        msg.contains("unclosed group"),
        "message must contain regex compile error, got: {msg}"
    );
}

#[test]
fn invalid_regex_rejected_at_parse_header() {
    let err = err_of(&doc_with_expects_header(r#"{regex: "(unclosed"}"#));
    let msg = err.to_string();
    assert!(
        matches!(err, TestDocError::InvalidMatcher(ref m) if m.contains("regex")),
        "invalid header regex must fail parsing, got: {err:?}"
    );
    assert!(
        msg.contains("unclosed group"),
        "message must contain regex compile error, got: {msg}"
    );
}

#[test]
fn json_subset_non_object_rejected() {
    let err = err_of(&doc_with_body_entry(r#"{jsonSubset: [1, 2]}"#));
    let msg = err.to_string();
    assert!(
        matches!(err, TestDocError::InvalidMatcher(_)),
        "non-object jsonSubset must fail parsing, got: {err:?}"
    );
    assert!(
        msg.contains("must be an object"),
        "message must require an object payload, got: {msg}"
    );
}

#[test]
fn json_subset_on_header_rejected() {
    // expects.headers position.
    let err = err_of(&doc_with_expects_header(r#"{jsonSubset: {a: 1}}"#));
    let msg = err.to_string();
    assert!(
        matches!(err, TestDocError::InvalidMatcher(_)),
        "jsonSubset in expects.headers must fail parsing, got: {err:?}"
    );
    assert!(
        msg.contains("applies to bodies only"),
        "message must state the bodies-only rule, got: {msg}"
    );
    // expectReply.headers position.
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    to: "mock:out"
inputs:
  - to: "direct:in"
    body: x
    expectReply:
      headers:
        X: {jsonSubset: {a: 1}}
"#;
    let err = err_of(yaml);
    let msg = err.to_string();
    assert!(
        matches!(err, TestDocError::InvalidMatcher(_)),
        "jsonSubset in expectReply.headers must fail parsing, got: {err:?}"
    );
    assert!(
        msg.contains("applies to bodies only"),
        "message must state the bodies-only rule, got: {msg}"
    );
}

#[test]
fn equals_wrapped_scalar_maps_to_json() {
    let doc = parse_test_document(&doc_with_body_entry(r#"{equals: 7}"#))
        .expect("wrapped equals should parse"); // allow-unwrap
    let bodies = doc.expects["result"]
        .bodies
        .as_ref()
        .expect("bodies present"); // allow-unwrap
    assert!(matches!(
        bodies[0],
        BodyMatcher::Equals(Body::Json(ref v)) if v == &serde_json::json!(7)
    ));
}

#[test]
fn backcompat_all_bare_documents_parse() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    to: "mock:out"
inputs:
  - to: "direct:in"
    body: x
    expectReply:
      body:
        status: ok
expects:
  mock:out:
    count: 1
    bodies:
      - plain
      - "another"
    headers:
      count: 2
      flag: "yes"
"#;
    let doc = parse_test_document(yaml).expect("backcompat document should parse"); // allow-unwrap
    let set = doc
        .expects
        .get("out")
        .expect("normalized key `out` present"); // allow-unwrap
    let bodies = set.bodies.as_ref().expect("bodies present"); // allow-unwrap
    assert!(matches!(
        bodies[0],
        BodyMatcher::Equals(Body::Text(ref s)) if s == "plain"
    ));
    assert!(matches!(
        bodies[1],
        BodyMatcher::Equals(Body::Text(ref s)) if s == "another"
    ));
    let headers = set.headers.as_ref().expect("headers present"); // allow-unwrap
    assert!(matches!(
        headers.get("count"),
        Some(HeaderMatcher::Equals(v)) if v == &serde_json::json!(2)
    ));
    assert!(matches!(
        headers.get("flag"),
        Some(HeaderMatcher::Equals(v)) if v == &serde_json::json!("yes")
    ));
    let reply = doc.inputs[0]
        .expect_reply
        .as_ref()
        .expect("expect_reply present"); // allow-unwrap
    assert!(matches!(
        reply.body,
        Some(BodyMatcher::Equals(Body::Json(ref v))) if v.get("status") == Some(&serde_json::json!("ok"))
    ));
}
