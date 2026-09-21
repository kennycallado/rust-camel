//! Document-level `logs:` assertions end to end (rc-tdgh5).
//!
//! Every document runs through the shared install-first helper
//! ([`common::run_logs_document`]) under [`common::RUN_LOCK`]: capture
//! windows are process-global, so document runs serialize inside this
//! binary. The scenarios cover the marker vocabulary (`contains`
//! substrings, unanchored `regex`, the `noLevelAbove` ceiling), window
//! attribution (outside-window events never satisfy; sequential
//! documents persist), the malformed-block load errors, and the
//! multi-thread harness-noise case.

mod common;

use camel_integration_test::{DocError, ScenarioVerdict, parse_scenario_document};
use common::{lock_run, run_logs_document};

/// The pass case: one send, the route logs the body at INFO, the
/// `contains` marker matches the composite message.
const MARKER_INFO_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
    body: cache served HIT
logs:
  contains: ['cache served HIT']
"#;

/// Marker miss: the entry names a body the route never logged.
const MARKER_MISS_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
    body: cache served HIT
logs:
  contains: ['cache served Miss']
"#;

/// Regex vocabulary: the unanchored pattern matches the composite
/// message around the body.
const REGEX_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
    body: processor 7 emitted order
logs:
  regex: ['processor .*emitted']
"#;

/// Clean window at INFO: the route logs at INFO, so the `info`
/// ceiling holds.
const CLEAN_LEVEL_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
    body: cache served HIT
logs:
  contains: ['cache served HIT']
  noLevelAbove: info
"#;

/// The route logs at WARN under an `info` ceiling: the diagnostic must
/// carry the warn level, the `camel_log` target, and the marker
/// message.
const ROUTE_WARN_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
    body: route warn marker body
logs:
  noLevelAbove: info
"#;

/// Multi-thread noise: the document sleeps before the warn-triggering
/// send, so the harness noise loop below certainly lands inside the
/// open window.
const MULTI_THREAD_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- sleep: {duration: 250ms}
- send:
    to: direct:start
    body: route warn marker body
logs:
  noLevelAbove: info
"#;

/// Vacuous pass: no route activity, an empty window satisfies the
/// `warn` ceiling.
const EMPTY_WINDOW_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- sleep: {duration: 10ms}
logs:
  noLevelAbove: warn
"#;

/// Outside-window marker: the `contains` entry can only match an
/// event the test emits BEFORE the document run.
const OUTSIDE_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
    body: cache served HIT
logs:
  contains: ['outside marker']
"#;

/// Sequential document A.
const SEQUENTIAL_A_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
    body: alpha marker A
logs:
  contains: ['alpha marker A']
"#;

/// Sequential document B: distinct marker; the boot exercises the
/// composition root's warn-and-skip (capture already owns the seat).
const SEQUENTIAL_B_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
    body: beta marker B
logs:
  contains: ['beta marker B']
"#;

#[allow(clippy::await_holding_lock)] // RUN_LOCK serializes document runs; the guard outlives the run
#[tokio::test]
async fn contains_passes_on_window_event() {
    let _guard = lock_run();
    let outcome = run_logs_document(MARKER_INFO_DOC, "marker-info.routes.yaml").await;
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "the marker must pass: {outcome:?}"
    );
    assert!(outcome.logs_failure.is_none());
}

#[allow(clippy::await_holding_lock)] // RUN_LOCK serializes document runs; the guard outlives the run
#[tokio::test]
async fn contains_fails_without_match() {
    let _guard = lock_run();
    let outcome = run_logs_document(MARKER_MISS_DOC, "marker-info.routes.yaml").await;
    assert_eq!(outcome.verdict, None, "the miss must fail: {outcome:?}");
    let failure = outcome
        .logs_failure
        .as_deref()
        .expect("the diagnostic must be present");
    assert!(
        failure.contains("`logs.contains` entry `cache served Miss` matched no captured event"),
        "the diagnostic must name the entry: {failure}"
    );
}

#[allow(clippy::await_holding_lock)] // RUN_LOCK serializes document runs; the guard outlives the run
#[tokio::test]
async fn regex_matches_window_event() {
    let _guard = lock_run();
    let outcome = run_logs_document(REGEX_DOC, "processor-marker.routes.yaml").await;
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "the unanchored regex must match: {outcome:?}"
    );
    assert!(outcome.logs_failure.is_none());
}

#[allow(clippy::await_holding_lock)] // RUN_LOCK serializes document runs; the guard outlives the run
#[tokio::test]
async fn no_level_above_clean_window_passes() {
    let _guard = lock_run();
    let outcome = run_logs_document(CLEAN_LEVEL_DOC, "marker-info.routes.yaml").await;
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "an INFO-only window must satisfy the info ceiling: {outcome:?}"
    );
    assert!(outcome.logs_failure.is_none());
}

