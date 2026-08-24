use super::*;

#[test]
fn beans_absent_keeps_behavior() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("doc without beans should parse"); // allow-unwrap
    assert!(doc.bean_decls().is_none());
    assert!(doc.beans.is_none());
}

#[test]
fn beans_setbody_parses() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
beans:
  enricher:
    kind: setBody
    config:
      body: stubbed
"#;
    let doc = parse_test_document(yaml).expect("valid beans doc should parse"); // allow-unwrap
    let decls = doc.bean_decls().expect("bean_decls is Some"); // allow-unwrap
    let decl = decls.get("enricher").expect("enricher present"); // allow-unwrap
    assert_eq!(decl.kind, BeanKindDoc::SetBody);
    assert_eq!(decl.methods, None);
    let config = decl.config.as_ref().expect("config present"); // allow-unwrap
    assert_eq!(config.get("body"), Some(&"stubbed".to_string()));
}

#[test]
fn beans_unknown_kind_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
beans:
  x:
    kind: teleport
"#;
    let err = err_of(yaml);
    let msg = err.to_string();
    assert!(
        msg.contains("teleport"),
        "message must name the unknown kind, got: {msg}"
    );
    for kind in ["echo", "setBody", "fail"] {
        assert!(
            msg.contains(kind),
            "message must list supported kind `{kind}`, got: {msg}"
        );
    }
}

#[test]
fn beans_blank_name_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
beans:
  "  ":
    kind: echo
"#;
    let err = err_of(yaml);
    assert!(
        matches!(
            err,
            TestDocError::InvalidBeans(ref m) if m.contains("bean names must be non-blank")
        ),
        "expected InvalidBeans for blank bean name, got: {err:?}"
    );
    assert!(err.to_string().contains("bean names must be non-blank"));
}

#[test]
fn beans_empty_methods_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
beans:
  x:
    kind: echo
    methods: []
"#;
    let err = err_of(yaml);
    assert!(
        matches!(
            err,
            TestDocError::InvalidBeans(ref m)
                if m.contains("methods must be non-empty or omitted")
        ),
        "expected InvalidBeans for empty methods, got: {err:?}"
    );
    assert!(
        err.to_string()
            .contains("methods must be non-empty or omitted")
    );
}

#[test]
fn beans_blank_method_entry_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
beans:
  x:
    kind: echo
    methods: ["a", ""]
"#;
    let err = err_of(yaml);
    assert!(
        matches!(
            err,
            TestDocError::InvalidBeans(ref m) if m.contains("method names must be non-blank")
        ),
        "expected InvalidBeans for blank methods entry, got: {err:?}"
    );
    assert!(err.to_string().contains("method names must be non-blank"));
}

#[test]
fn beans_setbody_missing_body_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
beans:
  x:
    kind: setBody
    config: {}
"#;
    let err = err_of(yaml);
    assert!(
        matches!(
            err,
            TestDocError::InvalidBeans(ref m) if m.contains("requires config key body")
        ),
        "expected InvalidBeans for setBody without body, got: {err:?}"
    );
    assert!(err.to_string().contains("requires config key body"));

    // config omitted entirely (None) hits the same guard.
    let no_config = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
beans:
  x:
    kind: setBody
"#;
    let err = err_of(no_config);
    assert!(err.to_string().contains("requires config key body"));
}

#[test]
fn beans_config_variant_pins() {
    // echo: any config key is invalid.
    let echo = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
beans:
  x:
    kind: echo
    config:
      body: y
"#;
    let err = err_of(echo);
    assert!(
        matches!(
            err,
            TestDocError::InvalidBeans(ref m) if m.contains("not valid for kind echo")
        ),
        "echo config key must be rejected, got: {err:?}"
    );
    // setBody: extra key beyond body is invalid.
    let set_body = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
beans:
  x:
    kind: setBody
    config:
      body: y
      extra: z
"#;
    let err = err_of(set_body);
    assert!(
        matches!(
            err,
            TestDocError::InvalidBeans(ref m) if m.contains("not valid for kind setBody")
        ),
        "setBody extra config key must be rejected, got: {err:?}"
    );
    // fail: only `message` is allowed.
    let fail = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
beans:
  x:
    kind: fail
    config:
      message: m
      other: o
"#;
    let err = err_of(fail);
    assert!(
        matches!(
            err,
            TestDocError::InvalidBeans(ref m) if m.contains("not valid for kind fail")
        ),
        "fail extra config key must be rejected, got: {err:?}"
    );
}

#[test]
fn beans_nested_unknown_field_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
beans:
  x:
    kind: echo
    metod: [a]
"#;
    let err = err_of(yaml);
    assert!(
        matches!(err, TestDocError::UnknownField(ref msg) if msg.contains("metod")),
        "expected UnknownField naming metod, got: {err:?}"
    );
}
