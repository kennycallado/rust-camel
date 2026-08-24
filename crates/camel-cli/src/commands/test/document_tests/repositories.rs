use super::*;

#[test]
fn repos_absent_keeps_behavior() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("doc without repositories should parse"); // allow-unwrap
    assert!(doc.repository_stubs().is_none());
}

#[test]
fn repos_cache_parses() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
repositories:
  cache:
    persistent: memory
"#;
    let doc = parse_test_document(yaml).expect("repositories doc should parse"); // allow-unwrap
    let stubs = doc.repository_stubs().expect("repository_stubs is Some"); // allow-unwrap
    let cache = stubs.cache.as_ref().expect("cache map present"); // allow-unwrap
    assert_eq!(cache.get("persistent"), Some(&"memory".to_string()));
}

#[test]
fn repos_unknown_registry_kind_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
repositories:
  blob:
    x: memory
"#;
    let err = err_of(yaml);
    assert!(
        matches!(
            err,
            TestDocError::InvalidRepositories(ref m)
                if m.contains("blob")
                    && m.contains("cache")
                    && m.contains("idempotent")
                    && m.contains("claimCheck")
        ),
        "expected InvalidRepositories naming blob and listing supported kinds, got: {err:?}"
    );
}

#[test]
fn repos_unknown_target_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
repositories:
  cache:
    persistent: rocksdb
"#;
    let err = err_of(yaml);
    assert!(
        matches!(
            err,
            TestDocError::InvalidRepositories(ref m)
                if m.contains("rocksdb") && m.contains("memory")
        ),
        "expected InvalidRepositories naming rocksdb and target memory, got: {err:?}"
    );
}

#[test]
fn repos_blank_name_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
repositories:
  cache:
    "  ": memory
"#;
    let err = err_of(yaml);
    assert!(
        matches!(
            err,
            TestDocError::InvalidRepositories(ref m) if m.contains("non-blank")
        ),
        "expected InvalidRepositories for blank repository name, got: {err:?}"
    );
    assert!(err.to_string().contains("non-blank"));
}

#[test]
fn repos_builtin_memory_name_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
repositories:
  cache:
    memory: memory
"#;
    let err = err_of(yaml);
    assert!(
        matches!(
            err,
            TestDocError::InvalidRepositories(ref m)
                if m.contains("memory") && m.contains("built-in")
        ),
        "expected InvalidRepositories stating memory is built-in, got: {err:?}"
    );
}

#[test]
fn repos_unknown_registry_kind_singular() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
repositories:
  blob:
    x: memory
"#;
    let err = err_of(yaml);
    let msg = err.to_string();
    assert!(
        msg.contains("unknown registry kind `blob`"),
        "singular form must be `unknown registry kind` for one key, got: {msg}"
    );
    assert!(
        !msg.contains("unknown registry kinds"),
        "singular must not use plural form, got: {msg}"
    );
}

#[test]
fn repos_unknown_registry_kinds_pluralized() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    count: 1
repositories:
  blob:
    x: memory
  frobniz:
    y: memory
"#;
    let err = err_of(yaml);
    let msg = err.to_string();
    assert!(
        msg.contains("unknown registry kinds"),
        "plural form must be `unknown registry kinds` for multiple keys, got: {msg}"
    );
    assert!(
        msg.contains("`blob`") && msg.contains("`frobniz`"),
        "both unknown kinds must be listed, got: {msg}"
    );
}
