//! Scenario-document execution (ADR-0069 sections 4, 5, 7, 10).
//!
//! Path selection lives in [`run_scenario_doc`]: a document whose wire
//! schemes this build provisions beyond `fake` runs through the
//! embedded full boot (`integration-http`: harness `http` partners,
//! `direct` route stimulus); a `fake`-only document keeps the no-boot
//! smoke path; any other scheme reports `infra-unavailable` naming the
//! adapter. Both paths execute the ORIGINAL document through
//! `run_scenario_document` and map the per-action outcome to rows
//! through [`outcome_rows`]; the taxonomy exit mapping (verdict 1,
//! apparatus 2, doc-validation 2) lives in the caller.

use std::path::Path;

use super::runner::EndpointResult;

/// The endpoint scheme the no-boot smoke path binds an in-memory
/// [`FakeAdapter`] for; the only partner scheme every build provides.
const FAKE_SCHEME: &str = "fake";

/// The wire schemes the full-boot path provisions when the CLI is built
/// with `integration-http` (ADR-0069 section 8): `direct` rides the
/// booted context's producer path, `http` binds a harness partner. A
/// `fake`-only document never boots — it stays on the smoke path.
#[cfg(feature = "integration-http")]
const BOOT_SCHEMES: [&str; 3] = [FAKE_SCHEME, "direct", "http"];

/// What this build's smoke path can provide, for the
/// infra-unavailable message. Without `integration-http` the string is
/// the historical one verbatim.
#[cfg(not(feature = "integration-http"))]
const PROVIDED_ADAPTERS: &str = "only the `fake:` in-memory adapter";
#[cfg(feature = "integration-http")]
const PROVIDED_ADAPTERS: &str = "the `fake:` in-memory adapter and the `http:` wire partner";

/// Outcome of running one scenario document: one row per executed
/// action plus the apparatus-class flag for the exit mapping.
pub(super) struct ScenarioDocResult {
    /// Per-action verdict rows, in action order; `endpoint` holds the
    /// row label (`scenario[i] <kind>`, or `shutdown` for the
    /// post-verdict teardown slot).
    pub action_results: Vec<EndpointResult>,
    /// Document-level error (`infra-unavailable`, `partner-bind-failure`,
    /// `full-boot-failure`): apparatus class, exit 2.
    pub doc_error: Option<String>,
    /// Any action failed with an apparatus-class failure
    /// (`action-transport-failure`, `partner-startup-failure`,
    /// `shutdown-failure`): exit 2 regardless of verdict failures.
    pub apparatus: bool,
}

/// Whether a scenario failure is apparatus class (ADR-0069 section 7):
/// the scenario never got a meaningful answer. Classification is by
/// variant, never by message text. Verdict-class failures
/// (`receive-timeout`, `validation-mismatch`, runtime
/// `scenario-var-unresolved`) return `false` and map to exit 1.
pub(super) fn is_apparatus(failure: &camel_integration_test::ScenarioFailure) -> bool {
    use camel_integration_test::ScenarioFailure as F;
    matches!(
        failure,
        F::ActionTransport { .. } | F::PartnerStartup { .. } | F::ShutdownFailure { .. }
    )
}

/// The row label for one scenario action: `scenario[i] <kind>`.
fn action_label(index: usize, action: &camel_integration_test::ScenarioAction) -> String {
    use camel_integration_test::ScenarioAction as A;
    let kind = match action {
        A::Send { .. } => "send",
        A::Receive { .. } => "receive",
        A::Sleep { .. } => "sleep",
        A::Validate { .. } => "validate",
        _ => "action",
    };
    format!("scenario[{index}] {kind}")
}

