//! The foreign-subscriber contract (rc-tdgh5), in its own binary: this
//! binary installs a `tracing_subscriber::fmt()` subscriber FIRST, so
//! the harness's capture subscriber loses the process's first-wins
//! `try_init`. A document with a `logs:` block must then fail through
//! the apparatus class `LogCaptureUnavailable` — never silently skip
//! the assertions.

mod common;

use camel_integration_test::ScenarioFailure;
use common::{lock_run, run_logs_document};

const MARKER_INFO_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
    body: cache served HIT
logs:
  contains: ['cache served HIT']
"#;

#[allow(clippy::await_holding_lock)] // RUN_LOCK serializes document runs; the guard outlives the run
#[tokio::test]
async fn foreign_subscriber_is_apparatus_error() {
    // The foreign subscriber takes the seat before anything the
    // harness controls runs.
    tracing_subscriber::fmt()
        .try_init()
        .expect("the foreign subscriber installs first in this binary");
    let _guard = lock_run();
    let outcome = run_logs_document(MARKER_INFO_DOC, "marker-info.routes.yaml").await;
    let failure = outcome
        .per_action
        .iter()
        .find_map(|result| result.as_ref().err())
        .expect("the apparatus failure must be recorded");
    assert!(
        matches!(failure, ScenarioFailure::LogCaptureUnavailable { .. }),
        "expected LogCaptureUnavailable, got {failure}"
    );
    let rendered = failure.to_string();
    assert!(
        rendered.contains("log-capture-unavailable"),
        "the class token must lead the diagnostic: {rendered}"
    );
    assert!(
        rendered.contains("foreign"),
        "the diagnostic must name the foreign-subscriber condition: {rendered}"
    );
    assert_eq!(outcome.verdict, None);
    assert!(
        outcome.logs_failure.is_none(),
        "no logs evaluation ran: {outcome:?}"
    );
}
