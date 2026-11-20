use super::*;

use camel_api::Body;
use camel_component_mock::BodyMatcher;
use serde_json::json;
use std::path::Path;

/// Valid fixture schema used by every positive case: an object requiring a
/// string `id`.
const VALID_SCHEMA: &str =
    r#"{"type":"object","required":["id"],"properties":{"id":{"type":"string"}}}"#;

/// Unit-tier document with one `bodies` entry under `mock:result`.
fn body_doc(entry: &str) -> String {
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

/// Unit-tier document with two `bodies` entries under `mock:result`.
fn two_body_doc(first: &str, second: &str) -> String {
    format!(
        r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 2
    bodies:
      - {first}
      - {second}
"#
    )
}

fn write_schema(dir: &Path, name: &str, content: &str) {
    std::fs::write(dir.join(name), content).expect("write fixture schema"); // allow-unwrap
}

fn err_at(dir: &Path, yaml: &str) -> TestDocError {
    match parse_test_document_at(yaml, dir) {
        Ok(doc) => panic!("document should fail to parse, got: {doc:?}"),
        Err(e) => e,
    }
}

/// Parses `yaml` against `dir`, requires an [`TestDocError::InvalidMatcher`],
/// and asserts the message contains every needle.
fn assert_invalid_matcher_at(dir: &Path, yaml: &str, needles: &[&str]) {
    let err = err_at(dir, yaml);
    let TestDocError::InvalidMatcher(msg) = &err else {
        panic!("expected InvalidMatcher, got: {err:?}");
    };
    for needle in needles {
        assert!(
            msg.contains(needle),
            "message must contain `{needle}`, got: {msg}"
        );
    }
}

fn first_body(doc: &TestDocument) -> &BodyMatcher {
    doc.expects
        .get("result")
        .expect("result expectation present") // allow-unwrap
        .bodies
        .as_ref()
        .expect("bodies present") // allow-unwrap
        .first()
        .expect("first body present") // allow-unwrap
}

fn json_schema_entry(payload: &str) -> String {
    format!("{{jsonSchema: {payload}}}")
}

#[test]
fn json_schema_valid_body_matches() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    write_schema(dir.path(), "order.schema.json", VALID_SCHEMA);
    let yaml = body_doc(&json_schema_entry(r#"{path: "order.schema.json"}"#));
    let doc = parse_test_document_at(&yaml, dir.path()).expect("jsonSchema body should parse"); // allow-unwrap
    let matcher = first_body(&doc);
    assert!(
        matches!(matcher, BodyMatcher::JsonSchema { .. }),
        "expected a JsonSchema matcher, got: {matcher:?}"
    );
    assert!(matcher.matches(&Body::Json(json!({"id": "x"}))));
    assert!(!matcher.matches(&Body::Json(json!({}))));
    assert!(
        format!("{matcher}").contains("order.schema.json"),
        "display must name the schema path, got: {matcher}"
    );
}

#[test]
fn json_schema_yaml_by_extension() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    // JSON is a YAML subset: the same document parses under the YAML branch.
    write_schema(dir.path(), "order.schema.yaml", VALID_SCHEMA);
    let yaml = body_doc(&json_schema_entry(r#"{path: "order.schema.yaml"}"#));
    let doc = parse_test_document_at(&yaml, dir.path()).expect("yaml schema should parse"); // allow-unwrap
    assert!(first_body(&doc).matches(&Body::Json(json!({"id": "x"}))));
}

#[test]
fn json_schema_type_override_selects_yaml_parser() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    // Bare `object`/`string` tokens are invalid JSON, so only the YAML
    // parser can read this file.
    write_schema(
        dir.path(),
        "order.schema.txt",
        "type: object\nrequired: [id]\nproperties:\n  id:\n    type: string\n",
    );
    let yaml = body_doc(&json_schema_entry(
        r#"{path: "order.schema.txt", type: "yaml"}"#,
    ));
    let doc = parse_test_document_at(&yaml, dir.path())
        .expect("type override should select the YAML parser"); // allow-unwrap
    assert!(first_body(&doc).matches(&Body::Json(json!({"id": "x"}))));
}

#[test]
fn json_schema_two_matchers_resolve_same_base() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    write_schema(dir.path(), "a.schema.json", VALID_SCHEMA);
    write_schema(dir.path(), "b.schema.json", VALID_SCHEMA);
    let yaml = two_body_doc(
        &json_schema_entry(r#"{path: "a.schema.json"}"#),
        &json_schema_entry(r#"{path: "b.schema.json"}"#),
    );
    let doc =
        parse_test_document_at(&yaml, dir.path()).expect("two jsonSchema matchers should parse"); // allow-unwrap
    let bodies = doc.expects["result"]
        .bodies
        .as_ref()
        .expect("bodies present"); // allow-unwrap
    assert_eq!(bodies.len(), 2);
    assert!(bodies[0].matches(&Body::Json(json!({"id": "x"}))));
    assert!(bodies[1].matches(&Body::Json(json!({"id": "y"}))));
    assert!(format!("{}", bodies[0]).contains("a.schema.json"));
    assert!(format!("{}", bodies[1]).contains("b.schema.json"));
}

#[test]
fn json_schema_missing_file_rejected() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    let yaml = body_doc(&json_schema_entry(r#"{path: "missing.schema.json"}"#));
    assert_invalid_matcher_at(dir.path(), &yaml, &["missing.schema.json"]);
}

#[test]
fn json_schema_unreadable_file_rejected() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    std::fs::create_dir(dir.path().join("order.schema.json"))
        .expect("create dir in place of schema"); // allow-unwrap
    let yaml = body_doc(&json_schema_entry(r#"{path: "order.schema.json"}"#));
    assert_invalid_matcher_at(dir.path(), &yaml, &["order.schema.json"]);
}

#[test]
fn json_schema_out_of_dir_ref_rejected() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    write_schema(
        dir.path(),
        "order.schema.json",
        r#"{"$ref": "../outside.json#/$defs/x"}"#,
    );
    let yaml = body_doc(&json_schema_entry(r#"{path: "order.schema.json"}"#));
    assert_invalid_matcher_at(dir.path(), &yaml, &["order.schema.json", "rejected"]);
}

#[test]
fn json_schema_missing_in_dir_ref_rejected() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    write_schema(
        dir.path(),
        "order.schema.json",
        r#"{"$ref": "defs.json#/$defs/x"}"#,
    );
    let yaml = body_doc(&json_schema_entry(r#"{path: "order.schema.json"}"#));
    assert_invalid_matcher_at(dir.path(), &yaml, &["order.schema.json"]);
}

#[test]
fn json_schema_invalid_schema_rejected() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    let yaml = body_doc(&json_schema_entry(r#"{path: "order.schema.json"}"#));
    for content in ["{not json", r#"{"maximum": "five"}"#] {
        write_schema(dir.path(), "order.schema.json", content);
        assert_invalid_matcher_at(dir.path(), &yaml, &["order.schema.json"]);
    }
}

#[test]
fn json_schema_unsupported_type_rejected() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    let yaml = body_doc(&json_schema_entry(r#"{path: "order.xsd", type: "xsd"}"#));
    assert_invalid_matcher_at(
        dir.path(),
        &yaml,
        &["bodies", "xsd", "json", "yaml", "order.xsd"],
    );
}

#[test]
fn json_schema_absolute_path_rejected() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    let yaml = body_doc(&json_schema_entry(r#"{path: "/abs/order.schema.json"}"#));
    assert_invalid_matcher_at(dir.path(), &yaml, &["/abs/order.schema.json", "relative"]);
}

#[test]
fn json_schema_unknown_field_rejected() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    write_schema(dir.path(), "order.schema.json", VALID_SCHEMA);
    let yaml = body_doc(&json_schema_entry(
        r#"{path: "order.schema.json", format: "draft7"}"#,
    ));
    assert_invalid_matcher_at(dir.path(), &yaml, &["bodies", "format", "jsonSchema"]);
}

#[test]
fn json_schema_missing_path_rejected() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    let missing_path = body_doc(&json_schema_entry(r#"{type: "json"}"#));
    assert_invalid_matcher_at(dir.path(), &missing_path, &["path"]);

    let non_object = body_doc(&json_schema_entry(r#""order.schema.json""#));
    assert_invalid_matcher_at(dir.path(), &non_object, &["object"]);
}

#[test]
fn json_schema_non_string_type_rejected() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    for payload in [
        r#"{path: "order.schema.json", type: 42}"#,
        r#"{path: "order.schema.json", type: null}"#,
    ] {
        assert_invalid_matcher_at(
            dir.path(),
            &body_doc(&json_schema_entry(payload)),
            &["type"],
        );
    }
}

#[test]
fn json_schema_on_header_rejected() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
    headers:
      X-Id:
        jsonSchema:
          path: order.schema.json
"#;
    assert_invalid_matcher_at(dir.path(), yaml, &["bodies only", "headers[X-Id]"]);
}

#[test]
fn json_schema_reply_body_grammar() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    write_schema(dir.path(), "order.schema.json", VALID_SCHEMA);
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
        jsonSchema:
          path: order.schema.json
"#;
    let doc = parse_test_document_at(yaml, dir.path()).expect("jsonSchema reply body should parse"); // allow-unwrap
    let reply = doc.inputs[0]
        .expect_reply
        .as_ref()
        .expect("expect_reply present"); // allow-unwrap
    let body = reply.body.as_ref().expect("reply body present"); // allow-unwrap
    assert!(
        matches!(body, BodyMatcher::JsonSchema { .. }),
        "expected a JsonSchema reply matcher, got: {body:?}"
    );
    assert!(body.matches(&Body::Json(json!({"id": "x"}))));
}

#[test]
fn json_schema_relative_to_doc_dir_not_cwd() {
    let root = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    let nested = root.path().join("nested");
    std::fs::create_dir(&nested).expect("create nested doc dir"); // allow-unwrap
    write_schema(&nested, "order.schema.json", VALID_SCHEMA);
    let yaml = body_doc(&json_schema_entry(r#"{path: "order.schema.json"}"#));
    // Resolution against the process CWD would not find the fixture and the
    // parse would fail; only the explicit doc dir resolves it.
    let doc = parse_test_document_at(&yaml, &nested)
        .expect("relative jsonSchema path must resolve against the doc dir"); // allow-unwrap
    let matcher = first_body(&doc);
    assert!(matcher.matches(&Body::Json(json!({"id": "x"}))));
    assert!(
        format!("{matcher}").contains("nested"),
        "display must show the doc-dir-joined path, got: {matcher}"
    );
}