/// Maps a whole-document `DocumentOutcome` to per-action rows plus the
/// apparatus flag. `per_action[i]` is action `i` — the runner stops at
/// the first failure, so indices align with `doc.scenario`. Shared by
/// the smoke and full-boot paths.
fn outcome_rows(
    doc: &camel_integration_test::ScenarioDocument,
    outcome: &camel_integration_test::DocumentOutcome,
) -> (Vec<EndpointResult>, bool) {
    let mut apparatus = false;
    let action_results = outcome
        .per_action
        .iter()
        .enumerate()
        .map(|(index, result)| {
            let label = action_label(index, &doc.scenario[index]);
            match result {
                Ok(_) => EndpointResult {
                    endpoint: label,
                    outcome: Ok(()),
                },
                Err(failure) => {
                    apparatus = apparatus || is_apparatus(failure);
                    EndpointResult {
                        endpoint: label,
                        outcome: Err(failure.to_string()),
                    }
                }
            }
        })
        .collect();
    (action_results, apparatus)
}

/// The scheme (text before the first `:`) of an endpoint URI.
fn scheme_of(endpoint: &str) -> &str {
    endpoint.split(':').next().unwrap_or(endpoint)
}

/// The `send`/`receive` endpoint references a scenario wires, in
/// declaration order, deduplicated by endpoint URI.
fn wire_endpoint_refs(
    doc: &camel_integration_test::ScenarioDocument,
) -> Vec<&camel_integration_test::EndpointRef> {
    use camel_integration_test::ScenarioAction as A;
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for action in &doc.scenario {
        let reference = match action {
            A::Send { to, .. } => to,
            A::Receive { from, .. } => from,
            _ => continue,
        };
        if seen.insert(reference.endpoint.clone()) {
            out.push(reference);
        }
    }
    out
}

/// Whether a wired reference is the map form (`provisioning: harness`)
/// pointing at an `http` endpoint — the only references the driver
/// binds a partner for, and the only keys a `partners:` entry may name.
#[cfg(feature = "integration-http")]
fn is_harness_http(reference: &camel_integration_test::EndpointRef) -> bool {
    use camel_integration_test::Provisioning;
    reference.provisioning == Some(Provisioning::Harness)
        && scheme_of(&reference.endpoint) == "http"
}

/// The scripted responses a document's `partners:` entry maps to, for
/// one endpoint key: a thin delegate to the canonical mapping
/// `camel_integration_test::partner_scripts_for`, so the
/// grammar-to-wire semantics live in one place.
#[cfg(feature = "integration-http")]
fn partner_scripts_for(
    doc: &camel_integration_test::ScenarioDocument,
    endpoint: &str,
) -> Option<Vec<camel_integration_test::ScriptedResponse>> {
    camel_integration_test::partner_scripts_for(doc, endpoint)
}

/// The partner-bind sub-step of the full-boot path, extracted for test
/// access: one scripted or permissive listener per harness
/// `http` reference (ADR-0069 sections 8-9). The binding scope is
/// deliberate — ONLY the map form with `provisioning: harness` gets a
/// partner. A plain-string endpoint ref gets none: a plain-string send
/// dials its literal interpolated URI (the inbound-put pattern), and a
/// dynamic `http://${PARTNER}/...` send routes by interpolated
/// authority through the router's wire-target math; binding a
/// plain-string ref would point an unused listener at an address no
/// scenario ever resolves. A scripted partner binds where the document
/// declares a matching `partners:` entry ([`partner_scripts_for`]),
/// permissive 200 otherwise. Returns the adapter map keyed by declared
/// endpoint plus the harness-provisioned env tier (the
/// `http://host:port` form route files interpolate), or the
/// `partner-bind-failure` doc error.
#[cfg(feature = "integration-http")]
async fn bind_partners(
    doc: &camel_integration_test::ScenarioDocument,
    wired: &[&camel_integration_test::EndpointRef],
) -> Result<
    (
        std::collections::BTreeMap<String, Box<dyn camel_integration_test::PartnerAdapter>>,
        std::collections::BTreeMap<String, String>,
    ),
    String,
