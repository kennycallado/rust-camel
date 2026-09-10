// Each test binary compiles this module and uses only its own helper
// family (log binaries never call the partner helpers and vice
// versa), so cross-family items read as dead in any single binary.
#![allow(dead_code)]

//! Shared helpers of the scenario-tier test binaries (rc-tdgh5,
//! rc-p1x2a).
//!
//! Two helper families, deliberately separate:
//!
//! - Log capture (rc-tdgh5): [`RUN_LOCK`] serializes document runs
//!   inside one test binary — capture windows are process-global, and
//!   overlapping runs would attribute events across documents.
//!   [`run_logs_document`] is the install-first run helper: the
//!   capture subscriber is ensured BEFORE [`boot_scenario`], so the
//!   harness wins the process's first-wins `try_init` — unless the
//!   test binary installed a foreign subscriber first (the
//!   `log_foreign_subscriber_test` contract), in which case capture
//!   loses and the document fails with `LogCaptureUnavailable`.
//!   These are the ONLY capture-aware helpers: partner/test binaries
//!   must not acquire capture-subscriber coupling.
//! - Partner document running (rc-p1x2a): the neutral
//!   [`wired_refs`] / [`bind_doc_partners`] / [`run_doc`] family the
//!   partner binaries previously duplicated per file. Feature `http`
//!   only.

use std::collections::BTreeMap;
use std::sync::Arc;

use camel_integration_test::runner::fill_bind_vars;
use camel_integration_test::{
    DirectStimulus, DocumentOutcome, EndpointRef, LayeredEnv, PartnerAdapter, PartnerRouter,
    ScenarioAction, ScenarioTarget, ScenarioVars, ambient_std, boot_scenario,
    ensure_capture_subscriber, parse_scenario_document, run_scenario_document,
};

/// Serializes document runs inside one test binary: capture windows
/// are process-global, and overlapping runs would attribute events
/// across documents.
pub static RUN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Acquires [`RUN_LOCK`] for the whole test (poison-tolerant).
pub fn lock_run() -> std::sync::MutexGuard<'static, ()> {
    RUN_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Runs one `logs:` document against a freshly booted route, the
