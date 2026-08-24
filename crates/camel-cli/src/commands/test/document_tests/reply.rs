use super::*;

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
        Some(InputBody::Text(ref s)) if s == "done"
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
        Some(InputBody::Json(ref v)) if v.get("status") == Some(&serde_json::json!("ok"))
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
    assert_eq!(headers.get("count"), Some(&serde_json::json!(2)));
    assert_eq!(headers.get("flag"), Some(&serde_json::json!("yes")));
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
        Some(InputBody::Text(ref s)) if s == "done"
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
