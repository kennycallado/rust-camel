//! The transacted-block runtime segment (sqltrans-exec step 7, ruling 604d).
//!
//! [`TransactedSegment`] wraps one `transacted { datasource }` block body in
//! the block lifecycle: open a session through the provider-neutral
//! [`TransactionManager`] port, install the [`BLOCK_SESSION`] binding for the
//! body task, race the body against the block deadline and the pipeline
//! cancellation token, then finalize per the 604d pseudocode — commit only
//! through the `state() == Active` R7 gate, otherwise the session's TOTAL
//! `rollback()` (the session selects the action from its private `TxPhase`;
//! the segment never re-derives lifecycle rows).
//!
//! Hexagonal boundary: this module is generic over the `camel-api` port. It
//! never references `camel-sql`; the admission `min(block_abs, acquire_abs)`
//! race and its `PoolExhausted` vs `Timeout` primary mapping live inside the
//! manager's `open_session` (the sql component), and this segment maps the
//! open result onto the exchange outcome. The port's `open_session()` carries
//! no block instant (the production manager arms a stand-in internal one), so
//! a ready `PoolExhausted` that resolves with the real block deadline already
//! spent is re-adjudicated HERE, by absolute instants through
//! [`TransactionManager::acquire_deadline`] (rc-9awnc). Step 9 wires the
//! registry-resolved manager and this segment into the executor; nothing here
//! resolves a registry.
//!
//! Decisions this module carries (all traced to a ruling):
//!
//! - **Open seam.** The segment takes an `Arc<dyn TransactionManager>`; the
//!   port's `open_session()` IS the "async closure returning
//!   `Result<Arc<dyn TransactionSession>, TransactionError>`". The segment
//!   still bounds the open await at `block_abs`: the open arm is polled
//!   first (a resolved open is authoritative — its internal race compares
//!   the absolute instants), and the timer resolves only a still-pending
//!   open (tie included → [`TransactionError::Timeout`] with elapsed
//!   measured from block entry): an open hook that ignores the block
//!   deadline can never hang the block.
//! - **Cancellation.** The pipeline cancellation surface is the per-start
//!   `CANCEL_TOKEN` task-local the executor scopes around `run_steps`
//!   (ADR-0043); `run_steps` already maps an observed cancellation to
//!   [`CamelError::ConsumerStopping`]. The frozen `TransactionError` family
//!   has NO cancellation variant, so the segment's cancellation primary is
//!   `CamelError::ConsumerStopping` — the established convention — never
//!   `Timeout` (a deadline is not a cancellation).
//! - **Cleanup context.** [`CamelError`] has no `with_cleanup` field, so the
//!   604d `(Some(p), Err(u))` row logs the cleanup uncertainty (`warn!`) and
//!   returns the primary — the frozen spec: "the body error is primary and
//!   finalization uncertainty is carried in the outcome, not substituted for
//!   it". The `(None, Err(u))` row reports `u` itself (there is no primary to
//!   protect).
//! - **Scope id.** One id is allocated per block execution (shared counter
//!   across clones) and exposed through the installed [`BlockSession`]; the
//!   executor may force a loader-baked id via
//!   [`TransactedSegment::run_with_scope`] when the `JOIN_SCOPE` baking
//!   decorator lands (step 8/9 wiring).
//!
//! [`BLOCK_SESSION`]: camel_api::transaction::join::BLOCK_SESSION

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use camel_api::CamelError;
use camel_api::Exchange;
use camel_api::OutcomePipeline;
use camel_api::OutcomeSegment;
use camel_api::PipelineOutcome;
use camel_api::TransactionState;
use camel_api::transaction::TransactionError;
use camel_api::transaction::TransactionManager;
use camel_api::transaction::TransactionOutcome;
use camel_api::transaction::TransactionSession;
use camel_api::transaction::join::BlockSession;
use camel_api::transaction::join::JoinScope;
use tokio_util::sync::CancellationToken;

use crate::lifecycle::adapters::route_compiler::CANCEL_TOKEN;

/// Default block deadline measured at block entry (frozen spec "Block
/// timeout": default 30 s, per-block override).
pub const DEFAULT_BLOCK_TIMEOUT: Duration = Duration::from_secs(30);

/// Outcome-aware structural segment for a `transacted { datasource }` block.
///
/// Operates at the [`PipelineOutcome`] layer so a clean `Stopped` from the
/// body propagates unchanged per ADR-0025 — a Stop is successful control
/// flow and MUST NOT become a rollback report (604c R2).
#[derive(Clone)]
pub struct TransactedSegment {
    /// Datasource the block is transacted on; names every typed error.
    pub datasource: String,
    /// Block deadline measured from entry; default [`DEFAULT_BLOCK_TIMEOUT`],
    /// per-block override through [`TransactedSegment::with_block_timeout`].
    pub block_timeout: Duration,
    /// Open seam: `open_session()` checks out + hygienizes the connection
    /// (lazy — `BEGIN` is never sent here). Resolved above this segment.
    pub manager: Arc<dyn TransactionManager>,
    /// The block body pipeline.
    pub body: OutcomeSegment,
    /// Per-execution scope allocation, shared across clones so two runs of
    /// the same compiled block never observe the same scope id.
    scope_seq: Arc<AtomicU64>,
}