#[allow(clippy::await_holding_lock)] // RUN_LOCK serializes document runs; the guard outlives the run
#[tokio::test]
async fn no_level_above_fails_on_route_warn() {
    let _guard = lock_run();
    let outcome = run_logs_document(ROUTE_WARN_DOC, "marker-warn.routes.yaml").await;
    assert_eq!(
        outcome.verdict, None,
        "the route warn must fail: {outcome:?}"
    );
    let failure = outcome
        .logs_failure
        .as_deref()
        .expect("the diagnostic must be present");
    assert!(
        failure.contains("`logs.noLevelAbove`"),
        "the diagnostic must name the clause: {failure}"
    );
    assert!(failure.contains("WARN"), "level named: {failure}");
    assert!(
        failure.contains("camel_component_log"),
        "camel-log target named: {failure}"
    );
    assert!(
        failure.contains("route warn marker body"),
        "marker message named: {failure}"
    );
}

#[allow(clippy::await_holding_lock)] // RUN_LOCK serializes document runs; the guard outlives the run
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_thread_document_fails_on_warn() {
    let _guard = lock_run();
    // Harness noise on a second worker thread while the document is
    // in flight: the capture layer is process-global, so the spawned
    // task's warns land in the open window alongside the route's own.
    // The loop stops when the sender drops (document complete); the
    // document's leading sleep keeps the window open across many
    // iterations.
    let (done, mut running) = tokio::sync::oneshot::channel::<()>();
    let noise = tokio::spawn(async move {
        // 30x the 5ms poll sleep, floored at 10s (ADR-0069 §13.2 R1).
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                tracing::warn!("harness task warn marker");
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                // Stop when the document completed (the sender dropped).
                if matches!(
                    running.try_recv(),
                    Err(tokio::sync::oneshot::error::TryRecvError::Closed)
                ) {
                    break;
                }
            }
        })
        .await
        .expect("harness warn-log loop must stop within 10s once the document completes");
    });
    let outcome = run_logs_document(MULTI_THREAD_DOC, "marker-warn.routes.yaml").await;
    drop(done);
    let _ = noise.await;

    assert_eq!(outcome.verdict, None, "both warns must fail: {outcome:?}");
    let failure = outcome
        .logs_failure
        .as_deref()
        .expect("the diagnostic must be present");
    assert!(
        failure.contains("harness task warn marker"),
        "the spawned-task warn must be named: {failure}"
    );
    assert!(
        failure.contains("route warn marker body"),
        "the route warn must be named: {failure}"
    );
}

#[allow(clippy::await_holding_lock)] // RUN_LOCK serializes document runs; the guard outlives the run
#[tokio::test]
async fn no_level_above_empty_window_passes() {
    let _guard = lock_run();
    let outcome = run_logs_document(EMPTY_WINDOW_DOC, "marker-warn.routes.yaml").await;
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "an empty window satisfies the ceiling vacuously: {outcome:?}"
    );
    assert!(outcome.logs_failure.is_none());
}

#[allow(clippy::await_holding_lock)] // RUN_LOCK serializes document runs; the guard outlives the run
#[tokio::test]
async fn outside_window_never_satisfies() {
    let _guard = lock_run();
    // Emitted before the document run: no window is open (the lock
    // serializes runs, so earlier documents' windows are closed), and
    // attribution is conservative — a window never sees earlier
    // events.
    tracing::info!("outside marker");
    let outcome = run_logs_document(OUTSIDE_DOC, "marker-info.routes.yaml").await;
    assert_eq!(outcome.verdict, None, "the miss must fail: {outcome:?}");
    let failure = outcome
        .logs_failure
        .as_deref()
        .expect("the diagnostic must be present");
    assert!(
        failure.contains("`logs.contains` entry `outside marker` matched no captured event"),
        "the diagnostic must name the entry: {failure}"
    );
}

#[allow(clippy::await_holding_lock)] // RUN_LOCK serializes document runs; the guard outlives the run
#[tokio::test]
async fn sequential_documents_capture_persists() {
    let _guard = lock_run();
    let outcome_a = run_logs_document(SEQUENTIAL_A_DOC, "marker-info.routes.yaml").await;
    assert_eq!(
        outcome_a.verdict,
        Some(ScenarioVerdict::Pass),
        "document A must pass: {outcome_a:?}"
    );
    // B's boot re-runs the composition root, whose subscriber install
    // loses first-wins again (warn-and-skip): capture persists across
    // documents.
    let outcome_b = run_logs_document(SEQUENTIAL_B_DOC, "marker-info.routes.yaml").await;
    assert_eq!(
        outcome_b.verdict,
        Some(ScenarioVerdict::Pass),
        "document B must pass after re-boot: {outcome_b:?}"
    );
}

#[test]
fn malformed_logs_block_load_errors() {
    let _guard = lock_run();
    // (text, the offending clause the error must name)
    let cases: &[(&str, &str)] = &[
        // Unknown key.
        (
            r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
logs:
  contains: ['x']
  bogus: 1
"#,
            "bogus",
        ),
        // Level outside the accepted set.
        (
            r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
logs:
  noLevelAbove: verbose
"#,
            "verbose",
        ),
        // Regex that does not compile.
        (
            r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
logs:
  regex: ['[']
"#,
            "`[`",
        ),
    ];
    for (text, named) in cases {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("case.test.yaml");
        std::fs::write(&path, text).expect("write case file");
        let err = parse_scenario_document(&path)
            .expect_err("a malformed logs block must be a load error");
        assert!(
            matches!(err, DocError::LogsBlock { .. }),
            "expected LogsBlock, got {err}"
        );
        assert!(
            err.to_string().contains(named),
            "the error must name the offending clause {named}: {err}"
        );
    }
}
