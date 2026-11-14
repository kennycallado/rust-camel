//! Idempotent Consumer EIP — outcome-aware Segment implementation.
//!
//! Implements the Idempotent Consumer pattern at the `OutcomePipeline` layer
//! (one layer above Tower), so that `PipelineOutcome::Stopped` from the child
//! sub-pipeline propagates with the Exchange intact (ADR-0024, ADR-0025).
//!
//! # Why Segment-mode (NOT Process-mode)
//!
//! `compose_pipeline()` translates `PipelineOutcome::Stopped → Ok(ex)` for
//! Tower `Service<Exchange>` consumers. If this EIP were a Tower Service,
//! a duplicate-detected `Stopped` would become `Ok(ex)` and downstream steps
//! would re-process the duplicate. By implementing `OutcomePipeline` directly,
//! the segment returns `PipelineOutcome::Completed(ex)` for duplicates and
//! skips the child sub-pipeline entirely.
//!
//! # Contract C1
//!
//! `IdempotentRepository::contains` returns `Result<bool, CamelError>` because
//! backends (Redis, JDBC) can have transient read failures. The segment
//! propagates `Err` as `PipelineOutcome::Failed` — it NEVER treats a failed
//! read as "not a duplicate."

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use camel_api::{
    BoxValueFuture, CamelError, Exchange, IdempotentRepository, OutcomePipeline, OutcomeSegment,
    PipelineOutcome, Value,
};
use camel_component_api::RuntimeObservability;

/// Exchange property set to `true` when the child sub-pipeline was skipped as
/// a duplicate (contains-hit path or eager race path).
///
/// Implements the spec requirement "Duplicate skips and bookkeeping failures
/// are observable" (`openspec/changes/idemon/specs/eip-idempotency/spec.md`).
pub const CAMEL_IDEMPOTENT_DUPLICATE: &str = "camel.idempotent.duplicate";

/// Fallible message-id source for the idempotent consumer.
///
/// An absent key — `None` from the sync arm, or null/empty from the async arm —
/// is resolved by the segment's [`OnMissingKey`] policy: it is never silently
/// forwarded. This deliberately drops Apache Camel parity, where a keyless
/// exchange bypasses idempotency silently (ADR-0099). The async arm may fail
/// with a [`camel_api::CamelError`]; the failure fails the step before the
/// child runs. Non-scalar values (array/object) are rejected.
// The closure shapes are part of the published contract (mirroring
// camel-api's source enums); keep the signatures literal.
#[allow(clippy::type_complexity)]
#[derive(Clone)]
pub enum MessageIdSource {
    /// Programmatic synchronous extractor.
    Sync(Arc<dyn Fn(&Exchange) -> Option<String> + Send + Sync>),
    /// Language-backed asynchronous expression.
    Async(Arc<dyn Fn(&Exchange) -> BoxValueFuture + Send + Sync>),
}

impl MessageIdSource {
    /// Extract the optional message id, propagating async failures.
    pub async fn message_id(
        &self,
        exchange: &Exchange,
    ) -> Result<Option<String>, camel_api::CamelError> {
        match self {
            // An empty string is not a usable key: treat it exactly like an
            // absent key so it can never collide as a real key.
            Self::Sync(f) => Ok(f(exchange).filter(|s| !s.is_empty())),
            Self::Async(f) => {
                let value = f(exchange).await?;
                Ok(match value {
                    Value::Null => None,
                    Value::String(s) if s.is_empty() => None,
                    Value::String(s) => Some(s),
                    Value::Array(_) | Value::Object(_) => {
                        return Err(camel_api::CamelError::ProcessorError(
                            "message id expression returned a non-scalar value (array/object); expected a string"
                                .into(),
                        ));
                    }
                    other => Some(other.to_string()),
                })
            }
        }
    }
}

