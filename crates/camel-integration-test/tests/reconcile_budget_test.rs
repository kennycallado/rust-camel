//! Reconcile-budget reproduction (rhai-reconcile-bench task 1.3): drives
//! the reconstructed customer workload — eleven large exchange
//! properties, ten reconcile phases, one rhai script per 128-member
//! chunk — through the scenario tier's `direct:` stimulus and records
//! the per-phase log-capture marks.
//!
//! Two tests share one runner:
//!
//! - [`reconcile_budget_1200_control_replies_expected_shape`] is the CI
//!   control: 1200 members, verdict Pass proves the fixture's
//!   `expectReply` match on `{members: 1200, clusters: 1200,
//!   retiredIds: 0, errors: 0}`, so the routes boot, the generator runs,
//!   the reconcile loops complete and the oracle commits its reply.
//! - [`reconcile_budget_5001_bench`] is the `#[ignore]`d 5001-member
//!   bench. It asserts the same document outcome plus the four
//!   `cb-phase-` markers (missing marker = fixture/harness defect) and
//!   prints total wall clock, the generator/reconcile/oracle phase
//!   durations and the rendered reply. It makes NO timing assertion:
//!   5001 exceeding the CI-friendly budget is the reproduction's point.
//!
//! The bench recipe (release, ignored tests only):
//!
//! ```text
//! cargo test -p camel-integration-test --release --features lang-rhai \
//!   --test reconcile_budget_test -- --ignored --nocapture
//! ```
//!
//! Historical comparison recipe (one worktree per tag — `v0.56.0`,
//! `v0.57.0`, `v0.61.0`, `v0.66.0` — the tags predate this harness):
//! `git worktree add <scratch> <tag>`, copy
//! `crates/camel-integration-test/tests/fixtures/reconcile/` unchanged
//! into the tag tree, build that tree's CLI in release
//! (`cargo build --release -p camel-cli` or the tag-era equivalent),
//! then run `camel test stable-singletons-5001.test.yaml` from a scratch
//! directory holding the copied fixtures. Do not edit the copied
//! fixtures.
#![cfg(feature = "lang-rhai")]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use camel_component_api::test_support::acquire_deadline;
use camel_integration_test::log_capture::open_window;
use camel_integration_test::runner::fill_bind_vars;
use camel_integration_test::{
    DirectStimulus, DocumentOutcome, LayeredEnv, LogEvent, ScenarioAction, ScenarioDocument,
    ScenarioFailure, ScenarioVars, ScenarioVerdict, ambient_std, boot_scenario,
    ensure_capture_subscriber, parse_scenario_document, run_scenario_document,
};
use tokio::sync::Mutex;

use common::lock_run;

/// The reconcile fixture project root (routes + scenario documents);
/// `routeFiles` and the boot root both anchor here.
fn reconcile_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/reconcile")
}

/// The exact endpoint URI the document's send addresses — the router's
/// declared-key contract dispatches on this string verbatim, query
/// included (`timeout_ms=600000` is part of the key the booted
/// `direct:` endpoint is created under).
fn send_endpoint(doc: &ScenarioDocument) -> &str {
    doc.scenario
        .iter()
        .find_map(|action| match action {
            ScenarioAction::Send { to, .. } => Some(to.endpoint.as_str()),
            _ => None,
        })
        .expect("the reconcile document must declare one send")
}

/// Boots the reconcile project, runs `doc_name` through the
/// `DirectStimulus` router, and returns the outcome, the wall clock
/// measured around the document run, and the events captured across
/// boot-independent run + shutdown.
///
/// The caller holds [`common::RUN_LOCK`] (acquired here for the whole
/// run): capture windows are process-global, and overlapping documents
/// would attribute events across runs.
#[allow(clippy::await_holding_lock)] // RUN_LOCK serializes document runs; the guard outlives the run
async fn run_reconcile_document(doc_name: &str) -> (DocumentOutcome, Duration, Vec<LogEvent>) {
    let _run_guard = lock_run();
    ensure_capture_subscriber();
    let root = reconcile_root();
    let doc = parse_scenario_document(&root.join(doc_name)).expect("document must load");
    let env = LayeredEnv::new(
        doc.env.clone().unwrap_or_default(),
        BTreeMap::new(),
        doc.env_passthrough.clone().unwrap_or_default(),
        ambient_std(),
    );
    let run = boot_scenario(&doc, &root, &env)
        .await
        .expect("the reconcile boot must succeed");
    let ctx = Arc::new(Mutex::new(run.ctx));
    let router = common::router_for(send_endpoint(&doc), DirectStimulus::new(Arc::clone(&ctx)));
    let wired = common::wired_refs(&doc);
    let mut vars = ScenarioVars::new();
    fill_bind_vars(&wired, &router, &mut vars);

    let window = open_window();
    let started = Instant::now();
    let mut outcome = run_scenario_document(&doc, &router, &mut vars, None).await;
    let elapsed = started.elapsed();

    let mut guard = acquire_deadline(
        &ctx,
        "scenario ctx (reconcile_budget_test)",
        Duration::from_secs(10),
    )
    .await;
    if let Err(e) = run.boot.shutdown(&mut guard).await {
        outcome.final_failure = Some(ScenarioFailure::ShutdownFailure {
            message: e.to_string(),
        });
    }
    let events = window.close();
    (outcome, elapsed, events)
}

