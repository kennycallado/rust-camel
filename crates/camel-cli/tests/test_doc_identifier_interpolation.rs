//! Integration tests for doc-side identifier interpolation (step (a0) of
//! [`parse_test_document`]): identifier fields interpolate
//! `${env:NAME}` / `${env:NAME:-default}` placeholders with a default-only
//! lookup — the ambient process environment is never consulted — giving
//! identifier name-match parity with route sources. Covered field-groups:
//! `repositories:` and `beans:` map keys, `intercepts:` keys and their
//! action target values, `expects:` map keys, `sequence:` entries, and
//! `inputs[].to` values.
//!
//! Task 1.3 adds the end-to-end witnesses (the doc-side resolved name
//! meets the route-side interpolated reference inside `run_test_doc`),
//! anti-widening pins (assertion data never interpolates), and non-goal
//! pins (`settle`, stub targets, route-file paths stay literal).
//!
//! These tests never read or write environment variables (`env::set_var` /
//! `env::var` are forbidden here): env-independence is asserted by never
//! touching the env API, since the lookup is default-only by construction.
//!
//! Spec: openspec/changes/doc-identifier-interpolation-parity (Tasks 1.1,
//! 1.2, and 1.3).

use std::fs;
use std::path::PathBuf;

use camel_cli::commands::test::document::InputBody;
use camel_cli::commands::test::document::TestDocError;
use camel_cli::commands::test::document::parse_test_document;
use camel_cli::commands::test::runner::{TestDocResult, run_test_doc};

fn temp_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("camel-test-doc-ident-{tag}-{}", std::process::id()));
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

/// A `repositories.cache` key with a default resolves to the default: the
/// rebuilt map carries `persistent` and no raw `${env:` placeholder.
#[test]
fn repository_key_default_resolves() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - to: "mock:out"
expects:
  mock:out:
    count: 1
repositories:
  cache:
    "${env:CACHE_REPO_NAME:-persistent}": memory
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let stubs = doc.repository_stubs().expect("repositories block declared"); // allow-unwrap
    let cache = stubs.cache.as_ref().expect("cache map declared"); // allow-unwrap
    assert!(
        cache.contains_key("persistent"),
        "cache keys must contain the resolved default `persistent`, got: {cache:?}"
    );
    assert!(
        !cache.keys().any(|key| key.contains("${env:")),
        "cache keys must not keep raw placeholders, got: {cache:?}"
    );
}

/// A `beans:` key with a default resolves to the default: the rebuilt map
/// carries `audit`.
#[test]
fn bean_key_default_resolves() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - to: "mock:out"
expects:
  mock:out:
    count: 1
beans:
  "${env:BEAN_NAME:-audit}":
    kind: echo
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let beans = doc.bean_decls().expect("beans block declared"); // allow-unwrap
    assert!(
        beans.contains_key("audit"),
        "bean names must contain the resolved default `audit`, got: {beans:?}"
    );
}

/// A `repositories.cache` key with a no-default placeholder fails parse,
/// naming the variable and the field, mirroring the route-side wording; the
/// stub target value never leaks into the message.
#[test]
fn repository_no_default_fails_naming_var() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - to: "mock:out"
expects:
  mock:out:
    count: 1
repositories:
  cache:
    "${env:NO_SUCH_VAR}": memory
"#;
    let err = parse_test_document(yaml).expect_err("unresolved placeholder must fail parse"); // allow-unwrap
    let display = err.to_string();
    match err {
        TestDocError::EnvUnresolved { var, field } => {
            assert_eq!(var, "NO_SUCH_VAR");
            assert_eq!(field, "repositories.cache");
        }
        other => panic!("expected EnvUnresolved, got: {other:?}"),
    }
    assert!(
        display.contains(
            "Environment variable 'NO_SUCH_VAR' not set (required by repositories.cache)"
        ),
        "Display must mirror the route-side wording, got: {display}"
    );
    assert!(
        !display.contains("memory"),
        "Display must not leak the stub target, got: {display}"
    );
}

/// A default that resolves to the empty string reaches the existing blank
/// guard: the resolved name is rejected as non-blank.
#[test]
fn empty_default_blank_name_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - to: "mock:out"
expects:
  mock:out:
    count: 1
repositories:
  cache:
    "${env:NAME:-}": memory
