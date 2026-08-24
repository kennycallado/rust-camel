//! Integration tests for declarative repository stubs (`repositories:` block
//! in `*.test.yaml`). One document may declare named repository stubs
//! (`cache`, `idempotent`, `claimCheck`) that the runner registers in the
//! context's repository registries before routes load, so `cache:` steps
//! resolve against them at route-add time.
//!
//! Spec: openspec/changes/declarative-repository-stubs (Task 1.2).

use std::fs;
use std::path::PathBuf;

use camel_cli::commands::test::document::parse_test_document;
use camel_cli::commands::test::run_tests;
use camel_cli::commands::test::runner::{TestDocResult, run_test_doc};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "camel-test-repo-stubs-{tag}-{}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("create temp dir"); // allow-unwrap
    dir
}

fn assert_green(result: &TestDocResult, expected_endpoints: usize) {
    assert!(
        result.doc_error.is_none(),
        "doc_error: {:?}",
        result.doc_error
    );
    assert_eq!(result.endpoint_results.len(), expected_endpoints);
    for er in &result.endpoint_results {
        assert!(
            er.outcome.is_ok(),
            "endpoint {} failed: {:?}",
            er.endpoint,
            er.outcome
        );
    }
}

/// A `cache:` step naming a stub-declared repository: the first input misses
/// (runs the `on_miss` branch to `mock:miss` and writes back), the second is
/// a cache hit that skips the miss branch; both continue to `mock:out`.
#[tokio::test(flavor = "multi_thread")]
async fn cache_stub_miss_then_hit() {
    let dir = temp_dir("miss-then-hit");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - cache:
          repository: persistent
          key: k
          on_miss:
            - to: "mock:miss"
      - to: "mock:out"
inputs:
  - to: "direct:in"
    body: "first"
  - to: "direct:in"
    body: "second"
repositories:
  cache:
    persistent: memory
expects:
  mock:miss:
    count: 1
  mock:out:
    count: 2
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _) = run_test_doc(&doc, &dir).await;
    assert_green(&result, 2);
}

/// An empty `cache: {}` map declares no repository stubs: the built-in
/// `memory` repository still serves the route, the document runs green, and
/// no `R-REPOSITORY-STUB` warning is emitted.
#[tokio::test(flavor = "multi_thread")]
async fn empty_cache_map_no_warning() {
    let dir = temp_dir("empty-cache-map");
    let path = dir.join("empty.test.yaml");
    fs::write(
        &path,
        r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - cache:
          repository: memory
          key: k
          on_miss:
            - to: "mock:miss"
      - to: "mock:out"
inputs:
  - to: "direct:in"
    body: "first"
repositories:
  cache: {}
expects:
  mock:miss:
    count: 1
  mock:out:
    count: 1
"#,
    )
    .expect("write empty-cache doc"); // allow-unwrap
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[path], &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 0, "document must run green");
    let err = String::from_utf8(err).expect("stderr utf-8"); // allow-unwrap
    assert!(
        !err.contains("R-REPOSITORY-STUB"),
        "err must NOT carry the stub warning: {err}"
    );
}

/// A `cache:` step naming a repository the document does NOT stub fails route
/// load with the same unknown-repository error as a run without any
/// `repositories:` block.
#[tokio::test(flavor = "multi_thread")]
async fn undeclared_repository_name_fails_route_load() {
    let dir = temp_dir("undeclared");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - cache:
          repository: persistant
          key: k
          on_miss:
            - to: "mock:miss"
      - to: "mock:out"
inputs:
  - to: "direct:in"
    body: "first"
repositories:
  cache:
    persistent: memory
expects:
  mock:miss:
    count: 1
  mock:out:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _) = run_test_doc(&doc, &dir).await;
    let err = result.doc_error.expect("doc_error must be Some"); // allow-unwrap
    assert!(
        err.contains("cache: repository 'persistant' is not registered"),
        "doc_error must name the unknown repository, got: {err}"
    );
}

