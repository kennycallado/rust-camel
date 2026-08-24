use super::*;

use camel_api::Body;
use camel_component_mock::{BodyMatcher, HeaderMatcher};

#[test]
fn expect_reply_absent_keeps_behavior() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
inputs:
  - to: "direct:start"
    body: x
expects:
  mock:result:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("doc without expectReply should parse"); // allow-unwrap
    assert!(doc.inputs[0].expect_reply.is_none());
}

#[test]
fn expect_reply_body_parses() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
inputs:
  - to: "direct:start"
    body: x
    expectReply:
      body: done
expects:
  mock:result:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("expectReply with body should parse"); // allow-unwrap
    let reply = doc.inputs[0]
        .expect_reply
        .as_ref()
        .expect("expect_reply present"); // allow-unwrap
    assert!(matches!(
        reply.body,
        Some(BodyMatcher::Equals(Body::Text(ref s))) if s == "done"
    ));
    assert!(reply.headers.is_none());
}

#[test]
fn expect_reply_json_body_parses() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
inputs:
  - to: "direct:start"
    body: x
    expectReply:
      body:
        status: ok
expects:
  mock:result:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("expectReply with JSON body should parse"); // allow-unwrap
    let reply = doc.inputs[0]
        .expect_reply
        .as_ref()
        .expect("expect_reply present"); // allow-unwrap
    assert!(matches!(
        reply.body,
        Some(BodyMatcher::Equals(Body::Json(ref v))) if v.get("status") == Some(&serde_json::json!("ok"))
    ));
}

#[test]
fn expect_reply_headers_parse_json_values() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
inputs:
  - to: "direct:start"
    body: x
    expectReply:
      headers:
        count: 2
        flag: "yes"
expects:
  mock:result:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("expectReply with headers should parse"); // allow-unwrap
    let reply = doc.inputs[0]
        .expect_reply
        .as_ref()
        .expect("expect_reply present"); // allow-unwrap
    let headers = reply.headers.as_ref().expect("headers present"); // allow-unwrap
    assert!(matches!(headers.get("count"),
        Some(HeaderMatcher::Equals(v)) if v == &serde_json::json!(2)));
    assert!(matches!(headers.get("flag"),
        Some(HeaderMatcher::Equals(v)) if v == &serde_json::json!("yes")));
    assert!(reply.body.is_none());
}

#[test]
fn expect_reply_empty_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
inputs:
  - to: "direct:start"
    body: x
    expectReply: {}
expects:
  mock:result:
    count: 1
"#;
    let err = err_of(yaml);
    assert!(
        matches!(
            err,
            TestDocError::InvalidReply(ref m)
                if m.contains("expectReply must declare body or headers")
        ),
        "expected InvalidReply for empty expectReply, got: {err:?}"
    );
    assert!(
        err.to_string()
            .contains("expectReply must declare body or headers")
    );
}

#[test]
fn expect_reply_unknown_field_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
inputs:
  - to: "direct:start"
    body: x
    expectReply:
      body: "x"
      bodi: "y"
expects:
  mock:result:
    count: 1
"#;
    let err = err_of(yaml);
    assert!(
        matches!(err, TestDocError::UnknownField(ref msg) if msg.contains("bodi")),
        "expected UnknownField naming bodi, got: {err:?}"
    );
}

#[test]
fn reply_only_document_valid() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    to: "mock:out"
inputs:
  - to: "direct:in"
    body: x
    expectReply:
      body: done
"#;
    let doc = parse_test_document(yaml).expect("reply-only doc without expects should parse"); // allow-unwrap
    assert!(doc.expects.is_empty());
    let reply = doc.inputs[0]
        .expect_reply
        .as_ref()
        .expect("expect_reply present"); // allow-unwrap
    assert!(matches!(
        reply.body,
        Some(BodyMatcher::Equals(Body::Text(ref s))) if s == "done"
    ));
}

#[test]
fn no_expects_no_expect_reply_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
inputs:
  - to: "direct:start"
    body: x
"#;
    let err = err_of(yaml);
    assert!(
        matches!(err, TestDocError::ExpectsEmpty),
        "expected ExpectsEmpty, got: {err:?}"
    );
    assert!(
        err.to_string()
            .contains("unless an input declares expectReply"),
        "mandatory message must mention the expectReply relaxation, got: {err}"
    );
}

// --- mock-matchers reply matcher tests (ported from monolith during rebase) ---

