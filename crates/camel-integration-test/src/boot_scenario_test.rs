//! boot_scenario delegation tests (scenario-shared-boot task 3.1).
//!
//! Unit-test module of the lib target, declared in `src/lib.rs` under
//! `#[cfg(test)]`. Every test is project-based: it writes a temporary
//! project (`Camel.toml`, an optional route file, a `.test.yaml`
//! document parsed through [`crate::parse_scenario_document`]) and
//! boots it through [`crate::boot_scenario`] with an empty
//! [`LayeredEnv`]. Every rejection path here fires before
//! `ctx.start()` (tier security gating, route-file pre-checks,
//! discovery), so no listener teardown is needed.

use std::collections::BTreeMap;

use camel_api::CamelError;

use crate::boot_scenario::boot_scenario;
use crate::env_layers::{LayeredEnv, ambient_std};
use crate::parse_scenario_document;
use crate::{RouteSource, ScenarioAction, ScenarioDocument};

/// A minimal route file: one `direct:` → `log:` route.
const ROUTE: &str = r#"
routes:
  - id: boot-route
    from: direct:start
    steps:
      - to: log:info
"#;

/// A minimal scenario document declaring the route file.
const DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
  - sleep:
      duration: 1s
"#;

/// Writes a temporary project: `Camel.toml` from `camel_toml`, an
/// optional route file `(name, text)`, and the scenario document
/// `doc`. Returns the project dir and the parsed document.
fn project(
    camel_toml: &str,
    route: Option<(&str, &str)>,
    doc: &str,
) -> (tempfile::TempDir, ScenarioDocument) {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("Camel.toml"), camel_toml).expect("write Camel.toml");
    if let Some((name, text)) = route {
        std::fs::write(dir.path().join(name), text).expect("write route file");
    }
    let doc_path = dir.path().join("case.test.yaml");
    std::fs::write(&doc_path, doc).expect("write document");
    let document = parse_scenario_document(&doc_path).expect("document parses");
    (dir, document)
}

/// An empty layered environment: no doc, harness, or passthrough
/// layer, so ambient values can never resolve through it.
fn empty_env() -> LayeredEnv {
    LayeredEnv::new(BTreeMap::new(), BTreeMap::new(), Vec::new(), ambient_std())
}

/// Extracts the error from a rejected boot; panics with `what` when
/// the boot unexpectedly succeeded.
fn expect_boot_error(
    result: Result<crate::boot_scenario::ScenarioRun, CamelError>,
    what: &str,
) -> CamelError {
    match result {
        Ok(_) => panic!("{what}"),
        Err(err) => err,
    }
}

#[tokio::test]
async fn boot_rejects_keycloak_offline() {
    // The keycloak fixture field set from camel-bundles'
    // security_boot tests; the fixture's `{{env:KC}}` secret marker is
    // rejected by the sealed config loader's legacy-brace gate before
    // the tier gate, so the secret is literal here — the gate keys on
    // the section's presence, never on the secret.
    let (dir, doc) = project(
        r#"
[security.keycloak]
server_url = "https://kc.example.com"
realm = "camel"
client_id = "camel-api"
client_secret = "kc-secret"
"#,
        Some(("routes.yaml", ROUTE)),
        DOC,
    );
    let err = expect_boot_error(
        boot_scenario(&doc, dir.path(), &empty_env()).await,
        "keycloak security must be rejected in the offline tier",
    );
    assert!(
        matches!(err, CamelError::AuthProviderUnavailable(_)),
        "expected AuthProviderUnavailable, got: {err:?}"
    );
}

#[tokio::test]
async fn boot_rejects_oidc_offline() {
    // Minimal `[security.oidc]`: `issuer` is OidcSecurityConfig's only
    // required field.
    let (dir, doc) = project(
        r#"
[security.oidc]
issuer = "https://oidc.example.test"
"#,
        Some(("routes.yaml", ROUTE)),
        DOC,
    );
    let err = expect_boot_error(
        boot_scenario(&doc, dir.path(), &empty_env()).await,
        "oidc security must be rejected in the offline tier",
    );
    assert!(
        matches!(err, CamelError::AuthProviderUnavailable(_)),
        "expected AuthProviderUnavailable, got: {err:?}"
    );
}

#[tokio::test]
async fn boot_rejects_wasm_security_policies() {
    // Minimal `[security.policies]` entry: the wrapper's only field is
    // the `wasm` map; one policy with its required `path`. The gate
    // fires before any builder call, so the .wasm file need not exist.
    let (dir, doc) = project(
        r#"
[security.policies.wasm.demo]
path = "policies/demo.wasm"
"#,
        Some(("routes.yaml", ROUTE)),
        DOC,
    );
    let err = expect_boot_error(
        boot_scenario(&doc, dir.path(), &empty_env()).await,
        "wasm security policies must be rejected in the scenario tier",
    );
    assert!(
        matches!(err, CamelError::Config(_)),
        "expected Config, got: {err:?}"
    );
    assert!(
        err.to_string()
            .contains("not supported in the scenario tier"),
        "error must name the v1 tier limitation: {err}"
    );
}

