//! Characterization tests for the `camel job` send-phase retry
//! classification (`is_retryable_startup_failure`, rc-fr20u).
//!
//! Pins the retry/no-retry outcome for every error kind reachable as
//! the inner pipeline `CamelError` of `attempt_send`, so the
//! structural replacement of the `"not registered"` text sniff cannot
//! regress any reachable outcome.
//!
//! Classification is by TYPED PROVENANCE (rc-3px7o): only a GENUINE SEDA
//! gate rejection — an `EndpointCreationFailedWithSource` whose source
//! chain carries the seda crate's private marker — is NON-retryable
//! (rc-ucemm: the rejection fires pre-enqueue but inside the caller's
//! pipeline, so a retry re-executes already-run route steps and
//! duplicates their side effects). Display text never classifies: a
//! plain-literal `EndpointCreationFailed` that byte-matches a canonical
//! gate wording is a FOREIGN imitation and stays retryable, as does any
//! typed `EndpointCreationFailedWithSource` lacking the marker. Every
//! non-gate `EndpointCreationFailed` stays retryable — the direct
//! registration race and the documented queue-full residual.
//!
//! The genuine-gate rows are BEHAVIORAL: they boot a real
//! [`camel_component_seda::SedaComponent`] endpoint with no consumer and
//! capture the producer's rejection, so non-retryability is pinned
//! against an error that actually carries the marker. The former
//! plain-literal `gate_single()`/`gate_fanout()` helpers that fabricated
//! gates from the wording were removed: those literals now describe
//! retryable foreign imitations (rc-3px7o).
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

use std::sync::Arc;

use camel_api::CamelError;
use camel_component_seda::{SedaComponent, is_no_active_consumers_gate};
use camel_core::CamelContext;

use super::document::{JobBody, JobSendAction};
use super::{SendError, is_retryable_startup_failure, send_with_startup_retry};

fn direct_startup_race() -> CamelError {
    CamelError::EndpointCreationFailed("direct endpoint 'out' not registered".into())
}

// ---- retryable: foreign imitations of the gate wording (rc-3px7o) --------

#[test]
fn foreign_byte_exact_single_imitation_stays_retryable() {
    // A plain-literal variant carrying the byte-exact single-mode gate
    // wording is a FOREIGN imitation: classification is typed, so it is
    // not a gate and stays retryable.
    let e = CamelError::EndpointCreationFailed("SEDA endpoint 'q' has no active consumers".into());
    assert!(
        is_retryable_startup_failure(&e),
        "a plain-literal byte-exact single-gate imitation must stay retryable"
    );
}

#[test]
fn foreign_byte_exact_fanout_imitation_stays_retryable() {
    let e =
        CamelError::EndpointCreationFailed("SEDA endpoint 'q' has no active subscribers".into());
    assert!(
        is_retryable_startup_failure(&e),
        "a plain-literal byte-exact fanout-gate imitation must stay retryable"
    );
}

#[test]
fn foreign_wording_collision_stays_retryable() {
    // A foreign component's own "has no active consumers" wording must
    // never be mistaken for the SEDA gate.
    let e = CamelError::EndpointCreationFailed(
        "kafka topic 'orders' has no active consumers (broker=1)".into(),
    );
    assert!(
        is_retryable_startup_failure(&e),
        "a foreign wording collision must stay retryable"
    );
}

/// Minimal foreign error used as the source of a typed
/// `EndpointCreationFailedWithSource` that carries no gate marker.
#[derive(Debug)]
struct ForeignSource;

impl std::fmt::Display for ForeignSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("foreign source")
    }
}

impl std::error::Error for ForeignSource {}

#[test]
fn typed_non_gate_source_stays_retryable() {
    // A typed source-carrying failure WITHOUT the crate-private gate
    // marker is a plain creation race, not a gate: retryable.
    let e = CamelError::EndpointCreationFailedWithSource(
        "foreign".into(),
        camel_api::OpaqueErrorSource::new(Arc::new(ForeignSource)),
    );
    assert!(
        is_retryable_startup_failure(&e),
        "a typed source without the gate marker must stay retryable"
    );
}

// ---- non-retryable: genuine SEDA gates, captured behaviorally ------------

/// The job send action targeting a `seda:` endpoint with no consumer.
fn tick_send(to: &str) -> JobSendAction {
    JobSendAction {
        to: to.to_string(),
        body: Some(JobBody::Text("tick".to_string())),
        headers: None,
    }
}

/// Boot a started context with a real [`SedaComponent`]. The caller then
/// sends to a `seda:` endpoint with NO consumer, so the producer fires
/// the genuine marker-carrying gate rejection under test.
async fn booted_seda_context() -> CamelContext {
    let mut ctx = CamelContext::builder()
        .build()
        .await
        .expect("context build");
    ctx.register_component(SedaComponent::new());
    ctx.start().await.expect("context start");
    ctx
}

#[tokio::test]
async fn behavioral_single_gate_fails_fast_classification() {
    let ctx = booted_seda_context().await;
    let send = tick_send("seda:worker");
    let e = match send_with_startup_retry(&ctx, &send, "seda:worker", &[]).await {
        Err(SendError::Pipeline(e)) => e,
        Err(SendError::Transport(detail)) => {
            panic!("expected the SEDA gate as a pipeline failure, got transport: {detail}")
        }
        Ok(_) => panic!("expected the SEDA gate as a pipeline failure, got Ok"),
    };
    assert!(
        is_no_active_consumers_gate(&e),
        "the captured error must be the genuine SEDA single-mode gate: {e}"
    );
    assert!(
        !is_retryable_startup_failure(&e),
        "a genuine SEDA single-mode gate must fail fast (non-retryable): {e}"
    );
}

#[tokio::test]
async fn behavioral_fanout_gate_fails_fast_classification() {
    let ctx = booted_seda_context().await;
    let send = tick_send("seda:worker?multipleConsumers=true");
    let e = match send_with_startup_retry(&ctx, &send, "seda:worker?multipleConsumers=true", &[])
        .await
    {
        Err(SendError::Pipeline(e)) => e,
        Err(SendError::Transport(detail)) => {
            panic!("expected the SEDA gate as a pipeline failure, got transport: {detail}")
        }
        Ok(_) => panic!("expected the SEDA gate as a pipeline failure, got Ok"),
    };
    assert!(
        is_no_active_consumers_gate(&e),
        "the captured error must be the genuine SEDA fanout gate: {e}"
    );
    assert!(
        !is_retryable_startup_failure(&e),
        "a genuine SEDA fanout gate must fail fast (non-retryable): {e}"
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