impl TransactedSegment {
    /// Build a segment on `datasource` with the default block timeout.
    #[must_use]
    pub fn new(
        datasource: impl Into<String>,
        manager: Arc<dyn TransactionManager>,
        body: OutcomeSegment,
    ) -> Self {
        Self {
            datasource: datasource.into(),
            block_timeout: DEFAULT_BLOCK_TIMEOUT,
            manager,
            body,
            scope_seq: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Override the block deadline (frozen spec "Block timeout": per-block
    /// override of the 30 s default).
    #[must_use]
    pub fn with_block_timeout(mut self, block_timeout: Duration) -> Self {
        self.block_timeout = block_timeout;
        self
    }

    /// Run the block with a runtime-allocated scope id.
    pub async fn run(&mut self, exchange: Exchange) -> PipelineOutcome {
        let scope = JoinScope::new(self.scope_seq.fetch_add(1, Ordering::Relaxed));
        self.run_in(exchange, scope).await
    }

    /// Run the block with a caller-chosen scope id.
    ///
    /// The seam the executor (step 9) uses once joined sql steps carry a
    /// loader-baked scope: the installed [`BlockSession`] must carry the SAME
    /// id the joined steps compare against.
    pub async fn run_with_scope(
        &mut self,
        exchange: Exchange,
        scope: JoinScope,
    ) -> PipelineOutcome {
        self.run_in(exchange, scope).await
    }

    /// One block execution: admission, body race, finalization.
    async fn run_in(&mut self, exchange: Exchange, scope: JoinScope) -> PipelineOutcome {
        // Block deadline at BLOCK ENTRY as an absolute instant (frozen spec
        // "Block timeout": deadlines are compared, never polled).
        let entry = tokio::time::Instant::now();
        let block_abs = entry + self.block_timeout;
        let datasource = self.datasource.clone();

        // Pipeline cancellation surface (ADR-0043): the executor scopes
        // CANCEL_TOKEN around run_steps; absent means "never cancelled",
        // exactly like run_steps' own between-steps check.
        let cancel = CANCEL_TOKEN.try_with(CancellationToken::clone).ok();

        // Admission: checkout + hygiene race `min(block_abs, acquire_abs)`
        // INSIDE the manager's open (604c admission; `PoolExhausted` vs
        // `Timeout` primary mapping happens in camel-sql's open). This
        // segment bounds the await at block_abs: the open arm is polled
        // FIRST, so a resolved open is authoritative — its internal race
        // already compares block_abs/acquire_abs as absolute instants
        // (`PoolExhausted` iff `acquire_abs < block_abs`, frozen spec "Block
        // timeout", never decided by polling order). The timer arm resolves
        // only a STILL-PENDING open (tie included → `Timeout`), expiry drops
        // the open future, so a checked-out connection detaches via the
        // guard (R0). Any open error maps onto the exchange failure and the
        // (partially opened) session is simply discarded.
        //
        // rc-9awnc: the port's `open_session()` carries no block instant, so
        // a manager that arms a stand-in internal block instant cannot decide
        // the both-elapsed tie itself. When a `PoolExhausted` resolves with
        // the REAL block deadline also spent (now >= block_abs), the segment
        // adjudicates by absolute instants — `acquire_abs = open_start +
        // acquire_deadline()`; `block_abs <= acquire_abs` (tie included) →
        // `Timeout` measured from entry, acquire strictly first → the
        // `PoolExhausted` passes through. Soundness: the stand-in instant
        // never elapses first, so `PoolExhausted` always implies the real
        // acquire arm elapsed. A `PoolExhausted` while `now < block_abs` is a
        // genuine acquire expiry and passes through unchanged; non-deadline
        // errors (`RollbackUncertain`, …) are never re-adjudicated.
        let open_start = tokio::time::Instant::now();
        let session: Arc<dyn TransactionSession> = tokio::select! {
            biased;
            opened = self.manager.open_session() => match opened {
                Ok(session) => session,
                Err(e @ TransactionError::PoolExhausted { .. })
                    if tokio::time::Instant::now() >= block_abs =>
                {
                    let acquire_abs = open_start + self.manager.acquire_deadline();
                    if block_abs <= acquire_abs {
                        return PipelineOutcome::Failed(CamelError::from(timeout_error(
                            &datasource,
                            entry,
                        )));
                    }
                    return PipelineOutcome::Failed(CamelError::from(e));
                }
                Err(e) => return PipelineOutcome::Failed(CamelError::from(e)),
            },
            _ = tokio::time::sleep_until(block_abs) => {
                return PipelineOutcome::Failed(CamelError::from(timeout_error(&datasource, entry)));
            }
        };

        // Install BLOCK_SESSION for the body task: datasource, this run's
        // scope id, and the block's own session allocation.
        let block_session =
            BlockSession::new(self.datasource.as_str(), scope, Arc::clone(&session));

        // Body race (604c §A): `biased select { sleep_until(block_deadline),
        // CANCEL_TOKEN.cancelled(), body }` IN THAT ORDER — tie → deadline
        // wins → Timeout.
        let body_end = tokio::select! {
            biased;
            _ = tokio::time::sleep_until(block_abs) => BodyEnd::Deadline,
            _ = cancelled_or_pending(cancel.as_ref()) => BodyEnd::Cancelled,
            outcome = block_session.enter(self.body.run(exchange)) => match outcome {
                PipelineOutcome::Failed(e) => BodyEnd::Err(e),
                // Boxed: PipelineOutcome dwarfs the other variants and the
                // deadline/cancelled arms would carry the padding.
                outcome @ (PipelineOutcome::Completed(_) | PipelineOutcome::Stopped(_)) => {
                    BodyEnd::Ok(Box::new(outcome))
                }
            },
        };

        self.finalize(session, body_end, cancel.as_ref(), entry, block_abs)
            .await
    }

    /// 604d finalization pseudocode, line for line.
    ///
    /// The segment decides commit-vs-rollback ONLY; the session's total
    /// `rollback()` selects the lifecycle action from its private `TxPhase`.
    /// A cleanup result never replaces an existing primary error.
    async fn finalize(
        &self,
        s: Arc<dyn TransactionSession>,
        body_end: BodyEnd,
        cancel: Option<&CancellationToken>,
        entry: tokio::time::Instant,
        block_abs: tokio::time::Instant,
    ) -> PipelineOutcome {
        let datasource = self.datasource.clone();

        // primary = match body_end {                    // precedence lives here, unchanged
        let (mut primary, ok_outcome): (Option<CamelError>, Option<Box<PipelineOutcome>>) =
            match body_end {
                BodyEnd::Deadline => (Some(timeout_error(&datasource, entry).into()), None),
                BodyEnd::Err(e) => (Some(e), None),
                // The frozen TransactionError family has no cancellation variant;
                // the pipeline convention (run_steps) is ConsumerStopping.
                BodyEnd::Cancelled => (Some(CamelError::ConsumerStopping), None),
                BodyEnd::Ok(_) if s.is_rollback_only() => (
                    Some(
                        TransactionError::AbortedByHandledError {
                            datasource: datasource.clone(),
                        }
                        .into(),
                    ),
                    None,
                ),
                BodyEnd::Ok(outcome) => (None, Some(outcome)),
            };

        // if primary.is_none() && s.state() == Active {  // R7 gate
        if primary.is_none() && s.state() == TransactionState::Active {
            // The gate reads the observable state (lossy Fresh is fine: any
            // non-Active state falls through to the total rollback).
            let now = tokio::time::Instant::now();
            // if CANCEL_TOKEN.is_cancelled() || now >= block_deadline {  // R6 cutoff
            if cancel.is_some_and(CancellationToken::is_cancelled) || now >= block_abs {
                // primary = Some(if now >= block_deadline { Timeout } else { Cancelled })
                primary = Some(if now >= block_abs {
                    timeout_error(&datasource, entry).into()
                } else {
                    CamelError::ConsumerStopping
                });
            } else {
                // else { return map_commit(s.commit().await, body_end) }  // R7, no re-consult
                return map_commit(s.commit().await, ok_outcome, &datasource);
            }
        }

        // let r = s.rollback().await;      // R1–R6: session selects by TxPhase
        let r = s.rollback().await;
        // match (primary, r) { ... }
        match (primary, r) {
            // (None, Ok(_))  => body_end_ok_unchanged — R2 clean; Stop
            // propagates per ADR-0025 (a Stop never becomes a rollback report).
            // Unreachable-by-construction: primary None implies the body
            // ended Ok, so the outcome is always present — an `expect` keeps
            // the invariant loud where a silent fallback would have to name a
            // typed error that is a misnomer here (nothing was committed and
            // the rollback was acknowledged).
            (None, Ok(_)) => {
                *ok_outcome.expect("primary None implies the body ended Ok") // allow-unwrap: unreachable invariant; loud panic beats a silent misnamed fallback
            }
            // (None, Err(u)) => Err(u) — e.g. step-local drop left BeginPending
            (None, Err(u)) => PipelineOutcome::Failed(CamelError::from(u)),
            // (Some(p), Ok(_)) => Err(p)
            (Some(p), Ok(_)) => PipelineOutcome::Failed(p),
            // (Some(p), Err(u)) => Err(p).with_cleanup(u) — CamelError carries
            // no cleanup-context field, so the uncertainty is logged and the
            // primary returned (frozen spec: never substituted).
            (Some(p), Err(u)) => {
                tracing::warn!(
                    datasource = %datasource,
                    cleanup = %u,
                    primary = %p,
                    "transacted block cleanup was not acknowledged; the primary error stands"
                );
                PipelineOutcome::Failed(p)
            }
        }
    }
}

impl OutcomePipeline for TransactedSegment {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        // Qualified call: inherent methods shadow trait methods, but the
        // names collide here — select the inherent `run` explicitly.
        Box::pin(TransactedSegment::run(self, exchange))
    }
}

/// The body-race resolution: `BodyEnd ∈ {Ok(Continue), Ok(Stopped), Err(e),
/// Deadline, Cancelled}` (604c §A). `Ok` carries the success outcome —
/// `Completed` or a clean `Stopped`, both preserved unchanged.
enum BodyEnd {
    /// The body returned a success outcome (`Completed` or `Stopped`).
    /// Boxed: `PipelineOutcome` dwarfs the other variants.
    Ok(Box<PipelineOutcome>),
    /// The body failed with an unhandled error.
    Err(CamelError),
    /// The block deadline elapsed first (tie included).
    Deadline,
    /// The pipeline cancellation token fired first.
    Cancelled,
}

/// `Timeout { datasource, elapsed }` measured from BLOCK ENTRY (frozen spec
/// "Block timeout": the typed error names the datasource and elapsed time).
fn timeout_error(datasource: &str, entry: tokio::time::Instant) -> TransactionError {
    TransactionError::Timeout {
        datasource: datasource.to_owned(),
        elapsed_ms: u64::try_from(entry.elapsed().as_millis()).unwrap_or(u64::MAX),
    }
}

/// The race branch for cancellation: the scoped token's `cancelled()` future,
/// or a never-ready future when the task-local is absent (direct calls and
/// unscooped tests never observe a cancellation).
async fn cancelled_or_pending(cancel: Option<&CancellationToken>) {
    match cancel {
        Some(token) => token.cancelled().await,
        None => std::future::pending::<()>().await,
    }
}

/// map_commit: commit Ok + body Ok/Stopped → the block's success outcome
/// (Stopped propagates unchanged per ADR-0025); commit Err(e) → Err(e) — a
/// cleanup result never replaces a primary, and the session already folded
/// its cleanup into the returned error (the port has no separate cleanup
/// channel for commit).
fn map_commit(
    commit: Result<TransactionOutcome, TransactionError>,
    body_outcome: Option<Box<PipelineOutcome>>,
    datasource: &str,
) -> PipelineOutcome {
    match commit {
        Ok(TransactionOutcome::Committed) => {
            body_outcome.map(|boxed| *boxed).unwrap_or_else(|| {
                // log-policy: system-broken
                tracing::error!(
                    datasource,
                    "commit acknowledged but the body outcome was lost; reporting CommitUncertain"
                );
                PipelineOutcome::Failed(CamelError::Transaction(
                    TransactionError::CommitUncertain {
                        datasource: datasource.to_owned(),
                    },
                ))
            })
        }
        Ok(outcome) => {
            // log-policy: system-broken
            tracing::error!(
                datasource,
                ?outcome,
                "commit resolved to a non-Committed outcome; reporting CommitUncertain"
            );
            PipelineOutcome::Failed(CamelError::Transaction(TransactionError::CommitUncertain {
                datasource: datasource.to_owned(),
            }))
        }
        Err(e) => PipelineOutcome::Failed(CamelError::from(e)),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;
    use std::task::Poll;
    use std::time::Duration;

    use camel_api::Message;
    use camel_api::transaction::join::current_block_session;
    use camel_component_api::test_support::log_capture;

    use super::*;

    const DS: &str = "pg";

    /// Install a bare registry as the process-global tracing default, once
    /// per test binary. Guards the `capture_logs_async` test against
    /// callsite-interest poisoning: `tracing` caches each callsite's
    /// `Interest` process-wide from its FIRST macro execution, evaluated
    /// against the executing thread's dispatcher. Subscriber-less sibling
    /// tests in this binary reach the segment's cleanup `warn!` callsite
    /// first and cache `Interest::never`, so a later thread-local
    /// `set_default` capture silently drops the event. The global registry
    /// heals prior poison and floors future interest evaluations at
    /// `sometimes` (fix pattern: c3853198; bd rc-img5).
    fn ensure_global_tracing_default() {
        static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        if INIT.set(()).is_ok() {
            let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
        }
    }

    // ── Fake session ────────────────────────────────────────────────────

    /// Scripted [`TransactionSession`]: the segment's fixture. `state` is
    /// whatever the test wants the lossy projection to report; the
    /// `rollback()`/`commit()` scripts stand in for the session's private
    /// `TxPhase` machinery (implemented for real by `SqlTxSession`).
    struct FakeSession {
        commit_result: Result<TransactionOutcome, TransactionError>,
        rollback_result: Result<TransactionOutcome, TransactionError>,
        state: TransactionState,
        rollback_only: AtomicBool,
        calls: Mutex<Vec<&'static str>>,
        /// When set, `commit()` stays pending until the token fires: the
        /// cancellation-mid-commit seam for the frozen post-cutoff fixtures.
        commit_gate: Option<CancellationToken>,
    }

    impl FakeSession {
        fn new(
            state: TransactionState,
            commit: Result<TransactionOutcome, TransactionError>,
            rollback: Result<TransactionOutcome, TransactionError>,
        ) -> Arc<Self> {
            Arc::new(Self {
                commit_result: commit,
                rollback_result: rollback,
                state,
                rollback_only: AtomicBool::new(false),
                calls: Mutex::new(Vec::new()),
                commit_gate: None,
            })
        }

        /// A session whose `commit()` stays pending until `gate` fires: the
        /// cancellation-mid-commit seam for the frozen post-cutoff fixtures.
        /// The commit resolves with `commit` after the gate; the rollback
        /// script MUST never be reached (R7 never consults cancellation).
        fn gated(
            commit: Result<TransactionOutcome, TransactionError>,
            gate: CancellationToken,
        ) -> Arc<Self> {
            Arc::new(Self {
                commit_result: commit,
                rollback_result: Ok(TransactionOutcome::RolledBack),
                state: TransactionState::Active,
                rollback_only: AtomicBool::new(false),
                calls: Mutex::new(Vec::new()),
                commit_gate: Some(gate),
            })
        }

        fn with_flag(self: Arc<Self>) -> Arc<Self> {
            self.rollback_only.store(true, Ordering::Relaxed);
            self
        }

        fn record(&self, call: &'static str) {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(call);
        }

        fn calls(&self) -> Vec<&'static str> {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl TransactionSession for FakeSession {
        fn datasource(&self) -> &str {
            DS
        }

        fn state(&self) -> TransactionState {
            self.state
        }

        fn begin(&self) -> camel_api::transaction::BeginFuture<'_> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn set_rollback_only(&self) {
            self.rollback_only.store(true, Ordering::Relaxed);
        }

        fn is_rollback_only(&self) -> bool {
            self.rollback_only.load(Ordering::Relaxed)
        }

        fn commit(&self) -> camel_api::transaction::FinalizeFuture<'_> {
            self.record("commit");
            let result = self.commit_result.clone();
            match self.commit_gate.clone() {
                // Cancellation-mid-commit seam: hold the COMMIT until the
                // token fires, then resolve per the script (R7 never
                // re-consults the token).
                Some(gate) => Box::pin(async move {
                    gate.cancelled().await;
                    result
                }),
                None => Box::pin(std::future::ready(result)),
            }
        }

        fn rollback(&self) -> camel_api::transaction::FinalizeFuture<'_> {
            self.record("rollback");
            Box::pin(std::future::ready(self.rollback_result.clone()))
        }
    }