#[tokio::test]
async fn boot_hermetic_env_not_resolved_from_process() {
    // The variable exists only in the process environment; the empty
    // layered environment must not resolve it (edition-2024 unsafe
    // env APIs, same wrapping as the camel-config properties tests).
    unsafe { std::env::set_var("RC_6BSF_HERMETIC", "leak") };
    let route = r#"
routes:
  - id: hermetic
    from: direct:${env:RC_6BSF_HERMETIC}
    steps:
      - to: log:info
"#;
    let (dir, doc) = project("# minimal\n", Some(("routes.yaml", route)), DOC);
    let result = boot_scenario(&doc, dir.path(), &empty_env()).await;
    unsafe { std::env::remove_var("RC_6BSF_HERMETIC") };
    let err = expect_boot_error(
        result,
        "the process environment must not resolve scenario placeholders",
    );
    assert!(
        matches!(err, CamelError::Config(_)),
        "expected the unresolved-placeholder Config error, got: {err:?}"
    );
    let display = err.to_string();
    assert!(
        display.contains("RC_6BSF_HERMETIC"),
        "error must name the variable: {display}"
    );
    assert!(
        display.contains("no layer of the scenario environment defines it"),
        "error must name the hermetic resolution failure: {display}"
    );
}

#[tokio::test]
async fn boot_missing_route_file_names_the_file() {
    let doc = r#"
routeFiles: [nope.yaml]
scenario:
  - sleep:
      duration: 1s
"#;
    let (dir, doc) = project("# minimal\n", None, doc);
    let err = expect_boot_error(
        boot_scenario(&doc, dir.path(), &empty_env()).await,
        "a missing declared route file must fail the boot",
    );
    assert!(
        matches!(err, CamelError::Io(_)),
        "expected Io, got: {err:?}"
    );
    assert!(
        err.to_string().contains("nope.yaml"),
        "error must name the missing file: {err}"
    );
}

/// A nested scenario document declaring its route file relative to
/// itself: the file lives next to the document, not at the root.
const NESTED_DOC: &str = r#"
routeFiles: [local.yaml]
scenario:
  - sleep:
      duration: 1s
"#;

/// A nested scenario document declaring its route file against the
/// project root's route space (`rr/` under the `Camel.toml` dir).
const NESTED_FROM_ROOT_DOC: &str = r#"
routeFilesFromRoot: [rr/root-route.yaml]
scenario:
  - sleep:
      duration: 1s
"#;

#[tokio::test]
async fn boot_root_walks_to_ancestor() {
    // rc-jjzy5, nested tree: the sealed `Camel.toml` and the
    // `routeFilesFromRoot` file live at the ANCESTOR root; the
    // relative-routeFiles file lives only next to the document. One
    // document declares exactly one route source
    // (`RouteSourceConflict`), so the pair boots twice: each `Ok`
    // proves its declared file was found and parsed at its own anchor
    // — the document directory for `routeFiles`, the passed root for
    // `routeFilesFromRoot`. Anchoring `routeFiles` at the root (the
    // pre-fix behavior) misses `local.yaml` and fails the first boot.
    let root = tempfile::tempdir().expect("temp dir");
    let sub = root.path().join("sub");
    std::fs::create_dir(&sub).expect("mkdir sub");
    std::fs::create_dir_all(root.path().join("rr")).expect("mkdir rr");
    std::fs::write(root.path().join("Camel.toml"), "# minimal\n").expect("write Camel.toml");
    std::fs::write(root.path().join("rr/root-route.yaml"), ROUTE).expect("write root route");
    std::fs::write(sub.join("local.yaml"), ROUTE).expect("write local route");

    let files_path = sub.join("doc.test.yaml");
    std::fs::write(&files_path, NESTED_DOC).expect("write document");
    let files_doc = parse_scenario_document(&files_path).expect("document parses");

    let from_root_path = sub.join("from-root.test.yaml");
    std::fs::write(&from_root_path, NESTED_FROM_ROOT_DOC).expect("write document");
    let from_root_doc = parse_scenario_document(&from_root_path).expect("document parses");

    boot_scenario(&files_doc, root.path(), &empty_env())
        .await
        .expect("relative routeFiles must anchor at the document directory");
    boot_scenario(&from_root_doc, root.path(), &empty_env())
        .await
        .expect("routeFilesFromRoot must follow the resolved root");
}