"#;
    let err = parse_test_document(yaml).expect_err("blank resolved name must be rejected"); // allow-unwrap
    let TestDocError::InvalidRepositories(msg) = err else {
        panic!("expected InvalidRepositories, got: {err:?}");
    };
    assert!(
        msg.contains("non-blank"),
        "the blank guard must see the resolved empty name, got: {msg}"
    );
}

/// A default that resolves to the built-in `memory` repository name reaches
/// the existing built-in-name guard.
#[test]
fn memory_default_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - to: "mock:out"
expects:
  mock:out:
    count: 1
repositories:
  cache:
    "${env:NAME:-memory}": memory
"#;
    let err = parse_test_document(yaml).expect_err("`memory` resolved name must be rejected"); // allow-unwrap
    let display = err.to_string();
    assert!(
        display.contains("built-in repository name"),
        "the built-in guard must see the resolved `memory` name, got: {display}"
    );
}

/// Two cache keys resolving to the same name collide: the second insertion
/// is rejected with an error naming the map and the resolved value.
#[test]
fn repository_key_collision_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - to: "mock:out"
expects:
  mock:out:
    count: 1
repositories:
  cache:
    "${env:A:-x}": memory
    x: memory
"#;
    let err = parse_test_document(yaml).expect_err("collision must fail parse"); // allow-unwrap
    let TestDocError::InvalidRepositories(msg) = err else {
        panic!("expected InvalidRepositories, got: {err:?}");
    };
    assert!(
        msg.contains("duplicate repository name"),
        "error must name the collision, got: {msg}"
    );
    assert!(
        msg.contains("`x`"),
        "error must name the resolved `x`, got: {msg}"
    );
}

/// The same collision rule for `beans:`: two keys resolving to `audit` are
/// rejected as an `InvalidBeans` error naming the resolved value.
#[test]
fn bean_key_collision_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - to: "mock:out"
expects:
  mock:out:
    count: 1
beans:
  "${env:B:-audit}":
    kind: echo
  audit:
    kind: echo
"#;
    let err = parse_test_document(yaml).expect_err("bean collision must fail parse"); // allow-unwrap
    let TestDocError::InvalidBeans(msg) = err else {
        panic!("expected InvalidBeans, got: {err:?}");
    };
    assert!(
        msg.contains("duplicate bean name"),
        "error must name the collision, got: {msg}"
    );
    assert!(
        msg.contains("`audit`"),
        "error must name the resolved `audit`, got: {msg}"
    );
}

/// An `intercepts:` source key and its `skipTo` target with defaults both
/// resolve: the rebuilt map carries `direct:archive` and the action points
/// at `mock:skipped`.
#[test]
fn intercept_source_and_target_default_resolve() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - to: "mock:out"
inputs:
  - to: "direct:in"
expects:
  mock:out:
    count: 1
intercepts:
  "direct:${env:TARGET:-archive}":
    skipTo: "mock:${env:SINK:-skipped}"
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let intercepts = doc.intercepts.as_ref().expect("intercepts block declared"); // allow-unwrap
    let action = intercepts
        .get("direct:archive")
        .expect("resolved source key `direct:archive` present"); // allow-unwrap
    assert_eq!(
        action.skip_to.as_deref(),
        Some("mock:skipped"),
        "the skipTo target must carry the resolved default, got: {action:?}"
    );
    assert!(
        !intercepts.keys().any(|key| key.contains("${env:")),
        "intercepts keys must not keep raw placeholders, got: {intercepts:?}"
    );
}

/// `expects:` keys and `sequence:` entries are `mock:` references and
/// resolve before the scheme checks: the map normalizes to bare endpoints
/// `result`/`other` and the sequence carries the same resolved names.
#[test]
fn expects_and_sequence_mock_refs_resolve() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - to: "mock:out"
expects:
  "mock:${env:EP:-result}":
    count: 1
  "mock:${env:EP2:-other}":
    count: 1
sequence:
  - "mock:${env:EP:-result}"
  - "mock:${env:EP2:-other}"
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    assert_eq!(doc.expects.len(), 2, "both expectations declared");
    assert!(
        doc.expects.contains_key("result"),
        "expects keys must contain resolved bare `result`, got: {:?}",
        doc.expects.keys().collect::<Vec<_>>()
    );
    assert!(
        doc.expects.contains_key("other"),
        "expects keys must contain resolved bare `other`, got: {:?}",
        doc.expects.keys().collect::<Vec<_>>()
    );
    let sequence = doc.sequence.as_ref().expect("sequence declared"); // allow-unwrap
    assert_eq!(
        sequence,
        &["result".to_string(), "other".to_string()],
        "sequence entries must resolve in place, got: {sequence:?}"
    );
}

