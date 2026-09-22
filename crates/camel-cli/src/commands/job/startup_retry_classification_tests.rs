//! Characterization tests for the `camel job` send-phase retry
//! classification (`is_retryable_startup_failure`, rc-fr20u).
//!
//! Pins the retry/no-retry outcome for every error kind reachable as
//! the inner pipeline `CamelError` of `attempt_send`, so the
//! structural replacement of the `"not registered"` text sniff cannot
//! regress any reachable outcome.
//!
//! The SEDA no-active-consumers gate is NON-retryable (rc-ucemm): the
//! rejection fires pre-enqueue but inside the caller's pipeline, so a
//! retry re-executes already-run route steps and duplicates their side
//! effects. Every non-gate `EndpointCreationFailed` stays retryable —
//! the direct registration race and the documented queue-full residual.
//!
//! Reachability (production sources of a space-separated
//! "not registered" wording in a pipeline error at send time):
//! - `camel-direct`'s startup race — and it arrives AS
//!   `EndpointCreationFailed` (camel-direct owns the wording).
//! - The function runtime's not-registered failure renders as
//!   `function:not_registered:` (underscore, function_step's
//!   `map_invocation_error`) — never matched by the old sniff.
//! - Route-compile "not registered" errors (`ComponentNotFound` from
//!   step compilers) fire at startup, before the job send loop.

use super::is_retryable_startup_failure;
use camel_api::CamelError;

/// The single-mode gate wording: "has no active consumers". The
/// rejection is pre-enqueue yet inside the caller's pipeline — a retry
/// would replay already-executed route steps (rc-ucemm).
fn gate_single() -> CamelError {
    CamelError::EndpointCreationFailed("SEDA endpoint 'jobs' has no active consumers".into())
}

/// The fanout-mode gate wording: "has no active subscribers". Same
/// pre-enqueue-inside-pipeline rejection as the single-mode gate
/// (rc-ucemm).
fn gate_fanout() -> CamelError {
    CamelError::EndpointCreationFailed("SEDA endpoint 'jobs' has no active subscribers".into())
}

fn direct_startup_race() -> CamelError {
    CamelError::EndpointCreationFailed("direct endpoint 'out' not registered".into())
}

// ---- non-retryable: the SEDA gate fails fast (rc-ucemm) ------------------

#[test]
fn seda_single_mode_gate_is_not_retryable() {
    assert!(
        !is_retryable_startup_failure(&gate_single()),
        "SEDA single-mode gate must fail fast: a retry replays the caller's \
         pipeline and duplicates pre-SEDA side effects (rc-ucemm)"
    );
}

#[test]
fn seda_fanout_gate_is_not_retryable() {
    assert!(
        !is_retryable_startup_failure(&gate_fanout()),
        "SEDA fanout-mode gate must fail fast: a retry replays the caller's \
         pipeline and duplicates pre-SEDA side effects (rc-ucemm)"
    );
}

// ---- retryable: the consumer-startup race family -------------------------

#[test]
fn seda_queue_full_is_retryable() {
    // Documented residual (bd rc-ucemm scope): queue-full shares the
    // EndpointCreationFailed variant and is NOT one of the excluded gate
    // wordings, so it stays retryable.
    let e = CamelError::EndpointCreationFailed("SEDA queue 'jobs' is full (size=10)".into());
    assert!(
        is_retryable_startup_failure(&e),
        "SEDA queue-full is not a gate wording and must stay retryable"
    );
}

#[test]
fn direct_not_registered_race_is_retryable() {
    // The bd rc-fr20u case: direct's "not registered" wording arrives
    // as EndpointCreationFailed, so the typed match IS the structural
    // classification of this race.
    assert!(
        is_retryable_startup_failure(&direct_startup_race()),
        "direct endpoint-not-registered startup race must stay retryable"
    );
}

#[test]
fn generic_endpoint_creation_failure_is_retryable() {
    // Pre-existing breadth of the typed check: ANY endpoint-creation
    // failure is retryable, not just gate/race wordings.
    let e = CamelError::EndpointCreationFailed("unsupported option `nope`".into());
    assert!(is_retryable_startup_failure(&e));
}

// ---- non-retryable: every other reachable pipeline failure ---------------

#[test]
fn function_not_registered_stays_non_retryable() {
    // function_step's map_invocation_error renders NotRegistered as
    // "function:not_registered: {id}" (underscore) — the old text sniff
    // never matched it, and the structural classification keeps it
    // non-retryable: a missing function is configuration drift, not a
    // consumer-startup race.
    let e = CamelError::ProcessorError("function:not_registered: 9f2c1a".into());
    assert!(!is_retryable_startup_failure(&e));
}

#[test]
fn generic_pipeline_failure_stays_non_retryable() {
    let e = CamelError::ProcessorError("step 2 transform failed".into());
    assert!(!is_retryable_startup_failure(&e));
}

#[test]
fn io_failure_stays_non_retryable() {
    let e = CamelError::Io("connection reset by peer".into());
    assert!(!is_retryable_startup_failure(&e));
}

#[test]
fn component_not_found_stays_non_retryable() {
    // Display is "Component not found: kafka" — the old sniff never
    // matched this text either. Registry misses in attempt_send take
    // the transport class (outer Err, retried unconditionally until the
    // deadline), never the pipeline-error classification under test.
    let e = CamelError::ComponentNotFound("kafka".into());
    assert!(!is_retryable_startup_failure(&e));
}

#[test]
fn wrapped_not_registered_text_is_not_sniffed() {
    // POST-fix contract (rc-fr20u): no CamelError variant is classified
    // by its Display text. A ProcessorError merely CARRYING the wording
    // is not retryable — no production producer emits one (direct uses
    // EndpointCreationFailed; the function runtime uses the underscore
    // form), so narrowing the unreachable input changes no reachable
    // outcome while killing the wording-coupling for good.
    let e = CamelError::ProcessorError("endpoint 'x' not registered".into());
    assert!(
        !is_retryable_startup_failure(&e),
        "classification must be structural (variant), not text-sniffed"
    );
}