> {
    use camel_integration_test::HttpPartner;
    let mut adapters: std::collections::BTreeMap<
        String,
        Box<dyn camel_integration_test::PartnerAdapter>,
    > = std::collections::BTreeMap::new();
    let mut harness_provisioned: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for reference in wired {
        let endpoint = &reference.endpoint;
        if !is_harness_http(reference) {
            continue;
        }
        let partner = match partner_scripts_for(doc, endpoint) {
            Some(scripts) => HttpPartner::start(scripts).await,
            None => HttpPartner::start_permissive(200).await,
        };
        let partner = match partner {
            Ok(partner) => partner,
            Err(e) => {
                return Err(format!("partner-bind-failure: endpoint {endpoint}: {e}"));
            }
        };
        if let Some(bind_var) = &reference.bind_var {
            harness_provisioned
                .insert(bind_var.clone(), format!("http://{}", partner.bound_addr()));
        }
        adapters.insert(endpoint.clone(), Box::new(partner));
    }
    Ok((adapters, harness_provisioned))
}

/// Runs one scenario document.
///
/// Path selection: when this build provides every wire scheme the
/// document uses beyond `fake` — `integration-http` built in, endpoints
/// only `direct`/`http`/`fake` — the document runs through the embedded
/// full boot ([`run_scenario_full_boot`]). Otherwise the no-boot fake
/// smoke path applies, which reports the first scheme without an
/// adapter as `infra-unavailable` (apparatus class, exit 2) — never a
/// silent `ReceiveTimeout` verdict failure.
///
/// `root` is the project root holding `Camel.toml`; v1 keeps the
/// document in the project root, so the caller passes the document's
/// directory.
pub(super) async fn run_scenario_doc(
    doc: &camel_integration_test::ScenarioDocument,
    // Only the full-boot path reads the root; the featureless smoke
    // path takes it for signature stability.
    #[cfg_attr(not(feature = "integration-http"), allow(unused_variables))] root: &Path,
) -> ScenarioDocResult {
    let wired = wire_endpoint_refs(doc);
    #[cfg(feature = "integration-http")]
    if wired.iter().any(|r| scheme_of(&r.endpoint) != FAKE_SCHEME)
        && wired
            .iter()
            .all(|r| BOOT_SCHEMES.contains(&scheme_of(&r.endpoint)))
    {
        return run_scenario_full_boot(doc, root).await;
    }
    run_scenario_fake_smoke(doc, &wired).await
}

/// The no-boot fake smoke path (ADR-0069 section 5, pre-boot harness):
/// every wired endpoint must be `fake:` — bind one scripted-empty
/// [`FakeAdapter`] per endpoint URI and run the whole document through
/// the shared `run_scenario_document` (available in every build; the
/// fake adapters need no boot).
///
/// LOAD-TIME ADAPTER COVERAGE (Task 2.4 review carry-forward): an
/// endpoint whose scheme has no adapter in this build is reported as
/// `infra-unavailable` naming the adapter. The map covers every wired
/// endpoint by construction: each one is either bound as `fake:` or
/// reported missing above (coverage is debug-asserted). No boot exists
/// on this path, so no shutdown site exists.
async fn run_scenario_fake_smoke(
    doc: &camel_integration_test::ScenarioDocument,
    wired: &[&camel_integration_test::EndpointRef],
) -> ScenarioDocResult {
    use camel_integration_test::{FakeAdapter, PartnerRouter, ScenarioVars};

    // Build the adapter map keyed by exact endpoint URI.
    let mut adapters: std::collections::BTreeMap<
        String,
        Box<dyn camel_integration_test::PartnerAdapter>,
    > = std::collections::BTreeMap::new();
    let mut missing: Vec<(&str, &str)> = Vec::new();
    for reference in wired {
        let endpoint = reference.endpoint.as_str();
        if scheme_of(endpoint) == FAKE_SCHEME {
            adapters.insert(
                endpoint.to_string(),
                Box::new(FakeAdapter::scripted(Vec::new())),
            );
        } else {
            missing.push((endpoint, scheme_of(endpoint)));
        }
    }
    if let Some((endpoint, scheme)) = missing.first() {
        return ScenarioDocResult {
            action_results: Vec::new(),
            doc_error: Some(format!(
                "infra-unavailable: endpoint {endpoint} needs the {scheme} partner adapter; \
                 this build provides {PROVIDED_ADAPTERS}"
            )),
            apparatus: true,
        };
    }
    // Coverage invariant of the rule above: every wired endpoint is
    // either bound as `fake:` or was reported missing (early return).
    debug_assert!(
        wired
            .iter()
            .all(|reference| adapters.contains_key(&reference.endpoint))
    );
    let router = PartnerRouter::new(adapters);

    let mut vars = ScenarioVars::new();
    let outcome = camel_integration_test::run_scenario_document(doc, &router, &mut vars).await;
    let (action_results, apparatus) = outcome_rows(doc, &outcome);
    ScenarioDocResult {
        action_results,
        doc_error: None,
        apparatus,
    }
}