/// An `inputs[].to` target with a default resolves: the input delivers to
/// `direct:start`.
#[test]
fn input_to_default_resolves() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - to: "mock:out"
inputs:
  - to: "direct:${env:IN:-start}"
expects:
  mock:out:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    assert_eq!(
        doc.inputs[0].to, "direct:start",
        "the input target must carry the resolved default, got: {:?}",
        doc.inputs[0].to
    );
}

/// An `intercepts:` key with a no-default placeholder fails parse, naming
/// the variable and the `intercepts` field.
#[test]
fn intercept_no_default_fails() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - to: "mock:out"
inputs:
  - to: "direct:in"
expects:
  mock:out:
    count: 1
intercepts:
  "${env:SRC}":
    skipTo: "mock:skipped"
"#;
    let err = parse_test_document(yaml).expect_err("unresolved placeholder must fail parse"); // allow-unwrap
    let TestDocError::EnvUnresolved { var, field } = err else {
        panic!("expected EnvUnresolved, got: {err:?}");
    };
    assert_eq!(var, "SRC");
    assert_eq!(field, "intercepts");
}

/// Two `intercepts:` keys resolving to the same source URI collide: parse
/// fails with an `InterceptInvalid` error naming the resolved source.
#[test]
fn intercept_key_collision_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - to: "mock:out"
inputs:
  - to: "direct:in"
expects:
  mock:out:
    count: 1
intercepts:
  "${env:T:-direct:archive}":
    skipTo: "mock:skipped"
  "direct:archive":
    skipTo: "mock:skipped"
"#;
    let err = parse_test_document(yaml).expect_err("collision must fail parse"); // allow-unwrap
    let TestDocError::InterceptInvalid(msg) = err else {
        panic!("expected InterceptInvalid, got: {err:?}");
    };
    assert!(
        msg.contains("duplicate source"),
        "error must name the collision, got: {msg}"
    );
    assert!(
        msg.contains("`direct:archive`"),
        "error must name the resolved source `direct:archive`, got: {msg}"
    );
}

/// Two `expects:` keys resolving to the same endpoint collide at (a0):
/// parse fails with a `Yaml` error naming the FULL resolved key (the
/// `mock:` scheme is stripped later, at step (c)).
#[test]
fn expects_collision_rejected() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - to: "mock:out"
expects:
  "mock:${env:A:-x}":
    count: 1
  "mock:x":
    count: 1
"#;
    let err = parse_test_document(yaml).expect_err("expects collision must fail parse"); // allow-unwrap
    let TestDocError::Yaml(msg) = err else {
        panic!("expected Yaml, got: {err:?}");
    };
    assert!(
        msg.contains("duplicate expectation endpoint"),
        "error must name the collision, got: {msg}"
    );
    assert!(
        msg.contains("`mock:x`"),
        "error must name the FULL resolved key `mock:x`, got: {msg}"
    );
}

/// The key-then-target order is canonical: a doc with BOTH an unresolved
/// intercept key AND an unresolved `skipTo` target reports the KEY error —
/// `field == "intercepts"` — not the target error.
#[test]
fn intercept_key_error_precedes_target_error() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - to: "mock:out"
inputs:
  - to: "direct:in"
expects:
  mock:out:
    count: 1
intercepts:
  "${env:SRC}":
    skipTo: "mock:${env:SINK}"
"#;
    let err = parse_test_document(yaml).expect_err("unresolved key must fail parse first"); // allow-unwrap
    let TestDocError::EnvUnresolved { var, field } = err else {
        panic!("expected EnvUnresolved, got: {err:?}");
    };
    assert_eq!(var, "SRC");
    assert_eq!(field, "intercepts");
}

// ---------------------------------------------------------------------------
// Task 1.3: end-to-end rc-4hexo flip, parity pins, and non-goal pins
// ---------------------------------------------------------------------------

