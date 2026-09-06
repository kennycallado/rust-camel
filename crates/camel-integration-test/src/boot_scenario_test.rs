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

use crate::ScenarioDocument;
use crate::boot_scenario::boot_scenario;
use crate::env_layers::{LayeredEnv, ambient_std};
use crate::parse_scenario_document;

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

#[tokio::test]
async fn boot_inline_routes_still_rejected() {
    let doc = r#"
routes:
  - id: inline-route
    from: direct:start
    steps:
      - to: log:info
scenario:
  - sleep:
      duration: 1s
"#;
    let (dir, doc) = project("# minimal\n", None, doc);
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