/// An `idempotent_consumer:` step naming a stub-declared repository dedups by
/// the `messageId` header: two inputs with the SAME message id collapse to a
/// single `mock:out` delivery (the duplicate is filtered).
#[tokio::test(flavor = "multi_thread")]
async fn idempotent_stub_filters_duplicates() {
    let dir = temp_dir("idempotent");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - idempotent_consumer:
          repository: redis
          expression: "${header.messageId}"
          steps:
            - to: "mock:out"
inputs:
  - to: "direct:in"
    body: "first"
    headers:
      messageId: "dup-1"
  - to: "direct:in"
    body: "second"
    headers:
      messageId: "dup-1"
repositories:
  idempotent:
    redis: memory
expects:
  mock:out:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _) = run_test_doc(&doc, &dir).await;
    assert_green(&result, 1);
}

/// A document declaring a repository stub emits one `R-REPOSITORY-STUB`
/// warning line on stderr per run, naming the registry and the stubbed name.
#[tokio::test(flavor = "multi_thread")]
async fn stub_warning_emitted_per_run() {
    let dir = temp_dir("stub-warning");
    let path = dir.join("stub.test.yaml");
    fs::write(
        &path,
        r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - cache:
          repository: persistent
          key: k
          on_miss:
            - to: "mock:miss"
      - to: "mock:out"
inputs:
  - to: "direct:in"
    body: "first"
repositories:
  cache:
    persistent: memory
expects:
  mock:miss:
    count: 1
  mock:out:
    count: 1
"#,
    )
    .expect("write stub doc"); // allow-unwrap
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[path], &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 0, "document must run green");
    let err = String::from_utf8(err).expect("stderr utf-8"); // allow-unwrap
    assert!(
        err.contains("R-REPOSITORY-STUB"),
        "err must carry the stub warning: {err}"
    );
    assert!(
        err.contains("cache=persistent"),
        "err must name the stubbed registry and repository: {err}"
    );
    assert!(
        err.contains("persistent"),
        "err must name the stubbed repository: {err}"
    );
}

/// A document WITHOUT a `repositories:` block (route uses the built-in
/// `memory` repository) emits no `R-REPOSITORY-STUB` warning.
#[tokio::test(flavor = "multi_thread")]
async fn no_stub_no_warning() {
    let dir = temp_dir("no-stub");
    let path = dir.join("nostub.test.yaml");
    fs::write(
        &path,
        r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - cache:
          repository: memory
          key: k
          on_miss:
            - to: "mock:miss"
      - to: "mock:out"
inputs:
  - to: "direct:in"
    body: "first"
expects:
  mock:miss:
    count: 1
  mock:out:
    count: 1
"#,
    )
    .expect("write no-stub doc"); // allow-unwrap
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[path], &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 0, "document must run green");
    let err = String::from_utf8(err).expect("stderr utf-8"); // allow-unwrap
    assert!(
        !err.contains("R-REPOSITORY-STUB"),
        "err must NOT carry the stub warning: {err}"
    );
}
/// A claim-check register + retrieve roundtrip against a stub-declared
/// repository: the body stashed by `set` is restored by `get`, so the output
/// body equals the input body.
#[tokio::test(flavor = "multi_thread")]
async fn claimcheck_stub_roundtrip() {
    let dir = temp_dir("claimcheck");
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - claim_check:
          repository: redb
          operation: set
          key: "${header.claimKey}"
      - claim_check:
          repository: redb
          operation: get
          key: "${header.claimKey}"
      - to: "mock:out"
inputs:
  - to: "direct:in"
    body: "payload"
    headers:
      claimKey: "k1"
repositories:
  claimCheck:
    redb: memory
expects:
  mock:out:
    count: 1
    bodies:
      - "payload"
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _) = run_test_doc(&doc, &dir).await;
    assert_green(&result, 1);
}
