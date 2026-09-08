use super::*;
use camel_integration_test::parse_scenario_document;

use std::fs;
use std::path::PathBuf;

/// A unique temp project root for one test, removed on drop
/// (panic-safe): the directory holding `Camel.toml`, the route
/// file, and the scenario document (the v1 harness keeps the
/// document in the project root).
struct TempProject(PathBuf);

impl TempProject {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("camel-cli-scenario-{tag}-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("create temp project root"); // allow-unwrap
        Self(dir)
    }

    fn root(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Writes the minimal boot project into `root` with the default
/// sealed `Camel.toml` (just `log_level = "info"`): one no-op
/// route file (the boot needs a route source; the scenario
/// actions under test never touch it) and the scenario document
/// in the root. Returns the document path.
fn write_project(root: &Path, doc_yaml: &str) -> PathBuf {
    write_project_with_camel_toml(root, "log_level = \"info\"\n", doc_yaml)
}

/// [`write_project`] with an explicit `Camel.toml` body (tests
/// that declare a `[security.*]` section write their own).
fn write_project_with_camel_toml(root: &Path, camel_toml: &str, doc_yaml: &str) -> PathBuf {
    fs::write(root.join("Camel.toml"), camel_toml).expect("write Camel.toml"); // allow-unwrap
    fs::write(
        root.join("routes.yaml"),
        "routes:\n  - id: noop\n    from: direct:start\n    steps:\n      - log: \"noop\"\n",
    )
    .expect("write routes.yaml"); // allow-unwrap
    let doc_path = root.join("scenario.test.yaml");
    fs::write(&doc_path, doc_yaml).expect("write scenario doc"); // allow-unwrap
    doc_path
}

#[test]
fn partner_scripts_map_defaults() {
    let _wasm_acks_guard = crate::commands::run::WASM_ACKS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let project = TempProject::new("partner-scripts-map-defaults");
    let doc_path = write_project(
        project.root(),
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
partners:
  http://127.0.0.1:0/orders:
  - method: POST
    path: /orders
    response:
      body:
        id: ord-7
"#,
    );
    let doc = parse_scenario_document(&doc_path).expect("parse scenario doc"); // allow-unwrap
    let scripts = partner_scripts_for(&doc, "http://127.0.0.1:0/orders")
        .expect("the declared entry must map"); // allow-unwrap
    assert_eq!(scripts.len(), 1, "the entry carries one script");
    let scripted = &scripts[0];
    assert_eq!(scripted.method.as_deref(), Some("POST"));
    assert_eq!(scripted.path.as_deref(), Some("/orders"));
    assert_eq!(scripted.status, 200, "absent status defaults to 200");
    assert!(
        scripted.headers.is_empty(),
        "absent headers default to empty"
    );
    assert_eq!(
        scripted.body,
        br#"{"id":"ord-7"}"#.to_vec(),
        "the body must be the JSON serialization"
    );
}

#[test]
fn partner_scripts_none_when_absent() {
    let _wasm_acks_guard = crate::commands::run::WASM_ACKS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let project = TempProject::new("partner-scripts-none");
    let doc_path = write_project(
        project.root(),
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
"#,
    );
    let doc = parse_scenario_document(&doc_path).expect("parse scenario doc"); // allow-unwrap
    assert!(
        partner_scripts_for(&doc, "http://127.0.0.1:0/orders").is_none(),
        "a document without a partners entry maps to None (caller binds permissive)"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::await_holding_lock)]
async fn driver_binds_permissive_when_partners_absent() {
    let _wasm_acks_guard = crate::commands::run::WASM_ACKS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let project = TempProject::new("driver-binds-permissive");
    let root = project.root();
    let doc_path = write_project(
        root,
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
- receive:
    from:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    deadline: 2s
    extract:
      status: status
      body: body
- validate:
    target: { variable: status }
    expectation: 200
- validate:
    target: { variable: body }
    expectation: ""
"#,
    );
    let doc = parse_scenario_document(&doc_path).expect("parse scenario doc"); // allow-unwrap
    let result = run_scenario_full_boot(&doc, root).await;
    assert_eq!(result.doc_error, None, "permissive bind must not error");
    assert!(!result.apparatus, "no apparatus failure is expected");
    assert_eq!(result.action_results.len(), 4, "every action must run");
    for row in &result.action_results {
        assert!(
            row.outcome.is_ok(),
            "send + receive + validates must pass against the permissive 200 empty partner: {:?}",
            row.outcome
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::await_holding_lock)]
async fn driver_binds_no_partner_for_plain_strings() {
    let _wasm_acks_guard = crate::commands::run::WASM_ACKS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // The literal dial target: a listener the TEST owns, at a real
    // port, outside the driver's adapter map.
    let target = camel_integration_test::HttpPartner::start_permissive(200)
        .await
        .expect("bind the test-owned listener"); // allow-unwrap
    let uri = format!("http://{}/x", target.bound_addr());
    let project = TempProject::new("driver-plain-string");
    let root = project.root();
    let doc_path = write_project(
        root,
        &format!(
            r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: {uri}
- receive:
    from: {uri}
    deadline: 2s
"#
        ),
    );
    let doc = parse_scenario_document(&doc_path).expect("parse scenario doc"); // allow-unwrap

    // Binding scope: the plain-string reference gets NO partner.
    let wired = wire_endpoint_refs(&doc);
    let (adapters, harness_provisioned) = bind_partners(&doc, &wired)
        .await
        .expect("bind step must succeed"); // allow-unwrap
    assert!(
        !adapters.contains_key(&uri),
        "a plain-string ref must not get a partner listener"
    );
    assert!(
        harness_provisioned.is_empty(),
        "no harness bind means no env-tier binding"
    );

    // The send dials the literal URI: the test-owned listener
    // records the arrival.
    let result = run_scenario_full_boot(&doc, root).await;
    assert_eq!(result.doc_error, None, "the plain-string send must dial");
    assert_eq!(result.action_results.len(), 2, "send + receive must run");
    for row in &result.action_results {
        assert!(
            row.outcome.is_ok(),
            "the send must reach the literal URI and the receive must read the roundtrip: {:?}",
            row.outcome
        );
    }
    let recorded = target.recorder().recorded_requests();
    assert_eq!(
        recorded.len(),
        1,
        "exactly one wire arrival on the literal URI"
    );
    assert_eq!(recorded[0].path, "/x");
}

/// The wiring arm under test (validate-partner-self-declare task 2):
/// a validate action's object-form partner target self-declares its
/// harness reference (`provisioning: harness` + `bindVar`), so
/// `wire_endpoint_refs` must wire it exactly like a send/receive
/// reference — and `bind_partners` must fold its `bindVar` into the
/// harness-provisioned env tier route files interpolate.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::await_holding_lock)]
async fn wiring_includes_object_form_validate_partner() {
    let _wasm_acks_guard = crate::commands::run::WASM_ACKS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let project = TempProject::new("wiring-includes-object-form-validate");
    let doc_path = write_project(
        project.root(),
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
- validate:
    target:
      partner:
        endpoint: http://127.0.0.1:0/tiles
        provisioning: harness
        bindVar: upstream
    expectation: {count: 1}
partners:
  http://127.0.0.1:0/tiles:
  - path: /tiles
    response:
      status: 200
      body: tile
"#,
    );
    let doc = parse_scenario_document(&doc_path).expect("parse scenario doc"); // allow-unwrap

    // The wired refs: the send's `direct:start` AND the validate's
    // self-declared partner URI, in declaration order.
    let wired = wire_endpoint_refs(&doc);
    let endpoints: Vec<&str> = wired.iter().map(|r| r.endpoint.as_str()).collect();
    assert_eq!(
        endpoints,
        vec!["direct:start", "http://127.0.0.1:0/tiles"],
        "the object-form validate partner must wire like a send/receive ref"
    );

    // The bind step: the wired validate ref binds its scripted
    // partner and folds the bindVar into the harness env tier.
    let (_adapters, harness_provisioned) = bind_partners(&doc, &wired)
        .await
        .expect("bind step must succeed"); // allow-unwrap
    let bound = harness_provisioned
        .get("upstream")
        .expect("the validate ref's bindVar must land in the harness env tier"); // allow-unwrap
    assert!(
        bound.starts_with("http://"),
        "the env tier keeps the `http://host:port` form route files interpolate: {bound}"
    );
}

/// Shape lock: a plain-string validate partner target carries no
/// provisioning, so the wiring arm must not wire it — only the send's
/// reference wires. Constructed programmatically: the load-time
/// cross-check rejects an undeclared plain-string target at parse
/// (pinned by `undeclared_partner_target_exits_two`), and this lock is
/// about wiring scope, not load-time rejection.
#[test]
fn wiring_excludes_plain_string_validate_partner() {
    use camel_integration_test::{
        CountBound, EndpointRef, PartnerExpectation, RouteSource, ScenarioAction, ScenarioTarget,
        ValidateExpectation,
    };

    let doc = camel_integration_test::ScenarioDocument {
        source_path: std::path::PathBuf::new(),
        route_source: RouteSource::RouteFiles(Vec::new()),
        scenario: vec![
            ScenarioAction::Send {
                to: EndpointRef {
                    endpoint: "direct:start".to_string(),
                    provisioning: None,
                    bind_var: None,
                },
                body: None,
                headers: None,
                method: "GET".to_string(),
                expect_reply: None,
            },
            ScenarioAction::Validate {
                target: ScenarioTarget::Partner(EndpointRef {
                    endpoint: "http://upstream/tiles".to_string(),
                    provisioning: None,
                    bind_var: None,
                }),
                expectation: ValidateExpectation::Partner(PartnerExpectation {
                    bound: CountBound::Exact(1),
                    method: None,
                    path: None,
                    query: None,
                }),
                deadline: None,
                elapsed_at_least: None,
            },
        ],
        partners: None,
        env: None,
        env_passthrough: None,
        profile: None,
        send_deadline: None,
        inbound: None,
    };
    let wired = wire_endpoint_refs(&doc);
    let endpoints: Vec<&str> = wired.iter().map(|r| r.endpoint.as_str()).collect();
    assert_eq!(
        endpoints,
        vec!["direct:start"],
        "a plain-string validate partner must stay inert: only the send wires"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::await_holding_lock)]
async fn partners_key_typo_fails_load() {
    let _wasm_acks_guard = crate::commands::run::WASM_ACKS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let project = TempProject::new("partners-key-typo");
    let doc_path = write_project(
        project.root(),
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
partners:
  http://127.0.0.1:0/order:
  - method: POST
    response:
      status: 201
"#,
    );
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = super::super::run_tests(&[doc_path], &mut out, &mut err).await;
    assert_eq!(
        summary.exit_code, 2,
        "doc-validation is apparatus class, exit 2"
    );
    let err = String::from_utf8(err).expect("stderr is utf-8"); // allow-unwrap
    assert!(
        err.contains("doc-validation"),
        "doc-validation class: {err}"
    );
    assert!(
        err.contains("http://127.0.0.1:0/order"),
        "the error must name the unmatched key: {err}"
    );
    assert!(
        !err.contains("partner-bind"),
        "the cross-check must fail before any partner binds: {err}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::await_holding_lock)]
async fn undeclared_partner_target_exits_two() {
    let _wasm_acks_guard = crate::commands::run::WASM_ACKS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let project = TempProject::new("undeclared-partner-target");
    let doc_path = write_project(
        project.root(),
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
- validate:
    target: {partner: http://127.0.0.1:0/nowhere}
    expectation: {count: 1}
"#,
    );
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = super::super::run_tests(&[doc_path], &mut out, &mut err).await;
    assert_eq!(
        summary.exit_code, 2,
        "doc-validation is apparatus class, exit 2"
    );
    let err = String::from_utf8(err).expect("stderr is utf-8"); // allow-unwrap
    assert!(
        err.contains("doc-validation"),
        "doc-validation class: {err}"
    );
    assert!(
        err.contains("http://127.0.0.1:0/nowhere"),
        "the error must name the undeclared partner URI: {err}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::await_holding_lock)]
async fn keycloak_boot_reports_infra_unavailable() {
    let _wasm_acks_guard = crate::commands::run::WASM_ACKS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // A keycloak-declaring Camel.toml fails the boot with
    // `CamelError::AuthProviderUnavailable` before any builder
    // call — the offline tier's keycloak/oidc rejection
    // (scenario-shared-boot task 3.1). Boot errors classify by
    // variant, never message text, so this rejection maps to the
    // `infra-unavailable` doc-error class, not
    // `full-boot-failure`. The fixture is the minimal keycloak
    // config from camel-integration-test's boot_scenario_test.rs.
    let project = TempProject::new("keycloak-infra-unavailable");
    let root = project.root();
    let doc_path = write_project_with_camel_toml(
        root,
        r#"
[security.keycloak]
server_url = "https://kc.example.com"
realm = "camel"
client_id = "camel-api"
client_secret = "kc-secret"
"#,
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
"#,
    );
    let doc = parse_scenario_document(&doc_path).expect("parse scenario doc"); // allow-unwrap
    let result = run_scenario_full_boot(&doc, root).await;
    let doc_error = result
        .doc_error
        .as_deref()
        .expect("the rejected boot must report a doc error"); // allow-unwrap
    assert!(
        doc_error.starts_with("infra-unavailable:"),
        "AuthProviderUnavailable must classify as infra-unavailable, got: {doc_error}"
    );
    assert!(
        doc_error.contains("scenario boot failed"),
        "the doc error must name the boot failure: {doc_error}"
    );
    assert!(
        result.apparatus,
        "infra-unavailable is apparatus class (exit 2)"
    );
}

/// The full-boot path feeds the router's secret-query mask: a boot
/// through the shared composition root registers the `http`
/// component, whose metadata classifies camel-http's secret uri
/// options (ADR-0051 positive secret rule), so
/// `http_secret_query_keys` — the exact helper
/// `run_scenario_full_boot` calls — yields all three secret
/// options and never a non-secret one.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::await_holding_lock)]
async fn full_boot_feeds_secret_query_keys() {
    let _wasm_acks_guard = crate::commands::run::WASM_ACKS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let project = TempProject::new("full-boot-secret-keys");
    let root = project.root();
    let doc_path = write_project(
        root,
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
"#,
    );
    let doc = parse_scenario_document(&doc_path).expect("parse scenario doc"); // allow-unwrap
    let run = camel_integration_test::boot_scenario(&doc, root, &tier_env())
        .await
        .expect("minimal scenario must boot through the shared wiring"); // allow-unwrap
    // The runner wraps the booted context in Arc<Mutex<_>> before
    // the helper reads it — mirror that wrapping exactly.
    let ctx = std::sync::Arc::new(tokio::sync::Mutex::new(run.ctx));
    let keys = http_secret_query_keys(&ctx).await;
    for secret in ["authUsername", "authPassword", "authBearerToken"] {
        assert!(
            keys.iter().any(|key| key == secret),
            "the booted http metadata must classify {secret} as secret: {keys:?}"
        );
    }
    assert!(
        !keys.iter().any(|key| key == "httpMethod"),
        "a non-secret uri option must never be classified secret: {keys:?}"
    );
    let mut guard = ctx.lock().await;
    let _ = run.boot.shutdown(&mut guard).await;
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::await_holding_lock)]
async fn boot_failure_stays_full_boot_failure() {
    let _wasm_acks_guard = crate::commands::run::WASM_ACKS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // A boot Io error — a routeFiles entry naming a file that does
    // not exist — is not an AuthProviderUnavailable rejection and
    // must keep the full-boot-failure class.
    let project = TempProject::new("boot-stays-full-boot-failure");
    let root = project.root();
    let doc_path = write_project(
        root,
        r#"
routeFiles: [nope.yaml]
scenario:
- send:
    to: direct:start
"#,
    );
    let doc = parse_scenario_document(&doc_path).expect("parse scenario doc"); // allow-unwrap
    let result = run_scenario_full_boot(&doc, root).await;
    let doc_error = result
        .doc_error
        .as_deref()
        .expect("the rejected boot must report a doc error"); // allow-unwrap
    assert!(
        doc_error.starts_with("full-boot-failure:"),
        "a non-auth boot error must keep full-boot-failure, got: {doc_error}"
    );
    assert!(
        result.apparatus,
        "full-boot-failure is apparatus class (exit 2)"
    );
}

// -------------------------------------------------------------------
// Scenario-tier acceptance tests (scenario-shared-boot task 3.3):
// one per divergence row of bd rc-6bsf. The scenario boot runs the
// SAME shared composition root `camel run` runs, so every behavior
// that used to diverge behind the hand-rolled boot now holds in
// the tier. Tests boot through `boot_scenario` directly (like the
// camel-integration-test boot tests); only the wasm doc test
// asserts document-level classification and goes through
// `run_scenario_full_boot`.
// -------------------------------------------------------------------

/// An empty layered environment (no doc, harness, or passthrough
/// layer): the tier-boot tests resolve no placeholders.
fn tier_env() -> camel_integration_test::LayeredEnv {
    use std::collections::BTreeMap;
    camel_integration_test::LayeredEnv::new(
        BTreeMap::new(),
        BTreeMap::new(),
        Vec::new(),
        camel_integration_test::ambient_std(),
    )
}

/// [`write_project_with_camel_toml`] with an explicit route file:
/// the tier divergence tests declare their own route sources (the
/// shared helpers fix `routes.yaml` to the noop route). Returns
/// the document path.
fn write_tier_project(root: &Path, camel_toml: &str, routes_yaml: &str, doc_yaml: &str) -> PathBuf {
    fs::write(root.join("Camel.toml"), camel_toml).expect("write Camel.toml"); // allow-unwrap
    fs::write(root.join("routes.yaml"), routes_yaml).expect("write routes.yaml"); // allow-unwrap
    let doc_path = root.join("scenario.test.yaml");
    fs::write(&doc_path, doc_yaml).expect("write scenario doc"); // allow-unwrap
    doc_path
}

/// Deliver an exchange to a `direct:` endpoint through the booted
/// context's own producer path — the mechanism the harness
/// `DirectStimulus` uses and the run-side twin of
/// `run_tests::direct_oneshot`: a fresh endpoint + producer per
/// send through the component registry, one `oneshot` per
/// exchange, retrying the consumer-startup race
/// (`EndpointCreationFailed`) on a bounded deadline.
async fn direct_oneshot(
    ctx: &camel_core::CamelContext,
    uri: &str,
    exchange: camel_api::Exchange,
) -> Result<camel_api::Exchange, camel_api::CamelError> {
    use tower::ServiceExt;

    const RETRY_SLEEP: std::time::Duration = std::time::Duration::from_millis(20);
    const RETRY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(1);

    let deadline = tokio::time::Instant::now() + RETRY_DEADLINE;
    loop {
        let producer_ctx = ctx.producer_context();
        let component = ctx
            .registry()
            .get("direct")
            .expect("direct component registered by the bundle cascade"); // allow-unwrap
        let endpoint = component
            .create_endpoint(uri, ctx)
            .expect("direct endpoint creation must succeed"); // allow-unwrap
        let producer = endpoint
            .create_producer(
                std::sync::Arc::new(camel_component_api::NoOpComponentContext),
                &producer_ctx,
            )
            .expect("direct producer creation must succeed"); // allow-unwrap
        match producer.oneshot(exchange.clone()).await {
            Ok(reply) => return Ok(reply),
            Err(e) => {
                let is_startup_race = matches!(e, camel_api::CamelError::EndpointCreationFailed(_));
                if is_startup_race && tokio::time::Instant::now() < deadline {
                    tokio::time::sleep(RETRY_SLEEP).await;
                    continue;
                }
                return Err(e);
            }
        }
    }
}

/// The provider registry the SHARED security builder builds from
/// the project's `Camel.toml` — the same seam `boot_scenario`
/// uses internally. `ScenarioRun` does not expose the registry;
/// rebuilding it through the same shared builder from the same
/// config yields the same registry (the run-side twin
/// `run_shared_wiring_native_credentials_gate` holds it from its
/// own wiring sequence).
#[cfg(feature = "security")]
async fn native_provider_registry(
    camel_toml: &str,
    ctx: &camel_core::CamelContext,
) -> camel_auth::ProviderRegistry {
    let config: camel_config::CamelConfig =
        toml::from_str(camel_toml).expect("parse test CamelConfig"); // allow-unwrap
    let sec = camel_bundles::security_boot::build_security_compile_context_from_config(
        &config,
        ctx.registry_arc(),
    )
    .await
    .expect("shared security builder must succeed"); // allow-unwrap
    sec.provider_registry()
}

/// Divergence row "security_policy route boots in the tier": a
/// native-security project with a `security_policy` route boots
/// through the shared composition root (the hand-rolled boot
/// failed with the default security compile context). Credential
/// pair on the booted route — the pass case drives the seam the
/// transports drive (`kernel_authenticate` against the shared
/// builder's registry + `install_carrier`; `direct:` has no
/// transport boundary, so a bearer header alone never
/// authenticates) and completes the route; the refuse case, a
/// plain exchange, is refused with `CamelError::Unauthenticated`.
/// The `Authorization` header rides the pass-case exchange for
/// fidelity — on `direct:` it is not what authenticates.
///
/// Arrival observation (deviation from the task text, reported):
/// `to:` producers bind eagerly at route-compile time inside
/// `boot_scenario`, so the cascade's mock instance — not a
/// test-owned one — records the message, and `Component` is not
/// `Any` (no downcast). The positive trace is therefore the
/// route-set reply header: the direct producer waits for the
/// whole pipeline, so the header proves the policy layer passed
/// AND the `mock:` producer delivered.
#[cfg(feature = "security")]
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::await_holding_lock)]
async fn security_policy_route_boots_in_tier() {
    let _wasm_acks_guard = crate::commands::run::WASM_ACKS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    use camel_api::security_policy::{
        AccessMode, CredentialSource, RouteSecurityPlan, TransportId,
    };
    use camel_api::{Body, CamelError, Exchange, Message};
    use camel_auth::credential_source::ExtractedToken;
    use camel_auth::kernel::{install_carrier, kernel_authenticate};

    // The native bearer credential fixture from camel-bundles
    // security_boot tests; the route is the smallest
    // security_policy fixture (run-side twin:
    // run_shared_wiring_native_credentials_gate).
    let camel_toml = r#"
[security.native]
subject = "dev-user"
issuer = "native"
bearer_token = "dev-token"
roles = ["admin"]
"#;
    let routes_yaml = r#"
routes:
  - id: sec-gate
    from: direct:sec
    security_policy:
      roles: ["admin"]
      provider: "native"
    steps:
      - to: mock:out
      - set_header:
          key: sec-route-trace
          value: passed
"#;
    let project = TempProject::new("sec-policy-tier");
    let root = project.root();
    let doc_path = write_tier_project(
        root,
        camel_toml,
        routes_yaml,
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:sec
"#,
    );
    let doc = parse_scenario_document(&doc_path).expect("parse scenario doc"); // allow-unwrap
    let mut run = camel_integration_test::boot_scenario(&doc, root, &tier_env())
        .await
        .expect("security_policy route must boot through the shared wiring"); // allow-unwrap

    let providers = native_provider_registry(camel_toml, &run.ctx).await;

    // Pass case: the native credential authenticates through the
    // shared builder's registry; the carrier-bearing exchange
    // traverses the policy route to mock:out.
    let mut message = Message::new(Body::Text("ping".to_string()));
    message.set_header(
        "Authorization",
        serde_json::Value::String("Bearer dev-token".to_string()),
    );
    let mut exchange = Exchange::new(message);
    let plan = RouteSecurityPlan {
        access_mode: AccessMode::Authenticated,
        provider_ref: Some("native".to_string()),
        transport: TransportId::Http,
        credential_sources: vec![CredentialSource::AuthorizationHeader],
        audience_binding: None,
    };
    let credentials = ExtractedToken {
        token: "dev-token".to_string(),
        source: CredentialSource::AuthorizationHeader,
    };
    let principal = kernel_authenticate(&plan, &providers, &credentials)
        .await
        .expect("native credential must authenticate via the shared builder's registry"); // allow-unwrap
    install_carrier(&mut exchange, &principal);
    let reply = direct_oneshot(&run.ctx, "direct:sec", exchange)
        .await
        .expect("credentialed send must complete the security_policy route"); // allow-unwrap
    assert_eq!(
        reply
            .input
            .header("sec-route-trace")
            .and_then(serde_json::Value::as_str),
        Some("passed"),
        "the reply must carry the route-set trace (policy passed, mock producer delivered)"
    );

    // Refuse case: no credential anywhere — no carrier, no header.
    let plain = Exchange::new(Message::new(Body::Text("ping".to_string())));
    let err = direct_oneshot(&run.ctx, "direct:sec", plain)
        .await
        .expect_err("credential-less send must be refused"); // allow-unwrap
    assert!(
        matches!(err, CamelError::Unauthenticated(_)),
        "refusal must be Unauthenticated, got: {err:?}"
    );

    let _ = run.boot.shutdown(&mut run.ctx).await;
}

/// Divergence row "non-loopback Public bind fails closed in the
/// tier": a non-loopback bind serving a Public route without
/// `allow_public_exposure` fails the scenario boot at context
/// start with the ADR-0061 acknowledgement error — the shared
/// installer, identical to `camel run` behavior.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::await_holding_lock)]
async fn public_bind_without_ack_fails_closed_in_tier() {
    let _wasm_acks_guard = crate::commands::run::WASM_ACKS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    use camel_api::CamelError;

    let camel_toml = r#"
[binds."0.0.0.0:41998"]
"#;
    let routes_yaml = r#"
routes:
  - id: pub-bind-tier
    from: http://0.0.0.0:41998/pub
    steps:
      - to: mock:out
"#;
    let project = TempProject::new("pub-bind-tier");
    let root = project.root();
    let doc_path = write_tier_project(
        root,
        camel_toml,
        routes_yaml,
        r#"
routeFiles: [routes.yaml]
scenario:
- sleep:
    duration: 1s
"#,
    );
    let doc = parse_scenario_document(&doc_path).expect("parse scenario doc"); // allow-unwrap
    let err = match camel_integration_test::boot_scenario(&doc, root, &tier_env()).await {
        Ok(_) => panic!("unacknowledged non-loopback Public bind must refuse the boot"),
        Err(err) => err,
    };
    match err {
        CamelError::RouteError(msg) => assert!(
            msg.contains("non-loopback address; acknowledge via [binds"),
            "refusal must name the acknowledgement path: {msg}"
        ),
        other => panic!("expected RouteError, got {other:?}"),
    }
}

/// Divergence row "stream-cache threshold from config applies": a
/// non-default `[stream_caching]` threshold plus a bare
/// `stream_cache:` step (threshold omitted at step level — the
/// camel-dsl shorthand grammar) boots through discovery, so the
/// config's threshold reaches route compilation through the same
/// wiring `camel run` uses. The threshold-threading equality
/// proof is task 1.1's test; this is the tier boot smoke.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::await_holding_lock)]
async fn stream_cache_threshold_smoke_boot() {
    let _wasm_acks_guard = crate::commands::run::WASM_ACKS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let camel_toml = r#"
log_level = "info"

[stream_caching]
threshold = 512
"#;
    let routes_yaml = r#"
routes:
  - id: sc-tier
    from: direct:sc
    steps:
      - stream_cache: true
      - to: log:info
"#;
    let project = TempProject::new("stream-cache-tier");
    let root = project.root();
    let doc_path = write_tier_project(
        root,
        camel_toml,
        routes_yaml,
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:sc
"#,
    );
    let doc = parse_scenario_document(&doc_path).expect("parse scenario doc"); // allow-unwrap
    let mut run = camel_integration_test::boot_scenario(&doc, root, &tier_env())
        .await
        .expect("stream_cache route with a config threshold must boot"); // allow-unwrap
    let _ = run.boot.shutdown(&mut run.ctx).await;
}

/// Divergence row "templated route file materializes": a route
/// file with one template and two templated routes boots through
/// discovery's two-pass materialization (the per-file parse
/// yielded zero routes), and both materialized routes accept
/// sends through the booted context. A send to a
/// non-materialized `direct:` endpoint fails with
/// `EndpointCreationFailed` (no consumer), so a completed send
/// per route is the materialization proof. Arrival at `mock:t*`
/// observes through the same reply mechanism as the security
/// test: a completed send means the whole materialized pipeline —
/// including the `mock:` producer — ran.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::await_holding_lock)]
async fn templated_route_file_materializes_in_tier() {
    let _wasm_acks_guard = crate::commands::run::WASM_ACKS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    use camel_api::{Body, Exchange, Message};

    let camel_toml = "log_level = \"info\"\n";
    // Template grammar mirrors the camel-dsl discovery tests: one
    // template parameter, two materializations direct:t1/t2 →
    // mock:t1/t2.
    let routes_yaml = r#"
routes: []
templates:
  - id: direct-mock
    parameters:
      - name: name
    routes:
      - id: "materialized-direct"
        from: "direct:{{name}}"
        steps:
          - to: "mock:{{name}}"
templated_routes:
  - route_template_ref: direct-mock
    route_id: "t1"
    parameters:
      name: t1
  - route_template_ref: direct-mock
    route_id: "t2"
    parameters:
      name: t2
"#;
    let project = TempProject::new("templated-tier");
    let root = project.root();
    let doc_path = write_tier_project(
        root,
        camel_toml,
        routes_yaml,
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:t1
- send:
    to: direct:t2
"#,
    );
    let doc = parse_scenario_document(&doc_path).expect("parse scenario doc"); // allow-unwrap
    let mut run = camel_integration_test::boot_scenario(&doc, root, &tier_env())
        .await
        .expect("both templated routes must materialize and boot"); // allow-unwrap

    for (uri, lane) in [("direct:t1", "t1"), ("direct:t2", "t2")] {
        let exchange = Exchange::new(Message::new(Body::Text(format!("ping {lane}"))));
        match direct_oneshot(&run.ctx, uri, exchange).await {
            Ok(_) => {}
            Err(e) => panic!("send to {uri} must complete the materialized route: {e}"),
        }
    }

    let _ = run.boot.shutdown(&mut run.ctx).await;
}

/// Divergence row "sql dynamic-query route fails closed at
/// scenario startup": a `sql:` endpoint declaring dynamic-query
/// intent (`useMessageBodyForSql` without `allowDynamicQuery`)
/// fails the scenario boot at context start with the ADR-0033
/// startup-check error naming the `sql-dynamic-query` check — the
/// shared installer, identical to `camel run` behavior. Fixture
/// shape mirrors the camel-core startup_validation scanner tests.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::await_holding_lock)]
async fn sql_dynamic_query_fails_closed_in_tier() {
    let _wasm_acks_guard = crate::commands::run::WASM_ACKS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    use camel_api::CamelError;

    let camel_toml = "log_level = \"info\"\n";
    let routes_yaml = r#"
routes:
  - id: sql-tier
    from: direct:sql
    steps:
      - to: "sql:select 1?db_url=postgres://x/y&useMessageBodyForSql=true"
"#;
    let project = TempProject::new("sql-tier");
    let root = project.root();
    let doc_path = write_tier_project(
        root,
        camel_toml,
        routes_yaml,
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:sql
"#,
    );
    let doc = parse_scenario_document(&doc_path).expect("parse scenario doc"); // allow-unwrap
    let err = match camel_integration_test::boot_scenario(&doc, root, &tier_env()).await {
        Ok(_) => panic!("the ADR-0033 fail-closed sql check must refuse the boot"),
        Err(err) => err,
    };
    match err {
        CamelError::Config(msg) => assert!(
            msg.contains("sql-dynamic-query"),
            "startup refusal must name the sql-dynamic-query check: {msg}"
        ),
        other => panic!("expected Config, got {other:?}"),
    }
}

/// Divergence row "wasm security policies rejected in v1",
/// document-level: a wasm `[security.policies]` declaration fails
/// the boot fail-closed with the v1 tier limitation, and the
/// variant-based classification keeps the `full-boot-failure`
/// doc-error class (a `Config` rejection, not
/// `AuthProviderUnavailable`). The fixture mirrors 3.1's
/// `boot_rejects_wasm_security_policies` (the wrapper's only
/// field is the `wasm` map; the gate fires before any builder
/// call, so the .wasm file need not exist).
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::await_holding_lock)]
async fn wasm_security_policies_rejected_in_tier_doc() {
    let _wasm_acks_guard = crate::commands::run::WASM_ACKS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let project = TempProject::new("wasm-tier-doc");
    let root = project.root();
    let doc_path = write_project_with_camel_toml(
        root,
        r#"
[security.policies.wasm.demo]
path = "policies/demo.wasm"
"#,
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
"#,
    );
    let doc = parse_scenario_document(&doc_path).expect("parse scenario doc"); // allow-unwrap
    let result = run_scenario_full_boot(&doc, root).await;
    let doc_error = result
        .doc_error
        .as_deref()
        .expect("the rejected boot must report a doc error"); // allow-unwrap
    assert!(
        doc_error.starts_with("full-boot-failure:"),
        "a wasm Config rejection must keep full-boot-failure, got: {doc_error}"
    );
    assert!(
        doc_error.contains("not supported in the scenario tier"),
        "the doc error must name the v1 tier limitation: {doc_error}"
    );
    assert!(
        result.apparatus,
        "full-boot-failure is apparatus class (exit 2)"
    );
}
