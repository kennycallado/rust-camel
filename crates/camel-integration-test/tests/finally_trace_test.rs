//! Teardown execution-order trace (scenario-finally-hooks task 1.4).
//!
//! Documents run through the shared install-first helper
//! ([`common::run_logs_document`]) under [`common::RUN_LOCK`]: capture
//! windows are process-global. The trace locks the execution order of
//! the document lifecycle — actions, then the `logs:` evaluation, then
//! the (possibly absent) fixed verdict, then teardown — and pins the
//! absence-as-witness rule: a failed logs evaluation fixes no verdict.

mod common;

use camel_integration_test::{ScenarioEvent, ScenarioVerdict};
use common::{lock_run, run_logs_document};

/// A passing logs document with one teardown item: the full lifecycle
/// reads action → logs evaluation → fixed verdict → teardown.
const PASSING_FINALLY_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
    body: cache served HIT
logs:
  contains: ['cache served HIT']
finally:
- sleep:
    duration: 1ms
"#;

/// A failing logs document (marker never logged) with one teardown
/// item: the evaluation is witnessed, the verdict stays unfixed, and
/// teardown still runs.
const FAILING_FINALLY_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
    body: cache served HIT
logs:
  contains: ['cache served Miss']
finally:
- sleep:
    duration: 1ms
"#;

#[allow(clippy::await_holding_lock)] // RUN_LOCK serializes document runs; the guard outlives the run
#[tokio::test]
async fn logs_evaluated_trace_ordering() {
    let _guard = lock_run();
    let outcome = run_logs_document(PASSING_FINALLY_DOC, "marker-info.routes.yaml").await;
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "the marker must pass: {outcome:?}"
    );
    assert!(outcome.logs_failure.is_none(), "{outcome:?}");
    assert_eq!(
        outcome.trace,
        vec![
            ScenarioEvent::Action(0),
            ScenarioEvent::LogsEvaluated,
            ScenarioEvent::VerdictFixed,
            ScenarioEvent::Teardown(0),
        ],
        "logs evaluation is witnessed between the last action and the fixed verdict; teardown is last"
    );
}

#[allow(clippy::await_holding_lock)] // RUN_LOCK serializes document runs; the guard outlives the run
#[tokio::test]
async fn failed_logs_leave_no_verdict_fixed() {
    let _guard = lock_run();
    let outcome = run_logs_document(FAILING_FINALLY_DOC, "marker-info.routes.yaml").await;
    assert_eq!(
        outcome.verdict, None,
        "a failed logs evaluation forces the verdict None: {outcome:?}"
    );
    assert!(
        outcome.logs_failure.is_some(),
        "the diagnostic must be present: {outcome:?}"
    );
    assert_eq!(
        outcome.trace,
        vec![
            ScenarioEvent::Action(0),
            ScenarioEvent::LogsEvaluated,
            ScenarioEvent::Teardown(0),
        ],
        "no VerdictFixed: the absence is the witness; teardown still ran"
    );
    assert_eq!(
        outcome.per_teardown[0],
        Ok(()),
        "teardown ran despite the failed logs evaluation"
    );
}