/// Policy for an exchange whose idempotency key is absent or empty.
///
/// Implements the spec requirement "Idempotent consumer step reservation
/// semantics" (`openspec/changes/idemon/specs/eip-idempotency/spec.md`): a
/// missing key is never silently forwarded, and an empty string counts as a
/// missing key on both the synchronous and asynchronous arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnMissingKey {
    /// Fail the exchange with an error naming the step and the repository;
    /// the child sub-pipeline does not run. This is the default.
    #[default]
    Fail,
    /// Log a warning and forward the exchange to the child sub-pipeline
    /// without repository interaction.
    Process,
}

impl OnMissingKey {
    /// Parses the canonical/DSL `idempotent_consumer.on_missing_key` knob:
    /// absent or `"fail"` → [`Fail`](Self::Fail), `"process"` →
    /// [`Process`](Self::Process). Any other value fails closed naming the
    /// field and the accepted values.
    pub fn parse(raw: Option<&str>) -> Result<Self, CamelError> {
        match raw {
            None | Some("fail") => Ok(Self::Fail),
            Some("process") => Ok(Self::Process),
            Some(other) => Err(CamelError::Config(format!(
                "idempotent_consumer: invalid on_missing_key '{other}'; must be \"fail\" or \"process\""
            ))),
        }
    }
}

/// Keys whose eager rollback could not be completed, shared by every clone of
/// the segment that owns them.
///
/// `pending` holds keys whose reservation could not be removed after a failed
/// child run; `in_flight` holds the pending key whose removal recovery is
/// currently owned by exactly one clone, and spans the recovery from the
/// ownership take until the key's reservation step resolves (ADR-0099,
/// `openspec/changes/idemon/specs/eip-idempotency/spec.md`).
///
/// Every access is a single critical section: the lock is taken, mutated, and
/// dropped — it is never held across an `.await` (the segment runs as a `Send`
/// future).
#[derive(Default)]
struct PendingRollbacks {
    pending: HashSet<String>,
    in_flight: HashSet<String>,
}

/// Outcome-aware Idempotent Consumer segment.
///
/// Wraps a child sub-pipeline (`OutcomeSegment`) and a named
/// `IdempotentRepository`. On each exchange:
///
/// 1. Extract message-id via `message_id(&exchange)`. `None` → resolve the
///    exchange through `on_missing_key` ([`OnMissingKey::Fail`] fails the step,
///    [`OnMissingKey::Process`] warns and forwards to the child).
/// 2. Pending-rollback recovery, before any repository read: a key already
///    in flight fails loudly; a pending key is taken atomically and its
///    removal retried. On removal failure the key returns to pending and the
///    step fails with the removal error.
/// 3. Check `repo.contains(&key)`:
///    - `Ok(true)` → duplicate, return `Completed(exchange)` (skip child).
///    - `Ok(false)` → proceed.
///    - `Err(e)` → `Failed(e)` (contract C1).
/// 4. If `eager`: `repo.add(&key)` before child. `Ok(false)` = race duplicate
///    → `Completed`. `Err` → `Failed`.
/// 5. Run child pipeline.
/// 6. Post-child:
///    - `Completed` + non-eager → `repo.add(&key)` (best-effort).
///    - `Stopped` → no add (pipeline terminating).
///    - `Failed` + `eager` + `remove_on_failure` → `repo.remove(&key)`; a
///      failed removal queues the key for pending-rollback recovery.
///
/// The in-flight marker is cleared as soon as the reservation step resolves
/// (the pre-child `add` in eager mode, the post-child `add`/no-add in lazy
/// mode), so a settled key never remains in flight.
///
/// # Eager mode + Stopped
///
/// In eager mode, the key is added BEFORE the child pipeline runs. If the child
/// returns `Stopped`, the key remains registered — subsequent deliveries of the
/// same message ID are treated as duplicates and return `Completed` immediately.
/// This is intentional: a Stop indicates the route chose to terminate, and
/// re-processing the same message would be incorrect.
pub struct IdempotentConsumerSegment {
    repository: Arc<dyn IdempotentRepository>,
    message_id: MessageIdSource,
    child_pipeline: OutcomeSegment,
    eager: bool,
    remove_on_failure: bool,
    on_missing_key: OnMissingKey,
    rt: Arc<dyn RuntimeObservability>,
    rollback_state: Arc<Mutex<PendingRollbacks>>,
}