    // ── Fake manager (the open seam) ────────────────────────────────────

    /// Scripted [`TransactionManager`]: what `open_session()` resolves to.
    enum OpenScript {
        /// Hand back an already-built session Arc.
        Session(Arc<dyn TransactionSession>),
        /// The hook itself fails (PoolExhausted / Timeout / … from the
        /// camel-sql admission race). Mapped onto the exchange failure.
        Fail(TransactionError),
        /// Checkout never completes: only the segment's block-abs bound
        /// resolves it.
        Pending,
        /// The hook stays pending on its FIRST poll and fails with the
        /// scripted error from every later poll: at the block-deadline poll
        /// the open future is READY at the same instant the timer fires, so
        /// the race must decide by the open's own instant comparison, not by
        /// polling order.
        FailAfterFirstPoll(TransactionError),
    }

    struct FakeManager {
        script: OpenScript,
        /// What `acquire_deadline()` reports: the segment re-derives the
        /// absolute acquire instant from it when adjudicating a both-elapsed
        /// `PoolExhausted` (rc-9awnc). Mirrors the production manager, whose
        /// internal block instant is a stand-in (`NO_BLOCK_DEADLINE`).
        acquire_deadline: Duration,
    }

    impl FakeManager {
        /// A manager with the production-default 5 s acquire bound.
        fn script(script: OpenScript) -> Self {
            Self {
                script,
                acquire_deadline: Duration::from_secs(5),
            }
        }