/// The rc-4hexo repro, end to end: a route file whose `cache:` step names
/// its repository via `"${env:CACHE_REPO_NAME:-persistent}"` meets a doc
/// whose `repositories.cache` key carries the same placeholder. Honest
/// before-prediction (verified against v0.42.0 by the bd ticket): the run
/// failed at route-add with `repository 'persistent' is not registered`,
/// because the doc stub registered under the raw placeholder text while
/// the route side resolved to `persistent`. After the change both sides
/// resolve to `persistent` and the run is green.
#[tokio::test(flavor = "multi_thread")]
async fn rc4hexo_repro_repository_matches_route() {
    let dir = temp_dir("rc4hexo");
    fs::write(
        dir.join("route.yaml"),
        r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - cache:
          repository: "${env:CACHE_REPO_NAME:-persistent}"
          key: k
          on_miss:
            - to: "mock:out"
"#,
    )
    .expect("write route.yaml"); // allow-unwrap
    let yaml = r#"
routeFiles: [route.yaml]
inputs:
  - to: "direct:in"
    body: "x"
expects:
  mock:out:
    count: 1
repositories:
  cache:
    "${env:CACHE_REPO_NAME:-persistent}": memory
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _) = run_test_doc(&doc, &dir).await;
    assert_green(&result, 1);
}

/// Spec scenario "bean key default matches", run level: a route file
/// invoking a bean named via `"${env:BEAN_NAME:-audit}"` meets a doc whose
/// `beans:` key carries the same placeholder; both sides resolve to
/// `audit` and the echo stub handles the invocation (mirrors the bean
/// route scaffolding of `tests/test_beans.rs`).
#[tokio::test(flavor = "multi_thread")]
async fn bean_key_matches_route_e2e() {
    let dir = temp_dir("bean-e2e");
    fs::write(
        dir.join("route.yaml"),
        r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - bean:
          name: "${env:BEAN_NAME:-audit}"
          method: handle
      - to: "mock:out"
"#,
    )
    .expect("write route.yaml"); // allow-unwrap
    let yaml = r#"
routeFiles: [route.yaml]
inputs:
  - to: "direct:start"
    body: "x"
expects:
  mock:out:
    count: 1
beans:
  "${env:BEAN_NAME:-audit}":
    kind: echo
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _) = run_test_doc(&doc, &dir).await;
    assert_green(&result, 1);
}

/// Spec scenario "intercept source and target defaults match", run level:
/// the intercept source key and its `skipTo` target both carry defaults.
/// The route file wires `direct:${env:TARGET:-archive}` to `mock:real`
/// across a route boundary (a downstream route consumes the resolved
/// `direct:archive`); `skipTo` substitutes the sender's send URI, so the
/// exchange lands on the resolved target `mock:skipped` and the
/// downstream `mock:real` step never fires (`maxCount: 0` per the count
/// grammar; mirrors the skipTo scaffolding of `tests/test_intercepts.rs`).
#[tokio::test(flavor = "multi_thread")]
async fn intercept_applies_to_resolved_uri_e2e() {
    let dir = temp_dir("intercept-e2e");
    fs::write(
        dir.join("route.yaml"),
        r#"
routes:
  - id: sender
    from: "direct:start"
    steps:
      - to: "direct:${env:TARGET:-archive}"
  - id: downstream
    from: "direct:archive"
    steps:
      - to: "mock:real"
"#,
    )
    .expect("write route.yaml"); // allow-unwrap
    let yaml = r#"
routeFiles: [route.yaml]
inputs:
  - to: "direct:start"
    body: "x"
intercepts:
  "direct:${env:TARGET:-archive}":
    skipTo: "mock:${env:SINK:-skipped}"
expects:
  mock:skipped:
    count: 1
  mock:real:
    maxCount: 0
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    let (result, _) = run_test_doc(&doc, &dir).await;
    assert_green(&result, 2);
}

/// Spec scenario "escaped placeholder key stays literal": a doc key
/// `"$${env:CACHE_REPO_NAME:-persistent}"` survives parse with exactly one
/// `$` stripped by the shared escape grammar — the parsed cache map's
/// single key is the literal text `${env:CACHE_REPO_NAME:-persistent}`.
/// The end-to-end variant uses a route file whose `repository` field is
/// the same escaped form, so both sides keep the literal name and the
/// stub registers under it.
#[tokio::test(flavor = "multi_thread")]
async fn escaped_placeholder_key_stays_literal() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - to: "mock:out"
expects:
  mock:out:
    count: 1