impl IdempotentConsumerSegment {
    /// Build a new segment from a resolved repository, message-id extractor,
    /// child sub-pipeline, and behaviour flags.
    pub fn new(
        repository: Arc<dyn IdempotentRepository>,
        message_id: MessageIdSource,
        child_pipeline: OutcomeSegment,
        eager: bool,
        remove_on_failure: bool,
        on_missing_key: OnMissingKey,
        rt: Arc<dyn RuntimeObservability>,
    ) -> Self {
        Self {
            repository,
            message_id,
            child_pipeline,
            eager,
            remove_on_failure,
            on_missing_key,
            rt,
            rollback_state: Arc::new(Mutex::new(PendingRollbacks::default())),
        }
    }

    /// Drop the in-flight marker for `key`, if set.
    ///
    /// Called at every resolution point of the reservation step (fail paths
    /// and success paths alike): a marker left behind would wedge every later
    /// delivery of that key into the recovery-in-progress error.
    fn clear_in_flight(&self, key: &str) {
        self.lock_rollback_state().in_flight.remove(key);
    }

    /// Lock the shared rollback state, recovering it if a previous holder
    /// panicked (a poisoned `HashSet` is still a consistent set).
    fn lock_rollback_state(&self) -> std::sync::MutexGuard<'_, PendingRollbacks> {
        self.rollback_state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }
}

impl Clone for IdempotentConsumerSegment {
    fn clone(&self) -> Self {
        Self {
            repository: Arc::clone(&self.repository),
            message_id: self.message_id.clone(),
            child_pipeline: self.child_pipeline.clone(),
            eager: self.eager,
            remove_on_failure: self.remove_on_failure,
            on_missing_key: self.on_missing_key,
            rt: Arc::clone(&self.rt),
            rollback_state: Arc::clone(&self.rollback_state),
        }
    }
}