#[tokio::test]
async fn route_files_anchor_to_doc_dir() {
    // rc-jjzy5: the route file exists ONLY next to the nested
    // document; resolving relative `routeFiles` against the boot root
    // would miss it and fail the boot.
    let root = tempfile::tempdir().expect("temp dir");
    let sub = root.path().join("sub");
    std::fs::create_dir(&sub).expect("mkdir sub");
    std::fs::write(root.path().join("Camel.toml"), "# minimal\n").expect("write Camel.toml");
    std::fs::write(sub.join("local.yaml"), ROUTE).expect("write local route");
    let doc_path = sub.join("case.test.yaml");
    std::fs::write(&doc_path, NESTED_DOC).expect("write document");
    let doc = parse_scenario_document(&doc_path).expect("document parses");

    boot_scenario(&doc, root.path(), &empty_env())
        .await
        .expect("the colocated route file must load from the document directory");
}

#[tokio::test]
async fn boot_inline_routes_still_rejected() {
    // The load-time doc gate (rc-9dpx) rejects inline route sources
    // inside `parse_scenario_document`, so the boot-level rejection is
    // reachable only through a directly-constructed document — exactly
    // the defense-in-depth path this test keeps covered.
    let routes = camel_dsl::parse_yaml(
        "routes:\n  - id: inline-route\n    from: direct:start\n    steps:\n      - to: log:info\n",
    )
    .expect("inline routes parse");
    let doc = ScenarioDocument {
        source_path: std::path::PathBuf::new(),
        route_source: RouteSource::Inline(routes),
        scenario: vec![ScenarioAction::Sleep {
            duration: std::time::Duration::from_secs(1),
        }],
        partners: None,
        env: None,
        env_passthrough: None,
        profile: None,
        send_deadline: None,
        inbound: None,
        logs: None,
    };
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("Camel.toml"), "# minimal\n").expect("write Camel.toml");
    let err = expect_boot_error(
        boot_scenario(&doc, dir.path(), &empty_env()).await,
        "inline route sources must not boot in v1",
    );
    assert!(
        matches!(err, CamelError::Config(_)),
        "expected Config, got: {err:?}"
    );
    assert!(
        err.to_string()
            .contains("inline route sources cannot boot in v1"),
        "error must keep the inline-routes message: {err}"
    );
}

/// The `inbound:` rejection without the `http` feature, mirroring the
/// inline-routes defense-in-depth path.
#[cfg(not(feature = "http"))]
#[tokio::test]
async fn boot_rejects_inbound_without_http_feature() {
    // The load-time doc gate rejects `inbound:` without the `http`
    // feature inside `parse_scenario_document`, so the boot-level
    // rejection is reachable only through a directly-constructed
    // document — the same defense-in-depth path
    // `boot_inline_routes_still_rejected` keeps covered.
    let doc = ScenarioDocument {
        source_path: std::path::PathBuf::new(),
        route_source: RouteSource::RouteFiles(vec!["routes.yaml".into()]),
        scenario: vec![ScenarioAction::Sleep {
            duration: std::time::Duration::from_secs(1),
        }],
        partners: None,
        env: None,
        env_passthrough: None,
        profile: None,
        send_deadline: None,
        inbound: Some(crate::InboundListener {
            bind_var: "INBOUND".to_string(),
        }),
        logs: None,
    };
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("Camel.toml"), "# minimal\n").expect("write Camel.toml");
    std::fs::write(dir.path().join("routes.yaml"), ROUTE).expect("write route file");
    let err = expect_boot_error(
        boot_scenario(&doc, dir.path(), &empty_env()).await,
        "an inbound document must not boot without the http feature",
    );
    assert!(
        matches!(err, CamelError::Config(_)),
        "expected Config, got: {err:?}"
    );
    assert!(
        err.to_string().contains("http` feature"),
        "error must name the missing feature: {err}"
    );
}

#[tokio::test]
async fn boot_rejects_test_suffix_route_file() {
    // Discovery's reserved-suffix gate silently skips `.test.yaml`/
    // `.test.yml` files (they name `camel test` documents, not
    // routes); a document that explicitly declares one must fail the
    // boot instead of starting zero routes.
    let doc = r#"
routeFiles: [routes.test.yaml]
scenario:
  - sleep:
      duration: 1s
"#;
    let (dir, doc) = project("# minimal\n", Some(("routes.test.yaml", ROUTE)), doc);
    let err = expect_boot_error(
        boot_scenario(&doc, dir.path(), &empty_env()).await,
        "a reserved-suffix route file must fail the boot",
    );
    assert!(
        matches!(err, CamelError::Config(_)),
        "expected Config, got: {err:?}"
    );
    let display = err.to_string();
    assert!(
        display.contains("routes.test.yaml"),
        "error must name the declared file: {display}"
    );
    assert!(
        display.contains("`camel test`"),
        "error must carry the `camel test` guidance: {display}"
    );
}