/// install-first discipline applied: [`ensure_capture_subscriber`]
/// runs before [`boot_scenario`], so the harness subscriber wins the
/// process's first-wins `try_init` (in the foreign-subscriber binary
/// it loses by design and the document fails with
/// `LogCaptureUnavailable`).
///
/// `fixture` names a route file under `tests/fixtures/logs/`; it is
/// copied next to the temporary document, because relative
/// `routeFiles` anchor to the document's own directory (rc-jjzy5).
pub async fn run_logs_document(doc_yaml: &str, fixture: &str) -> DocumentOutcome {
    ensure_capture_subscriber();
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path();
    std::fs::write(root.join("Camel.toml"), "log_level = \"info\"\n").expect("write Camel.toml");
    let fixture_path = format!(
        "{}/tests/fixtures/logs/{fixture}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::copy(&fixture_path, root.join("routes.yaml")).expect("copy route fixture");
    let path = root.join("case.test.yaml");
    std::fs::write(&path, doc_yaml).expect("write case file");
    let doc = parse_scenario_document(&path).expect("document must load");

    let env = LayeredEnv::new(
        doc.env.clone().unwrap_or_default(),
        BTreeMap::new(),
        doc.env_passthrough.clone().unwrap_or_default(),
        ambient_std(),
    );
    let run = boot_scenario(&doc, root, &env)
        .await
        .expect("the full boot must succeed");
    let ctx = Arc::new(tokio::sync::Mutex::new(run.ctx));

    let mut adapters: BTreeMap<String, Box<dyn PartnerAdapter>> = BTreeMap::new();
    adapters.insert(
        "direct:start".to_string(),
        Box::new(DirectStimulus::new(Arc::clone(&ctx))),
    );
    let router = PartnerRouter::new(adapters);

    let wired = wired_refs(&doc);
    let mut vars = ScenarioVars::new();
    fill_bind_vars(&wired, &router, &mut vars);
    let outcome = run_scenario_document(&doc, &router, &mut vars, None).await;

    let mut guard = ctx.lock().await;
    run.boot
        .shutdown(&mut guard)
        .await
        .expect("clean shutdown must complete");
    outcome
}

// ---------------------------------------------------------------------------
// Partner document helpers (rc-p1x2a, feature `http`)
// ---------------------------------------------------------------------------

/// The endpoint references a document wires, in declaration order:
/// send targets, receive sources, and `lastReceived` validate keys —
/// the canonical walk the partner test binaries and the CLI driver's
/// `fill_bind_vars` step share. Partner validate targets bind nothing
/// of their own: the parse-time cross-check requires their URI to be
/// declared by a send/receive, which is where the partner binds.
#[cfg(feature = "http")]
pub fn wired_refs(doc: &camel_integration_test::ScenarioDocument) -> Vec<EndpointRef> {
    doc.scenario
        .iter()
        .filter_map(|action| match action {
            ScenarioAction::Send { to, .. } => Some(to.clone()),
            ScenarioAction::Receive { from, .. } => Some(from.clone()),
            ScenarioAction::Validate {
                target: ScenarioTarget::LastReceived(endpoint),
                ..
            } => Some(endpoint.clone()),
            _ => None,
        })
        .collect()
}

/// Binds one partner per harness `http` reference (scripted where the
/// document declares a matching `partners:` entry, permissive 200
/// otherwise) and returns the router with the per-endpoint recorders
/// and bound authorities (`host:port` — the raw-dial path a foreign
/// client takes). Deduplicates by endpoint URI.
#[cfg(feature = "http")]
pub async fn bind_doc_partners(
    doc: &camel_integration_test::ScenarioDocument,
) -> (
    PartnerRouter,
    BTreeMap<String, camel_integration_test::HttpRecorder>,
    BTreeMap<String, String>,
) {
    use camel_integration_test::{HttpPartner, Provisioning, partner_scripts_for};

    let mut adapters: BTreeMap<String, Box<dyn PartnerAdapter>> = BTreeMap::new();
    let mut recorders: BTreeMap<String, camel_integration_test::HttpRecorder> = BTreeMap::new();
    let mut authorities: BTreeMap<String, String> = BTreeMap::new();
    for reference in wired_refs(doc) {
        if reference.provisioning != Some(Provisioning::Harness)
            || !reference.endpoint.starts_with("http://")
            || adapters.contains_key(&reference.endpoint)
        {
            continue;
        }
        let partner = match partner_scripts_for(doc, &reference.endpoint) {
            Some(scripts) => HttpPartner::start(scripts).await,
            None => HttpPartner::start_permissive(200).await,
        }
        .expect("partner must bind 127.0.0.1:0");
        authorities.insert(reference.endpoint.clone(), partner.bound_addr().to_string());
        recorders.insert(reference.endpoint.clone(), partner.recorder());
        adapters.insert(reference.endpoint.clone(), Box::new(partner));
    }
    (PartnerRouter::new(adapters), recorders, authorities)
}

/// Loads `yaml` through the crate's document path, binds the declared
/// partners, fills the bind variables, runs the whole document, and
/// returns the outcome with the per-endpoint recorders.
#[cfg(feature = "http")]
pub async fn run_doc(
    yaml: &str,
) -> (
    DocumentOutcome,
    BTreeMap<String, camel_integration_test::HttpRecorder>,
) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("case.test.yaml");
    std::fs::write(&path, yaml).expect("write case file");
    let doc = parse_scenario_document(&path).expect("document must load");
    let (router, recorders, _authorities) = bind_doc_partners(&doc).await;
    let wired = wired_refs(&doc);
    let mut vars = ScenarioVars::new();
    fill_bind_vars(&wired, &router, &mut vars);
    let outcome = run_scenario_document(&doc, &router, &mut vars, None).await;
    (outcome, recorders)
}

/// [`run_doc`] plus the bound authorities: the raw-dial coordinates a
/// foreign client needs.
#[cfg(feature = "http")]
pub async fn run_doc_with_authorities(
    yaml: &str,
) -> (
    DocumentOutcome,
    BTreeMap<String, camel_integration_test::HttpRecorder>,
    BTreeMap<String, String>,
) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("case.test.yaml");
    std::fs::write(&path, yaml).expect("write case file");
    let doc = parse_scenario_document(&path).expect("document must load");
    let (router, recorders, authorities) = bind_doc_partners(&doc).await;
    let wired = wired_refs(&doc);
    let mut vars = ScenarioVars::new();
    fill_bind_vars(&wired, &router, &mut vars);
    let outcome = run_scenario_document(&doc, &router, &mut vars, None).await;
    (outcome, recorders, authorities)
}
