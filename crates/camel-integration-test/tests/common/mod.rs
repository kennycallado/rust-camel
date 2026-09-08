//! Shared helpers of the log-assertion test binaries (rc-tdgh5).
//!
//! [`RUN_LOCK`] serializes document runs inside one test binary:
//! capture windows are process-global, and overlapping runs would
//! attribute events across documents. [`run_logs_document`] is the
//! install-first run helper: the capture subscriber is ensured BEFORE
//! `boot_scenario`, so the harness wins the process's first-wins
//! `try_init` — unless the test binary installed a foreign subscriber
//! first (the `log_foreign_subscriber_test` contract), in which case
//! capture loses and the document fails with `LogCaptureUnavailable`.

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
    let outcome = run_scenario_document(&doc, &router, &mut vars).await;

    let mut guard = ctx.lock().await;
    run.boot
        .shutdown(&mut guard)
        .await
        .expect("clean shutdown must complete");
    outcome
}

/// The endpoint references a document wires (send targets, receive
/// sources, `lastReceived` validate keys) — the same walk the CLI
/// driver and the scripting test use for `fill_bind_vars`.
fn wired_refs(doc: &camel_integration_test::ScenarioDocument) -> Vec<EndpointRef> {
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