repositories:
  cache:
    "$${env:CACHE_REPO_NAME:-persistent}": memory
"#;
    let doc = parse_test_document(yaml).expect("escaped document should parse"); // allow-unwrap
    let stubs = doc.repository_stubs().expect("repositories block declared"); // allow-unwrap
    let cache = stubs.cache.as_ref().expect("cache map declared"); // allow-unwrap
    let keys: Vec<_> = cache.keys().collect();
    assert_eq!(
        keys,
        vec!["${env:CACHE_REPO_NAME:-persistent}"],
        "the cache map must carry exactly the single-$ literal key, got: {cache:?}"
    );

    let dir = temp_dir("escaped-e2e");
    fs::write(
        dir.join("route.yaml"),
        r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - cache:
          repository: "$${env:CACHE_REPO_NAME:-persistent}"
          key: k
          on_miss:
            - to: "mock:out"
"#,
    )
    .expect("write route.yaml"); // allow-unwrap
    let e2e = r#"
routeFiles: [route.yaml]
inputs:
  - to: "direct:in"
    body: "x"
expects:
  mock:out:
    count: 1
repositories:
  cache:
    "$${env:CACHE_REPO_NAME:-persistent}": memory
"#;
    let doc = parse_test_document(e2e).expect("escaped e2e document should parse"); // allow-unwrap
    let (result, _) = run_test_doc(&doc, &dir).await;
    assert_green(&result, 1);
}

/// Spec scenario "assertion data stays literal" (anti-widening witness):
/// input `body`, input `headers`, `expectReply.body`, `expects` matcher
/// contents, and bean `config`/`methods` values all carry
/// `${env:...}` placeholder text verbatim — no value position interpolates.
/// The run leg witnesses the scenario's second THEN clause: the
/// placeholder-free route echoes the literal input body to `mock:out`, so
/// the run COMPARES the raw texts — the `bodies` matcher and the reply
/// matcher (which targets the echoed body's raw text) both match the
/// literal text at run time.
#[tokio::test(flavor = "multi_thread")]
async fn assertion_data_stays_literal() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - to: "mock:out"
inputs:
  - to: "direct:in"
    body: "${env:PAYLOAD:-leaked}"
    headers:
      X-T: "${env:T:-v}"
    expectReply:
      body: "${env:PAYLOAD:-leaked}"
expects:
  mock:out:
    bodies: ["${env:PAYLOAD:-leaked}"]
beans:
  failer:
    kind: fail
    config:
      message: "${env:MSG:-m}"
    methods: ["${env:M:-run}"]
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    match &doc.inputs[0].body {
        Some(InputBody::Text(text)) => {
            assert_eq!(
                text, "${env:PAYLOAD:-leaked}",
                "the input body must stay the raw text"
            );
        }
        other => panic!("expected InputBody::Text, got: {other:?}"),
    }
    let headers = doc.inputs[0].headers.as_ref().expect("headers declared"); // allow-unwrap
    assert!(
        format!("{:?}", headers.get("X-T")).contains("${env:T:-v}"),
        "the header value must still carry the raw text, got: {headers:?}"
    );
    let reply = doc.inputs[0]
        .expect_reply
        .as_ref()
        .expect("expectReply declared"); // allow-unwrap
    let reply_body = reply.body.as_ref().expect("reply body matcher declared"); // allow-unwrap
    assert!(
        format!("{reply_body:?}").contains("${env:PAYLOAD:-leaked}"),
        "the reply body matcher must still target the raw text, got: {reply_body:?}"
    );
    let set = doc.expects.values().next().expect("expects declared"); // allow-unwrap
    let bodies = set.bodies.as_ref().expect("bodies matcher declared"); // allow-unwrap
    assert!(
        format!("{:?}", bodies[0]).contains("${env:PAYLOAD:-leaked}"),
        "the bodies matcher must still target the raw text, got: {:?}",
        bodies[0]
    );
    let beans = doc.bean_decls().expect("beans block declared"); // allow-unwrap
    let failer = beans.get("failer").expect("bean `failer` declared"); // allow-unwrap
    let config = failer.config.as_ref().expect("bean config declared"); // allow-unwrap
    assert_eq!(
        config.get("message"),
        Some(&"${env:MSG:-m}".to_string()),
        "the bean config value must stay the raw text, got: {config:?}"
    );
    let methods = failer.methods.as_ref().expect("bean methods declared"); // allow-unwrap
    assert_eq!(
        methods,
        &["${env:M:-run}".to_string()],
        "the bean methods entries must stay raw text, got: {methods:?}"
    );

    // Run leg (second THEN clause): the placeholder-free route echoes the
    // literal input body to `mock:out`, and the run compares the raw texts
    // — the `bodies` matcher AND the reply matcher (against the echoed
    // body) both pass on the literal text. Two endpoint rows: `mock:out`
    // plus the `reply[0]` row.
    let dir = temp_dir("assertion-literal");
    let (result, _) = run_test_doc(&doc, &dir).await;
    assert_green(&result, 2);
}