        /// Override the reported acquire bound: the absolute-instant
        /// adjudication compares it against the real block deadline.
        fn with_acquire_deadline(mut self, acquire_deadline: Duration) -> Self {
            self.acquire_deadline = acquire_deadline;
            self
        }
    }

    impl TransactionManager for FakeManager {
        fn datasource(&self) -> &str {
            DS
        }

        fn acquire_deadline(&self) -> Duration {
            self.acquire_deadline
        }

        fn open_session(&self) -> camel_api::transaction::OpenSessionFuture<'_> {
            match &self.script {
                OpenScript::Session(session) => {
                    let session = Arc::clone(session);
                    Box::pin(std::future::ready(Ok(session)))
                }
                OpenScript::Fail(e) => {
                    let e = e.clone();
                    Box::pin(std::future::ready(Err(e)))
                }
                OpenScript::Pending => Box::pin(std::future::pending()),
                OpenScript::FailAfterFirstPoll(e) => {
                    let e = e.clone();
                    // Per-call flag: poll #1 pends (the hook is in flight),
                    // every later poll fails — at the block-deadline wake the
                    // open future is READY at the same poll the timer fires.
                    let mut polled = false;
                    Box::pin(std::future::poll_fn(move |_cx| {
                        if polled {
                            Poll::Ready(Err(e.clone()))
                        } else {
                            polled = true;
                            Poll::Pending
                        }
                    }))
                }
            }
        }
    }

    // ── Scripted body ───────────────────────────────────────────────────

    /// What the body pipeline does. An [`OutcomePipeline`] so it fits
    /// [`OutcomeSegment`] exactly like a compiled step chain.
    #[derive(Clone)]
    enum BodyScript {
        /// Complete with `Completed(exchange carrying `text`)`.
        Complete(&'static str),
        /// Complete with `Stopped(exchange carrying `text`)`.
        Stop(&'static str),
        /// Fail with `ProcessorError("body failed")`.
        Fail,
        /// Never resolve — only the deadline branch can win.
        Hang,
        /// Cancel the pipeline token as the body's last act, then complete:
        /// the R6 cutoff sees the flag set after the body race resolved Ok.
        CancelThenComplete(&'static str, CancellationToken),
        /// Read the installed BLOCK_SESSION, record `(datasource, scope)` in
        /// `seen`, prove the session pointer identity, then complete.
        Observe {
            expected_session: Arc<dyn TransactionSession>,
            seen: Arc<Mutex<Option<(String, u64)>>>,
        },
    }

    impl OutcomePipeline for BodyScript {
        fn clone_box(&self) -> Box<dyn OutcomePipeline> {
            Box::new(self.clone())
        }

        fn run<'a>(
            &'a mut self,
            _exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            let script = self.clone();
            Box::pin(async move {
                match script {
                    BodyScript::Complete(text) => {
                        PipelineOutcome::Completed(Exchange::new(Message::new(text)))
                    }
                    BodyScript::Stop(text) => {
                        PipelineOutcome::Stopped(Exchange::new(Message::new(text)))
                    }
                    BodyScript::Fail => PipelineOutcome::Failed(CamelError::ProcessorError(
                        "body failed".to_owned(),
                    )),
                    BodyScript::Hang => std::future::pending::<PipelineOutcome>().await,
                    BodyScript::CancelThenComplete(text, token) => {
                        token.cancel();
                        PipelineOutcome::Completed(Exchange::new(Message::new(text)))
                    }
                    BodyScript::Observe {
                        expected_session,
                        seen,
                    } => {
                        let running = current_block_session()
                            .expect("the body task must see BLOCK_SESSION installed");
                        assert_eq!(
                            running.datasource(),
                            DS,
                            "the binding names the block datasource"
                        );
                        assert!(
                            Arc::ptr_eq(&running.session(), &expected_session),
                            "the body must see the block's own session allocation"
                        );
                        *seen
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) =
                            Some((running.datasource().to_owned(), running.scope().get()));
                        PipelineOutcome::Completed(Exchange::new(Message::new("observed")))
                    }
                }
            })
        }
    }

    // ── Harness ─────────────────────────────────────────────────────────

    fn segment(
        manager: FakeManager,
        body: BodyScript,
        block_timeout: Duration,
    ) -> TransactedSegment {
        TransactedSegment::new(DS, Arc::new(manager), OutcomeSegment::new(Box::new(body)))
            .with_block_timeout(block_timeout)
    }

    fn session_manager(session: Arc<FakeSession>) -> FakeManager {
        FakeManager::script(OpenScript::Session(session))
    }

    fn tx_err(e: &CamelError) -> &TransactionError {
        match e {
            CamelError::Transaction(tx) => tx,
            other => panic!("expected a Transaction error, got {other:?}"),
        }
    }

    fn assert_failed_timeout(outcome: PipelineOutcome, min_elapsed_ms: u64) {
        let PipelineOutcome::Failed(e) = outcome else {
            panic!("expected Failed, got {outcome:?}");
        };
        match tx_err(&e) {
            TransactionError::Timeout {
                datasource,
                elapsed_ms,
            } => {
                assert_eq!(datasource, DS);
                assert!(
                    *elapsed_ms >= min_elapsed_ms,
                    "Timeout.elapsed_ms must measure from block entry: {elapsed_ms}"
                );
            }
            other => panic!("expected Timeout, got {other:?}"),
        }
    }

    fn assert_failed_tx(outcome: PipelineOutcome, expected: &TransactionError) {
        let PipelineOutcome::Failed(e) = outcome else {
            panic!("expected Failed, got {outcome:?}");
        };
        assert_eq!(tx_err(&e), expected, "typed primary mismatch");
    }

    fn assert_failed_processor(outcome: PipelineOutcome) {
        let PipelineOutcome::Failed(CamelError::ProcessorError(message)) = outcome else {
            panic!("expected Failed(ProcessorError), got {outcome:?}");
        };
        assert_eq!(message, "body failed", "the body error must stay primary");
    }

    fn assert_completed_text(outcome: PipelineOutcome, text: &str) {
        let PipelineOutcome::Completed(ex) = outcome else {
            panic!("expected Completed, got {outcome:?}");
        };
        assert_eq!(ex.input.body.as_text(), Some(text));
    }

    fn assert_stopped_text(outcome: PipelineOutcome, text: &str) {
        let PipelineOutcome::Stopped(ex) = outcome else {
            panic!("expected Stopped, got {outcome:?}");
        };
        assert_eq!(ex.input.body.as_text(), Some(text));
    }

    fn committed() -> Result<TransactionOutcome, TransactionError> {
        Ok(TransactionOutcome::Committed)
    }

    fn rolled_back() -> Result<TransactionOutcome, TransactionError> {
        Ok(TransactionOutcome::RolledBack)
    }

    fn uncertain() -> Result<TransactionOutcome, TransactionError> {
        Err(TransactionError::RollbackUncertain {
            datasource: DS.to_owned(),
        })
    }

    /// Assert target for the R7 loop in `transacted_r7_commits_active_body`.
    enum Assert {
        Completed,
        Stopped,
    }

    // ── THE fixture: fresh-rollback precedence table ────────────────────

    /// The 604d `(primary, r)` outcome table over fresh/lossy sessions:
    /// Deadline, Err, and Ok bodies crossed with acknowledged and
    /// unacknowledged `rollback()`, INCLUDING the no-primary row (body Ok +
    /// a step-local drop leaving `BeginPending`, reported as the lossy
    /// `Fresh` state with the flag clear + `rollback()` →
    /// `Err(RollbackUncertain)` ⇒ the reported outcome is
    /// `Err(RollbackUncertain)`).
    #[tokio::test]
    async fn transacted_fresh_rollback_precedence() {
        // (Some(p), Ok(_)) — R5: Deadline primary, cleanup acknowledged.
        let session = FakeSession::new(TransactionState::Fresh, committed(), rolled_back());
        let mut seg = segment(
            session_manager(session.clone()),
            BodyScript::Hang,
            Duration::from_millis(30),
        );
        let outcome = seg.run(Exchange::new(Message::new("m"))).await;
        assert_failed_timeout(outcome, 30);
        assert_eq!(
            session.calls(),
            vec!["rollback"],
            "commit is never consulted on a Deadline body"
        );

        // (Some(p), Err(u)) — R5: Deadline primary, cleanup NOT substituted.
        let session = FakeSession::new(TransactionState::Fresh, committed(), uncertain());
        let mut seg = segment(
            session_manager(session.clone()),
            BodyScript::Hang,
            Duration::from_millis(30),
        );
        let outcome = seg.run(Exchange::new(Message::new("m"))).await;
        assert_failed_timeout(outcome, 30);
        assert_eq!(session.calls(), vec!["rollback"]);

        // (Some(p), Ok(_)) — body Err is primary, cleanup acknowledged.
        let session = FakeSession::new(TransactionState::Active, committed(), rolled_back());
        let mut seg = segment(
            session_manager(session.clone()),
            BodyScript::Fail,
            Duration::from_secs(1),
        );
        let outcome = seg.run(Exchange::new(Message::new("m"))).await;
        assert_failed_processor(outcome);
        assert_eq!(
            session.calls(),
            vec!["rollback"],
            "commit is never consulted on an Err body"
        );

        // (Some(p), Err(u)) — body Err primary, cleanup carried in the log.
        let session = FakeSession::new(TransactionState::Active, committed(), uncertain());
        let mut seg = segment(
            session_manager(session.clone()),
            BodyScript::Fail,
            Duration::from_secs(1),
        );
        let outcome = seg.run(Exchange::new(Message::new("m"))).await;
        assert_failed_processor(outcome);
        assert_eq!(session.calls(), vec!["rollback"]);

        // (None, Err(u)) — THE no-primary row: body Ok, flag clear, state()
        // Fresh (lossy BeginPending — the R7 gate correctly does not infer
        // begin-ness from state()), cleanup unacknowledged ⇒ the cleanup
        // result IS the outcome: Err(RollbackUncertain).
        let session = FakeSession::new(TransactionState::Fresh, committed(), uncertain());
        let mut seg = segment(
            session_manager(session.clone()),
            BodyScript::Complete("ok"),
            Duration::from_secs(1),
        );
        let outcome = seg.run(Exchange::new(Message::new("m"))).await;
        assert_failed_tx(
            outcome,
            &TransactionError::RollbackUncertain {
                datasource: DS.to_owned(),
            },
        );
        assert_eq!(
            session.calls(),
            vec!["rollback"],
            "Fresh is not Active: the R7 gate must not reach commit"
        );

        // (None, Ok(_)) — R2 clean: BEGIN never sent, body Ok unchanged.
        let session = FakeSession::new(TransactionState::Fresh, committed(), rolled_back());
        let mut seg = segment(
            session_manager(session.clone()),
            BodyScript::Complete("ok"),
            Duration::from_secs(1),
        );
        let outcome = seg.run(Exchange::new(Message::new("m"))).await;
        assert_completed_text(outcome, "ok");
        assert_eq!(session.calls(), vec!["rollback"]);

        // (None, Ok(_)) — R2 clean Stop propagates per ADR-0025: a Stop must
        // NOT become a rollback report (the outcome stays Stopped; the
        // rollback itself is still the session's R2 no-SQL answer).
        let session = FakeSession::new(TransactionState::Fresh, committed(), rolled_back());
        let mut seg = segment(
            session_manager(session.clone()),
            BodyScript::Stop("halted"),
            Duration::from_secs(1),
        );
        let outcome = seg.run(Exchange::new(Message::new("m"))).await;
        assert_stopped_text(outcome, "halted");
        assert_eq!(session.calls(), vec!["rollback"]);
    }

    // ── Table pins ──────────────────────────────────────────────────────

    /// R7 commit path: body Ok, state Active, flag clear, cutoff passed ⇒
    /// commit called once, rollback never, the body outcome unchanged; a
    /// clean Stopped propagates as Stopped (ADR-0025).
    #[tokio::test]
    async fn transacted_r7_commits_active_body() {
        for (body, text, expect) in [
            (BodyScript::Complete("ok"), "ok", Assert::Completed),
            (BodyScript::Stop("halted"), "halted", Assert::Stopped),
        ] {
            let session = FakeSession::new(TransactionState::Active, committed(), rolled_back());
            let mut seg = segment(
                session_manager(session.clone()),
                body,
                Duration::from_secs(1),
            );
            let outcome = seg.run(Exchange::new(Message::new("m"))).await;
            match expect {
                Assert::Completed => assert_completed_text(outcome, text),
                Assert::Stopped => assert_stopped_text(outcome, text),
            }
            assert_eq!(
                session.calls(),
                vec!["commit"],
                "R7: exactly one COMMIT, no rollback"
            );
        }
    }

    /// R6 cutoff FAIL: the flag is set between the body race resolving Ok and
    /// the finalization handoff ⇒ rollback runs, commit is NEVER consulted,
    /// and the primary is the cancellation error (deadline not elapsed).
    #[tokio::test]
    async fn transacted_r6_cutoff_cancels_before_commit() {
        let session = FakeSession::new(TransactionState::Active, committed(), rolled_back());
        // One token, two handles: the executor-scooped task-local the segment
        // reads, and the clone the body cancels as its last act.
        let token = CancellationToken::new();
        let mut seg = segment(
            session_manager(session.clone()),
            BodyScript::CancelThenComplete("ok", token.clone()),
            Duration::from_secs(1),
        );
        let outcome = CANCEL_TOKEN
            .scope(token, async move {
                seg.run(Exchange::new(Message::new("m"))).await
            })
            .await;
        let PipelineOutcome::Failed(e) = outcome else {
            panic!("expected Failed, got {outcome:?}");
        };
        assert!(
            matches!(e, CamelError::ConsumerStopping),
            "R6 cutoff failure with the deadline unelapsed is the cancellation error, got {e:?}"
        );
        assert_eq!(
            session.calls(),
            vec!["rollback"],
            "R6: COMMIT is never sent after a failed cutoff"
        );
    }

    /// R4 flag-only: `rollback_only` set + body Ok ⇒ the FLAG alone triggers
    /// the rollback row with the `AbortedByHandledError` primary; commit is
    /// never consulted even though the state is Active.
    #[tokio::test]
    async fn transacted_flag_only_rolls_back_ok_body() {
        let session =
            FakeSession::new(TransactionState::Active, committed(), rolled_back()).with_flag();
        let mut seg = segment(
            session_manager(session.clone()),
            BodyScript::Complete("ok"),
            Duration::from_secs(1),
        );
        let outcome = seg.run(Exchange::new(Message::new("m"))).await;
        assert_failed_tx(
            outcome,
            &TransactionError::AbortedByHandledError {
                datasource: DS.to_owned(),
            },
        );
        assert_eq!(
            session.calls(),
            vec!["rollback"],
            "R4: the flag alone forbids COMMIT"
        );
    }

    /// Deadline race tie → Timeout wins: with the deadline already elapsed
    /// and the body immediately ready, the biased select order resolves
    /// Deadline first — never the body's Ok.
    #[tokio::test]
    async fn transacted_deadline_tie_selects_timeout() {
        let session = FakeSession::new(TransactionState::Active, committed(), rolled_back());
        let mut seg = segment(
            session_manager(session.clone()),
            BodyScript::Complete("fast"),
            Duration::ZERO,
        );
        let outcome = seg.run(Exchange::new(Message::new("m"))).await;
        assert_failed_timeout(outcome, 0);
        assert_eq!(
            session.calls(),
            vec!["rollback"],
            "commit is never consulted on a Deadline body"
        );
    }

    /// Body-race Cancelled arm: the scoped token is pre-cancelled and the
    /// deadline is far, so the biased select resolves Cancelled (never the
    /// hanging body, never the deadline). Fresh/clean session ⇒
    /// `(Some(ConsumerStopping), Ok(RolledBack))` — the primary is the
    /// cancellation error and the connection returns clean, NOT a Timeout.
    #[tokio::test]
    async fn transacted_cancelled_body_rolls_back_clean() {
        let session = FakeSession::new(TransactionState::Fresh, committed(), rolled_back());
        let token = CancellationToken::new();
        token.cancel(); // pre-cancelled: the race branch is ready on first poll
        let mut seg = segment(
            session_manager(session.clone()),
            BodyScript::Hang,
            Duration::from_secs(1),
        );
        let outcome = CANCEL_TOKEN
            .scope(token, async move {
                seg.run(Exchange::new(Message::new("m"))).await
            })
            .await;
        let PipelineOutcome::Failed(e) = outcome else {
            panic!("expected Failed, got {outcome:?}");
        };
        assert!(
            matches!(e, CamelError::ConsumerStopping),
            "a cancelled body race is the cancellation error, never Timeout, got {e:?}"
        );
        assert_eq!(
            session.calls(),
            vec!["rollback"],
            "cancellation rolls back through the session's clean R2 path"
        );
    }

    /// Frozen fixture `transacted_stalled_rollback`, segment tier (spec "Block
    /// timeout"): the body deadline fires (primary `Timeout`) and the cleanup
    /// `rollback()` is unacknowledged — the segment reports the Timeout
    /// primary (never substitutes the cleanup error) and carries the cleanup
    /// uncertainty in the `warn!`, per bd rc-ngk4e (log-carried context;
    /// `CamelError` has no `with_cleanup` field).
    #[tokio::test]
    async fn transacted_stalled_rollback_logs_cleanup_context() {
        ensure_global_tracing_default();
        let session = FakeSession::new(TransactionState::Active, committed(), uncertain());
        let mut seg = segment(
            session_manager(session.clone()),
            BodyScript::Hang,
            Duration::from_millis(30),
        );
        let (outcome, captured) =
            log_capture::capture_logs_async(seg.run(Exchange::new(Message::new("m")))).await;

        assert_failed_timeout(outcome, 30);
        assert_eq!(
            session.calls(),
            vec!["rollback"],
            "commit is never consulted on a Deadline body"
        );
        assert!(
            captured.contains("transacted block cleanup was not acknowledged"),
            "the (Some(p), Err(u)) row carries the cleanup uncertainty in the log: {captured}"
        );
        assert!(
            captured.contains("primary error stands"),
            "the log states the primary error is not substituted: {captured}"
        );
    }

    /// BLOCK_SESSION binding: the body task sees the block's datasource, its
    /// own session allocation, and a per-execution scope id that increments
    /// across runs of the same segment (so it can be baked into joined steps).
    #[tokio::test]
    async fn transacted_block_session_scopes_the_body() {
        let session = FakeSession::new(TransactionState::Fresh, committed(), rolled_back());
        let seen = Arc::new(Mutex::new(None));
        let mut seg = segment(
            session_manager(session.clone()),
            BodyScript::Observe {
                expected_session: session,
                seen: Arc::clone(&seen),
            },
            Duration::from_secs(1),
        );
        let outcome = seg.run(Exchange::new(Message::new("m"))).await;
        assert_completed_text(outcome, "observed");
        let (datasource, first_scope) = {
            let guard = seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard
                .clone()
                .expect("the body recorded the installed binding")
        };
        assert_eq!(datasource, DS);

        let outcome = seg.run(Exchange::new(Message::new("m"))).await;
        assert_completed_text(outcome, "observed");
        let (_, second_scope) = {
            let guard = seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.clone().expect("the second run recorded its binding")
        };
        assert_eq!(
            second_scope,
            first_scope + 1,
            "every block execution allocates a fresh scope id"
        );
    }

    /// Admission: open-hook errors map onto the exchange failure unchanged
    /// (`PoolExhausted` and `Timeout` primaries are the camel-sql race's to
    /// name) and the partially opened session is simply discarded. A hook
    /// that ignores the block deadline is bounded at block_abs with tie →
    /// Timeout measured from entry.
    #[tokio::test]
    async fn transacted_open_error_maps_to_exchange_failure() {
        // PoolExhausted straight from the admission race (fixture (c) of
        // rc-9awnc: the block deadline has NOT elapsed when the open
        // resolves — a genuine acquire expiry, passed through unchanged).
        let mut seg = segment(
            FakeManager::script(OpenScript::Fail(TransactionError::PoolExhausted {
                datasource: DS.to_owned(),
                max_connections: 1,
                waited_ms: 5,
            })),
            BodyScript::Complete("ok"),
            Duration::from_secs(1),
        );
        let outcome = seg.run(Exchange::new(Message::new("m"))).await;
        assert_failed_tx(
            outcome,
            &TransactionError::PoolExhausted {
                datasource: DS.to_owned(),
                max_connections: 1,
                waited_ms: 5,
            },
        );

        // Timeout straight from the admission race — passed through, not
        // re-mapped by the segment (a non-deadline open error family member
        // is never re-adjudicated; `Timeout` from the manager is already the
        // race's own verdict).
        let mut seg = segment(
            FakeManager::script(OpenScript::Fail(TransactionError::Timeout {
                datasource: DS.to_owned(),
                elapsed_ms: 7,
            })),
            BodyScript::Complete("ok"),
            Duration::from_secs(1),
        );
        let outcome = seg.run(Exchange::new(Message::new("m"))).await;
        assert_failed_tx(
            outcome,
            &TransactionError::Timeout {
                datasource: DS.to_owned(),
                elapsed_ms: 7,
            },
        );

        // A hook that never completes is bounded by the block deadline
        // (biased tie → Timeout, elapsed measured from block entry).
        let mut seg = segment(
            FakeManager::script(OpenScript::Pending),
            BodyScript::Complete("ok"),
            Duration::from_millis(20),
        );
        let outcome = seg.run(Exchange::new(Message::new("m"))).await;
        assert_failed_timeout(outcome, 20);
    }

    /// Frozen "Block timeout" instant comparison at the segment's open race
    /// (spec: "Deadlines are compared as absolute instants — not by
    /// outer-timeout polling order"): an open that resolves AT the
    /// block-deadline poll is authoritative. rc-9awnc fixture (b): the
    /// acquire arm elapsed STRICTLY first (`acquire_abs = open_start +
    /// acquire_deadline()` < `block_abs`), so the ready `PoolExhausted`
    /// passes through even though the block timer fires at the same poll.
    /// The timer bound applies only to a still-pending open (tie →
    /// Timeout, pinned above).
    #[tokio::test]
    async fn transacted_open_race_ready_open_decides_over_elapsed_deadline() {
        // The open stays pending on its first poll, then fails with
        // PoolExhausted: at the 20 ms poll the open future and the block
        // timer are ready at once, and the absolute comparison passes the
        // error through (acquire 5 ms < block 20 ms).
        let exhausted = TransactionError::PoolExhausted {
            datasource: DS.to_owned(),
            max_connections: 1,
            waited_ms: 5,
        };
        let mut seg = segment(
            FakeManager::script(OpenScript::FailAfterFirstPoll(exhausted.clone()))
                .with_acquire_deadline(Duration::from_millis(5)),
            BodyScript::Complete("ok"),
            Duration::from_millis(20),
        );
        let outcome = seg.run(Exchange::new(Message::new("m"))).await;
        assert_failed_tx(outcome, &exhausted);
    }

    /// rc-9awnc: both-elapsed adjudication at the segment tier. The port's
    /// `open_session()` carries no block instant (the production manager arms
    /// a stand-in internal one), so a ready `PoolExhausted` that resolves
    /// with the REAL block deadline also spent is re-adjudicated by absolute
    /// instants through `acquire_deadline()`: `block_abs <= acquire_abs`
    /// (tie included) → `Timeout` from entry; acquire strictly first → the
    /// `PoolExhausted` passes through.
    #[tokio::test(start_paused = true)]
    async fn transacted_open_pool_exhausted_adjudicated_by_absolute_instants() {
        let exhausted = TransactionError::PoolExhausted {
            datasource: DS.to_owned(),
            max_connections: 1,
            waited_ms: 5,
        };

        // Fixture (a): block_abs < acquire_abs (production manager: the
        // stand-in internal block instant never elapses first), both elapsed
        // at the first poll — the ready `PoolExhausted` does NOT win; the
        // earlier block deadline reports `Timeout{elapsed from entry}`.
        let mut seg = segment(
            FakeManager::script(OpenScript::FailAfterFirstPoll(exhausted.clone())),
            BodyScript::Complete("ok"),
            Duration::from_millis(20),
        );
        let outcome = seg.run(Exchange::new(Message::new("m"))).await;
        assert_failed_timeout(outcome, 20);

        // Tie (block_abs == acquire_abs, start-paused tokio time): the
        // frozen tie resolves to `Timeout` as well.
        let mut seg = segment(
            FakeManager::script(OpenScript::FailAfterFirstPoll(exhausted.clone()))
                .with_acquire_deadline(Duration::from_millis(20)),
            BodyScript::Complete("ok"),
            Duration::from_millis(20),
        );
        let outcome = seg.run(Exchange::new(Message::new("m"))).await;
        assert_failed_timeout(outcome, 20);

        // Fixture (c): `PoolExhausted` resolves while the block deadline has
        // NOT elapsed — a genuine acquire expiry, passed through unchanged.
        let mut seg = segment(
            FakeManager::script(OpenScript::Fail(exhausted.clone())),
            BodyScript::Complete("ok"),
            Duration::from_millis(20),
        );
        let outcome = seg.run(Exchange::new(Message::new("m"))).await;
        assert_failed_tx(outcome, &exhausted);
    }

    /// Frozen fixture `transacted_cancel_after_commit_ack` (spec
    /// "Cancellation cutoff" / "cancellation after the cutoff with a
    /// deterministic ACK"), at the segment tier where CANCEL_TOKEN lives:
    /// cancellation arrives MID-COMMIT — the gated fake session resolves
    /// COMMIT only once the (scoped) token fires — and R7 never consults it
    /// again: the outcome is `Completed` and `rollback` is NEVER called.
    ///
    /// Determinism: the canceller task runs at the segment's first yield,
    /// which is the commit gate itself (open and body resolve on their first
    /// poll), so the token is still clean at the R6 cutoff and fires strictly
    /// mid-commit.
    #[tokio::test]
    async fn transacted_cancel_after_commit_ack() {
        // One token, three handles: the task-local the segment reads, the
        // commit gate, and the canceller's.
        let token = CancellationToken::new();
        let session = FakeSession::gated(Ok(TransactionOutcome::Committed), token.clone());
        tokio::spawn({
            let token = token.clone();
            async move {
                // Runs at the segment's first yield — the commit gate — so
                // cancellation lands strictly mid-commit, never at the R6
                // cutoff.
                token.cancel();
            }
        });
        let mut seg = segment(
            session_manager(session.clone()),
            BodyScript::Complete("ok"),
            Duration::from_secs(1),
        );
        let outcome = CANCEL_TOKEN
            .scope(token, async move {
                seg.run(Exchange::new(Message::new("m"))).await
            })
            .await;
        assert_completed_text(outcome, "ok");
        assert_eq!(
            session.calls(),
            vec!["commit"],
            "post-handoff cancellation is never consulted: no rollback call"
        );
    }

    /// Frozen fixture `transacted_cancel_after_commit_lost` (spec
    /// "Cancellation cutoff" / "cancellation after the cutoff with a lost
    /// ACK"), at the segment tier: cancellation arrives mid-commit and the
    /// COMMIT acknowledgment is lost (`CommitUncertain`) — the block fails
    /// with `CommitUncertain`, and no ROLLBACK is issued or claimed.
    #[tokio::test]
    async fn transacted_cancel_after_commit_lost() {
        let token = CancellationToken::new();
        let session = FakeSession::gated(
            Err(TransactionError::CommitUncertain {
                datasource: DS.to_owned(),
            }),
            token.clone(),
        );
        tokio::spawn({
            let token = token.clone();
            async move {
                token.cancel();
            }
        });
        let mut seg = segment(
            session_manager(session.clone()),
            BodyScript::Complete("ok"),
            Duration::from_secs(1),
        );
        let outcome = CANCEL_TOKEN
            .scope(token, async move {
                seg.run(Exchange::new(Message::new("m"))).await
            })
            .await;
        assert_failed_tx(
            outcome,
            &TransactionError::CommitUncertain {
                datasource: DS.to_owned(),
            },
        );
        assert_eq!(
            session.calls(),
            vec!["commit"],
            "a lost ACK after the cutoff is never reported as a rollback"
        );
    }
}