impl OutcomePipeline for IdempotentConsumerSegment {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        Box::pin(async move {
            // 1. Extract message-id. A missing key is resolved by the
            //    on_missing_key policy; a failed evaluation fails the step
            //    (contract C1).
            let key = match self.message_id.message_id(&exchange).await {
                Ok(Some(k)) => k,
                Ok(None) => match self.on_missing_key {
                    OnMissingKey::Fail => {
                        return PipelineOutcome::Failed(CamelError::ProcessorError(format!(
                            "idempotent_consumer: no idempotency key on the exchange and on_missing_key is 'fail' (repository '{}')",
                            self.repository.name()
                        )));
                    }
                    OnMissingKey::Process => {
                        tracing::warn!(
                            repository = %self.repository.name(),
                            "idempotent_consumer: no idempotency key on the exchange; processing without idempotency (on_missing_key = 'process')"
                        );
                        return self.child_pipeline.run(exchange).await;
                    }
                },
                Err(e) => return PipelineOutcome::Failed(e),
            };

            // 2. Pending-rollback recovery (spec Requirement "Failed rollback
            //    fails the exchange and recovery is single-owner"): before any
            //    repository read, a key already under recovery fails loudly and
            //    a pending key is taken by exactly one clone. One critical
            //    section decides ownership; the guard is dropped before the
            //    removal `.await`.
            let recovery_owner = {
                let mut state = self.lock_rollback_state();
                if state.in_flight.contains(&key) {
                    return PipelineOutcome::Failed(CamelError::ProcessorError(format!(
                        "idempotent_consumer: key is under pending-rollback recovery (repository '{}')",
                        self.repository.name()
                    )));
                }
                if state.pending.remove(&key) {
                    state.in_flight.insert(key.clone());
                    true
                } else {
                    false
                }
            };

            if recovery_owner {
                match self.repository.remove(&key).await {
                    Ok(()) => {
                        // The in-flight marker stays held: this clone owns the
                        // recovery until its reservation step resolves (step 4
                        // below).
                    }
                    Err(remove_err) => {
                        {
                            let mut state = self.lock_rollback_state();
                            state.in_flight.remove(&key);
                            state.pending.insert(key.clone());
                        }
                        return PipelineOutcome::Failed(remove_err);
                    }
                }
            }

            // 3. Check repository (contract C1: propagate Err, never treat as non-duplicate).
            match self.repository.contains(&key).await {
                Ok(true) => {
                    self.clear_in_flight(&key);
                    let mut exchange = exchange;
                    exchange.set_property(CAMEL_IDEMPOTENT_DUPLICATE, Value::Bool(true));
                    // allow-open-label rc-36ujy (repository: user-declared idempotent repository name)
                    self.rt.metrics().record_counter(
                        "camel.idempotent.duplicates",
                        1.0_f64,
                        &[("repository", self.repository.name())],
                    );
                    return PipelineOutcome::Completed(exchange);
                }
                Ok(false) => {}
                Err(e) => {
                    self.clear_in_flight(&key);
                    return PipelineOutcome::Failed(e);
                }
            }

            // 4. Eager mode: reserve the key before running the child.
            if self.eager {
                match self.repository.add(&key).await {
                    Ok(true) => {
                        // Reservation resolved: the marker's span ends here.
                        self.clear_in_flight(&key);
                    }
                    Ok(false) => {
                        // Race duplicate: a concurrent delivery reserved first.
                        self.clear_in_flight(&key);
                        let mut exchange = exchange;
                        exchange.set_property(CAMEL_IDEMPOTENT_DUPLICATE, Value::Bool(true));
                        // allow-open-label rc-36ujy (repository: user-declared idempotent repository name)
                        self.rt.metrics().record_counter(
                            "camel.idempotent.duplicates",
                            1.0_f64,
                            &[("repository", self.repository.name())],
                        );
                        return PipelineOutcome::Completed(exchange);
                    }
                    Err(e) => {
                        self.clear_in_flight(&key);
                        return PipelineOutcome::Failed(e);
                    }
                }
            }

            // 5. Run child sub-pipeline.
            let outcome = self.child_pipeline.run(exchange).await;

            // 6. Post-child bookkeeping (best-effort; log transient errors).
            match &outcome {
                PipelineOutcome::Completed(_) if !self.eager => {
                    if let Err(e) = self.repository.add(&key).await {
                        // log-policy: system-broken
                        tracing::error!(
                            error = %e,
                            repository = %self.repository.name(),
                            key = %key,
                            "idempotent repository add failed post-success; next delivery may re-process"
                        );
                        // allow-open-label rc-36ujy (repository: user-declared idempotent repository name)
                        self.rt.metrics().record_counter(
                            "camel.idempotent.add_failures",
                            1.0_f64,
                            &[("repository", self.repository.name())],
                        );
                    }
                }
                PipelineOutcome::Failed(child_err) if self.eager && self.remove_on_failure => {
                    if let Err(remove_err) = self.repository.remove(&key).await {
                        {
                            let mut state = self.lock_rollback_state();
                            // defensive: in eager mode the marker was already cleared at add resolution
                            state.in_flight.remove(&key);
                            state.pending.insert(key.clone());
                        }
                        // log-policy: system-broken
                        tracing::error!(
                            child_error = %child_err,
                            rollback_error = %remove_err,
                            repository = %self.repository.name(),
                            key = %key,
                            "idempotent repository remove failed on failure rollback; key is queued for pending-rollback recovery"
                        );
                    }
                }
                _ => {}
            }

            // Lazy mode: the in-flight marker spans the post-child bookkeeping
            // (add success, add failure with the success kept, or a non-success
            // outcome where no add happens) and ends here.
            if !self.eager {
                self.clear_in_flight(&key);
            }

            outcome
        })
    }
}

#[cfg(test)]
#[path = "idempotent_consumer_tests.rs"]
mod tests;