fn doc_with_reply_body(body: &str) -> String {
    format!(
        r#"
routes:
  - id: r1
    from: "direct:in"
    to: "mock:out"
inputs:
  - to: "direct:in"
    body: x
    expectReply:
      body: {body}
"#
    )
}

#[test]
fn reserved_predicate_key_rejected_reply_body() {
    let err = err_of(&doc_with_reply_body(r#"{predicate: "x"}"#));
    let msg = err.to_string();
    assert!(
        matches!(err, TestDocError::InvalidMatcher(_)),
        "predicate in reply body must fail parsing, got: {err:?}"
    );
    assert!(
        msg.contains("predicate matchers are not supported"),
        "message must reject predicate matchers, got: {msg}"
    );
}

#[test]
fn reserved_predicate_key_rejected_reply_header() {
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
        X: {predicate: "x"}
"#;
    let err = err_of(yaml);
    let msg = err.to_string();
    assert!(
        matches!(err, TestDocError::InvalidMatcher(_)),
        "predicate in reply headers must fail parsing, got: {err:?}"
    );
    assert!(
        msg.contains("predicate matchers are not supported"),
        "message must reject predicate matchers, got: {msg}"
    );
}

#[test]
fn reply_body_object_without_matcher_keys_literal() {
    let doc = parse_test_document(&doc_with_reply_body("{status: \"ok\"}"))
        .expect("object reply body should parse as literal"); // allow-unwrap
    let reply = doc.inputs[0]
        .expect_reply
        .as_ref()
        .expect("expect_reply present"); // allow-unwrap
    assert!(matches!(
        reply.body,
        Some(BodyMatcher::Equals(Body::Json(ref v))) if v.get("status") == Some(&serde_json::json!("ok"))
    ));
}

#[test]
fn reply_body_scalar_literals() {
    let doc = parse_test_document(&doc_with_reply_body("7"))
        .expect("numeric reply body should parse as literal"); // allow-unwrap
    let reply = doc.inputs[0]
        .expect_reply
        .as_ref()
        .expect("expect_reply present"); // allow-unwrap
    assert!(matches!(
        reply.body,
        Some(BodyMatcher::Equals(Body::Json(ref v))) if *v == serde_json::json!(7)
    ));
    let doc = parse_test_document(&doc_with_reply_body("null"))
        .expect("null reply body should parse as literal"); // allow-unwrap
    let reply = doc.inputs[0]
        .expect_reply
        .as_ref()
        .expect("expect_reply present"); // allow-unwrap
    assert!(matches!(
        reply.body,
        Some(BodyMatcher::Equals(Body::Json(ref v))) if v.is_null()
    ));
}

#[test]
fn reply_body_matcher_map() {
    let doc = parse_test_document(&doc_with_reply_body(r#"{regex: "^order-"}"#))
        .expect("reply body matcher map should parse"); // allow-unwrap
    let reply = doc.inputs[0]
        .expect_reply
        .as_ref()
        .expect("expect_reply present"); // allow-unwrap
    assert!(matches!(
        reply.body,
        Some(BodyMatcher::Regex(ref p)) if p == "^order-"
    ));
}

#[test]
fn reply_body_multi_key_predicate_literal() {
    let doc = parse_test_document(&doc_with_reply_body(r#"{predicate: "x", mode: "y"}"#))
        .expect("multi-key reply body should parse as literal"); // allow-unwrap
    let reply = doc.inputs[0]
        .expect_reply
        .as_ref()
        .expect("expect_reply present"); // allow-unwrap
    // A multi-key object never selects a matcher; it stays literal equals,
    // even when one of the keys is the reserved `predicate`.
    assert!(matches!(
        reply.body,
        Some(BodyMatcher::Equals(Body::Json(ref v)))
            if v.get("predicate") == Some(&serde_json::json!("x"))
                && v.get("mode") == Some(&serde_json::json!("y"))
    ));
}

#[test]
fn invalid_regex_rejected_at_parse_reply_body() {
    let err = err_of(&doc_with_reply_body(r#"{regex: "(unclosed"}"#));
    let msg = err.to_string();
    assert!(
        matches!(err, TestDocError::InvalidMatcher(ref m) if m.contains("regex")),
        "invalid reply-body regex must fail parsing, got: {err:?}"
    );
    assert!(
        msg.contains("unclosed group"),
        "message must contain regex compile error, got: {msg}"
    );
}