/// `settle:` is assertion data, not an identifier: the raw placeholder
/// text is not a duration, so parse fails with `SettleOutOfRange` —
/// interpolation never rescues `settle`. Regression pin: fails identically
/// before AND after the change.
#[test]
fn settle_placeholder_not_interpolated() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - to: "mock:out"
expects:
  mock:out:
    count: 1
settle: "${env:S:-500ms}"
"#;
    let err = parse_test_document(yaml).expect_err("raw placeholder must not become a duration"); // allow-unwrap
    let TestDocError::SettleOutOfRange(raw) = err else {
        panic!("expected SettleOutOfRange, got: {err:?}");
    };
    assert_eq!(
        raw, "${env:S:-500ms}",
        "the error must carry the raw text, not a substituted duration"
    );
}

/// Stub targets are values, not identifiers: a raw `${env:...}` target
/// reaches the existing target guard and is rejected as unsupported,
/// naming the RAW text. Regression pin: fails identically before AND
/// after the change.
#[test]
fn stub_target_not_interpolated() {
    let yaml = r#"
routes:
  - id: r1
    from: "direct:in"
    steps:
      - to: "mock:out"
expects:
  mock:out:
    count: 1
repositories:
  cache:
    ok-name: "${env:TGT:-memory}"
"#;
    let err = parse_test_document(yaml).expect_err("raw target text must stay unsupported"); // allow-unwrap
    let TestDocError::InvalidRepositories(msg) = err else {
        panic!("expected InvalidRepositories, got: {err:?}");
    };
    assert!(
        msg.contains("unsupported stub target"),
        "the target guard must reject the raw text, got: {msg}"
    );
    assert!(
        msg.contains("${env:TGT:-memory}"),
        "the error must name the RAW target text, got: {msg}"
    );
}

/// Spec scenario "route file path never interpolates" (non-goal pin).
/// (i) `routeFiles` entries parse verbatim and the run fails to open the
/// literal path — no substitution happened. (ii) `routeFilesFromRoot`
/// entries parse verbatim; root resolution is run-level and needs a
/// `Camel.toml`, so the parse-level pin suffices for the non-goal.
#[tokio::test(flavor = "multi_thread")]
async fn route_files_paths_never_interpolate() {
    let yaml = r#"
routeFiles: ["${env:ROUTE_DIR:-routes}/demo.yaml"]
expects:
  mock:out:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    assert_eq!(
        doc.route_files.as_ref().expect("routeFiles declared"), // allow-unwrap
        &["${env:ROUTE_DIR:-routes}/demo.yaml".to_string()],
        "routeFiles must carry the literal entry"
    );
    let dir = temp_dir("routefiles-literal");
    let (result, _) = run_test_doc(&doc, &dir).await;
    let err = result.doc_error.expect("route load must fail"); // allow-unwrap
    assert!(
        err.contains("${env:ROUTE_DIR:-routes}/demo.yaml"),
        "the run must fail naming the literal path, got: {err}"
    );

    let yaml = r#"
routeFilesFromRoot: ["${env:ROOT_DIR:-cfg}/r.yaml"]
expects:
  mock:out:
    count: 1
"#;
    let doc = parse_test_document(yaml).expect("document should parse"); // allow-unwrap
    assert_eq!(
        doc.route_files_from_root
            .as_ref()
            .expect("routeFilesFromRoot declared"), // allow-unwrap
        &["${env:ROOT_DIR:-cfg}/r.yaml".to_string()],
        "routeFilesFromRoot must carry the literal entry"
    );
}