/// Asserts the full-pass shape shared by both documents: every executed
/// action Ok, the whole-document verdict Pass (send executed, the
/// `expectReply` equals-matcher compared field-by-field, the
/// reconcile-completion checks inside the oracle all held), and a clean
/// teardown.
fn assert_all_pass(outcome: &DocumentOutcome) {
    assert!(
        outcome.per_action.iter().all(Result::is_ok),
        "every action must pass: {outcome:?}"
    );
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "the reconcile document must reach verdict Pass: {outcome:?}"
    );
    assert!(
        outcome.logs_failure.is_none(),
        "the logs assertion must pass: {outcome:?}"
    );
    assert!(
        outcome.final_failure.is_none(),
        "shutdown must be clean: {outcome:?}"
    );
}

/// The four phase markers the fixture emits in route order; each
/// camel-log line renders `[<marker>] Body: <body>`.
const PHASE_MARKERS: [&str; 4] = [
    "cb-phase-start",
    "cb-phase-generator",
    "cb-phase-reconcile",
    "cb-phase-oracle",
];

/// The CI control: 1200 members against the reconstructed workload must
/// reach verdict Pass — the `expectReply` matcher compares
/// `{members: 1200, clusters: 1200, retiredIds: 0, errors: 0}`
/// field-by-field, and the oracle's four reconcile-completion checks
/// make a no-op reconcile fail instead of passing vacuously.
#[tokio::test]
async fn reconcile_budget_1200_control_replies_expected_shape() {
    let (outcome, elapsed, _events) =
        run_reconcile_document("stable-singletons-1200.test.yaml").await;
    assert_all_pass(&outcome);
    println!(
        "reconcile control (1200 members) wall clock: {} ms",
        elapsed.as_millis()
    );
}

/// The 5001-member bench (`#[ignore]`, release recipe in the module
/// docs): asserts the document outcome and the four phase markers IN
/// ORDER, then prints total wall clock, the
/// generator/reconcile/oracle phase durations and the rendered reply.
/// A missing or out-of-order marker is a fixture/harness defect, not a
/// timing result. NO wall-clock assertion.
#[tokio::test]
#[ignore = "slow test: 5001-member reconcile bench, run in release with --ignored --nocapture"]
async fn reconcile_budget_5001_bench() {
    let (outcome, elapsed, events) =
        run_reconcile_document("stable-singletons-5001.test.yaml").await;
    assert_all_pass(&outcome);

    // Markers in arrival order: the window is append-only, so the
    // filter preserves route order. Any other event is irrelevant.
    let phases: Vec<&LogEvent> = events
        .iter()
        .filter(|event| event.message.contains("cb-phase-"))
        .collect();
    let mut stamps: Vec<&LogEvent> = Vec::with_capacity(PHASE_MARKERS.len());
    for marker in PHASE_MARKERS {
        let event = phases
            .iter()
            .find(|event| event.message.contains(marker))
            .unwrap_or_else(|| {
                panic!(
                    "missing `{marker}` marker in the closed capture window — \
                     fixture/harness defect, not a timing result: {phases:?}"
                )
            });
        if let Some(previous) = stamps.last() {
            assert!(
                previous.at <= event.at,
                "marker `{marker}` arrived before its predecessor: {phases:?}"
            );
        }
        stamps.push(event);
    }
    assert_eq!(
        phases.len(),
        PHASE_MARKERS.len(),
        "exactly the four phase markers must land in the window: {phases:?}"
    );

    let start = stamps[0].at;
    let generator = stamps[1].at.duration_since(start);
    let reconcile = stamps[2].at.duration_since(stamps[1].at);
    let oracle = stamps[3].at.duration_since(stamps[2].at);
    let total = stamps[3].at.duration_since(start);

    let reply = stamps[3]
        .message
        .split_once("Body: ")
        .map(|(_, body)| body)
        .unwrap_or("<reply not rendered>");

    println!(
        "reconcile bench (5001 members) wall clock: {} ms",
        elapsed.as_millis()
    );
    println!("  generator: {} ms", generator.as_millis());
    println!("  reconcile: {} ms", reconcile.as_millis());
    println!("  oracle:    {} ms", oracle.as_millis());
    println!("  total (start..oracle): {} ms", total.as_millis());
    println!("  reply: {reply}");
}