/// The full-boot path (ADR-0069 sections 4-5, 10): bind one
/// [`HttpPartner`] per harness `http` endpoint (each binds
/// `127.0.0.1:0`; every harness-provisioned `bindVar` lands in the
/// harness tier of the [`LayeredEnv`], winning over the document env),
/// boot the real composition root through `boot_scenario`, execute the
/// whole document through `run_scenario_document`, then
/// `boot.shutdown(&mut ctx)` — log-and-continue: a shutdown failure is
/// recorded in the outcome's `final_failure` slot and rendered as the
/// `shutdown-failure` row after the recorded verdict (apparatus class,
/// exit 2); it never masks the verdict. Partners are
/// caller-owned here: they bind before the boot and drop with the
/// router after the teardown.
///
/// Route stimulus for `direct:` sends rides the booted context's own
/// producer path ([`DirectStimulus`]), the same mechanism the
/// integration-tier crate's e2e tests use — the partner schemes
/// dispatch through the `PartnerRouter`.
///
/// Partner listeners are harness-scoped: only map-form references with
/// `provisioning: harness` bind one ([`bind_partners`]); a
/// scripted partner binds where the document's `partners:` entry
/// declares the key, a non-consuming permissive 200 default
/// ([`HttpPartner::start_permissive`]) otherwise — every unmatched
/// request gets it for the document's lifetime, because outbound
/// scenarios validate arrivals on the wire, not responses.
#[cfg(feature = "integration-http")]
async fn run_scenario_full_boot(
    doc: &camel_integration_test::ScenarioDocument,
    root: &Path,
) -> ScenarioDocResult {
    use camel_integration_test::{
        DirectStimulus, EndpointRef, FakeAdapter, LayeredEnv, PartnerRouter, ScenarioFailure,
        ScenarioVars, ambient_std, boot_scenario, run_scenario_document,
    };
    use std::sync::Arc;
    use tokio::sync::Mutex;

    let wired = wire_endpoint_refs(doc);

    // Load-time cross-check (ADR-0069 section 9): every `partners:` key
    // must equal a wired harness `http` endpoint reference. A typo of a
    // real key (`:0/order` vs `:0/orders`) fails here, at
    // doc-validation, BEFORE any partner binds — never as a silent
    // fall-through to the permissive default.
    if let Some(partners) = &doc.partners {
        let harness_http: std::collections::BTreeSet<&str> = wired
            .iter()
            .copied()
            .filter(|r| is_harness_http(r))
            .map(|r| r.endpoint.as_str())
            .collect();
        if let Some(key) = partners
            .keys()
            .find(|key| !harness_http.contains(key.as_str()))
        {
            return ScenarioDocResult {
                action_results: Vec::new(),
                doc_error: Some(format!(
                    "doc-validation: partners[{key}]: no wired harness `http` endpoint \
                     reference declares this key"
                )),
                apparatus: true,
            };
        }
    }

    // (a) Partners first: bind one listener per harness `http`
    // endpoint (plain-string refs get none — their sends dial the
    // literal interpolated URI), fold each env-tier bindVar into the
    // harness-provisioned tier.
    let (mut adapters, harness_provisioned) = match bind_partners(doc, &wired).await {
        Ok((adapters, harness_provisioned)) => (adapters, harness_provisioned),
        Err(doc_error) => {
            return ScenarioDocResult {
                action_results: Vec::new(),
                doc_error: Some(doc_error),
                apparatus: true,
            };
        }
    };

    // (b) The layered environment: harness-provisioned bindings win
    // over the document env; ambient reads stay behind the passthrough
    // allowlist.
    let env = LayeredEnv::new(
        doc.env.clone().unwrap_or_default(),
        harness_provisioned,
        doc.env_passthrough.clone().unwrap_or_default(),
        ambient_std(),
    );

    // (c) The real boot: sealed config load, component cascade, route
    // source, context start.
    let run = match boot_scenario(doc, root, &env).await {
        Ok(run) => run,
        Err(e) => {
            return ScenarioDocResult {
                action_results: Vec::new(),
                doc_error: Some(format!("full-boot-failure: scenario boot failed: {e}")),
                apparatus: true,
            };
        }
    };
    let ctx = Arc::new(Mutex::new(run.ctx));

    // (d) Complete the router: `direct:` sends stimulate the booted
    // context; `fake:` endpoints keep the in-memory adapter.
    for reference in &wired {
        match scheme_of(&reference.endpoint) {
            "direct" => {
                adapters.insert(
                    reference.endpoint.clone(),
                    Box::new(DirectStimulus::new(Arc::clone(&ctx))),
                );
            }
            FAKE_SCHEME => {
                adapters.insert(
                    reference.endpoint.clone(),
                    Box::new(FakeAdapter::scripted(Vec::new())),
                );
            }
            _ => {}
        }
    }
    let router = PartnerRouter::new(adapters);

    // (e) The whole document through the shared runner: one row per
    // executed action, stop at the first failure. The scenario-tier
    // bindVars carry the partner's `host:port` authority (so a
    // scenario string addresses the partner as
    // `http://${NAME}/...`); the env tier above keeps the
    // `http://host:port` form for route files.
    let mut vars = ScenarioVars::new();
    let wired_refs: Vec<EndpointRef> = wired.iter().map(|r| (*r).clone()).collect();
    camel_integration_test::runner::fill_bind_vars(&wired_refs, &router, &mut vars);
    let mut outcome = run_scenario_document(doc, &router, &mut vars).await;
    let (mut action_results, mut apparatus) = outcome_rows(doc, &outcome);

    // (f) Teardown, log-and-continue: a shutdown failure never masks
    // the recorded verdict; it fills the outcome's post-verdict
    // `final_failure` slot — the struct stays complete for
    // programmatic consumers — and the row below renders from that
    // slot as the `shutdown-failure` line: apparatus class, exit 2.
    {
        let mut guard = ctx.lock().await;
        if let Err(e) = run.boot.shutdown(&mut guard).await {
            // log-policy: system-broken
            tracing::error!("scenario boot shutdown failed: {e}");
            outcome.final_failure = Some(ScenarioFailure::ShutdownFailure {
                message: e.to_string(),
            });
        }
    }
    if let Some(final_failure) = &outcome.final_failure {
        apparatus = true;
        action_results.push(EndpointResult {
            endpoint: "shutdown".to_string(),
            outcome: Err(final_failure.to_string()),
        });
    }

    ScenarioDocResult {
        action_results,
        doc_error: None,
        apparatus,
    }
}

#[cfg(all(test, feature = "integration-http"))]
mod tests {
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
            let dir = std::env::temp_dir()
                .join(format!("camel-cli-scenario-{tag}-{}", std::process::id()));
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

    /// Writes the minimal boot project into `root`: a sealed
    /// `Camel.toml`, one no-op route file (the boot needs a route
    /// source; the scenario actions under test never touch it), and
    /// the scenario document in the root. Returns the document path.
    fn write_project(root: &Path, doc_yaml: &str) -> PathBuf {
        fs::write(root.join("Camel.toml"), "log_level = \"info\"\n").expect("write Camel.toml"); // allow-unwrap
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
    async fn driver_binds_permissive_when_partners_absent() {
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
    async fn driver_binds_no_partner_for_plain_strings() {
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

    #[tokio::test(flavor = "multi_thread")]
    async fn partners_key_typo_fails_load() {
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
}
