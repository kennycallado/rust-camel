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
        F::ActionTransport { .. }
            | F::PartnerStartup { .. }
            | F::ShutdownFailure { .. }
            | F::ArrivalLaneOverflow { .. }
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
/// declaration order, deduplicated by endpoint URI. A validate
/// action's object-form partner target carrying `provisioning:
/// harness` self-declares its harness reference (the grammar Task 1
/// of validate-partner-self-declare pinned), so it wires exactly like
/// a send/receive reference: the `seen` dedup below prevents
/// double-wiring when a send/receive also names the URI, and a
/// plain-string target (no provisioning) stays inert.
fn wire_endpoint_refs(
    doc: &camel_integration_test::ScenarioDocument,
) -> Vec<&camel_integration_test::EndpointRef> {
    use camel_integration_test::{Provisioning, ScenarioAction as A, ScenarioTarget};
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for action in &doc.scenario {
        let reference = match action {
            A::Send { to, .. } => to,
            A::Receive { from, .. } => from,
            A::Validate {
                target: ScenarioTarget::Partner(ep),
                ..
            } if ep.provisioning == Some(Provisioning::Harness) => ep,
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

/// ADR-0051 positive secret rule: the booted context's `http` component
/// metadata classifies secret query options; the router masks their
/// values from every wire-path diagnostic.
#[cfg(feature = "integration-http")]
async fn http_secret_query_keys(
    ctx: &std::sync::Arc<tokio::sync::Mutex<camel_core::CamelContext>>,
) -> Vec<String> {
    ctx.lock()
        .await
        .component_metadata("http")
        .map(|metadata| {
            metadata
                .uri_options
                .iter()
                .filter(|option| option.secret)
                .map(|option| option.name.clone())
                .collect()
        })
        .unwrap_or_default()
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
            // Document-error class by variant (the camel-integration-test
            // convention: classification by variant, never message text —
            // see the variant→class map in document.rs "Errors"): an
            // AuthProviderUnavailable rejection (keycloak/oidc config in
            // the offline tier, scenario-shared-boot task 3.1) is the
            // infra-unavailable class; every other boot failure keeps
            // full-boot-failure.
            let class = matches!(e, camel_api::CamelError::AuthProviderUnavailable(_))
                .then_some("infra-unavailable")
                .unwrap_or("full-boot-failure");
            return ScenarioDocResult {
                action_results: Vec::new(),
                doc_error: Some(format!("{class}: scenario boot failed: {e}")),
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

    // ADR-0051 positive secret rule — see `http_secret_query_keys`.
    router.set_secret_query_keys(http_secret_query_keys(&ctx).await);

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
#[path = "scenario_tests.rs"]
mod tests;
