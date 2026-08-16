//! ## Stop semantics (ADR-0025)
//!
//! This segment implements `OutcomePipeline` and propagates `PipelineOutcome::Stopped(ex)`
//! with the exchange state intact. See ADR-0025 §3.

use futures::FutureExt;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::task::JoinSet;

use camel_api::{Exchange, Value};

use crate::multicast::{CAMEL_MULTICAST_COMPLETE, CAMEL_MULTICAST_INDEX};

// ── MulticastSegment (ADR-0025 OutcomePipeline) ──────────────────────────

/// Outcome-aware Multicast segment. Holds N child OutcomeSegments and a
/// strategy (sequential or parallel). Parallel cancellation logic mirrors
/// T13 SplitSegment — lower-the-value CAS records lowest-branch-index that
/// Stopped (spec §5.2.2 line 497); pre-start gate skips not-yet-started
/// branches; in-flight branches run to completion (spec §5.6 line 544:
/// no abrupt abort); JoinSet ensures cancel-safe drop on outer future drop.
///
/// Each branch receives its OWN clone of the exchange (Multicast semantics —
/// branches do NOT share body mutations).
///
/// Aggregation SKIPPED when any branch Stopped (spec §5.2.2).
#[derive(Clone)]
pub struct MulticastSegment {
    pub branches: Vec<camel_api::OutcomeSegment>,
    pub parallel: bool,
    /// Maximum number of concurrent branches in parallel mode (None = unlimited).
    pub parallel_limit: Option<usize>,
    /// Whether to stop processing on the first exception.
    ///
    /// When `true`, a `Failed` outcome from any branch halts processing
    /// immediately (sequential) or propagates the first `Failed` branch
    /// (lowest branch index, parallel). When `false`, failures are collected and
    /// processing continues: a zero-success run propagates the representative
    /// error (last-wins), while a partial-success run aggregates the successful
    /// branches' outputs only and discards the failed outcomes (logged at warn).
    ///
    /// `Stopped` outcomes always propagate per ADR-0025 §7 regardless of this flag.
    pub stop_on_exception: bool,
    /// Per-branch timeout in parallel mode (None = no timeout).
    pub timeout: Option<Duration>,
    pub aggregator: Arc<dyn Fn(Vec<Exchange>) -> Exchange + Send + Sync>,
}

impl camel_api::OutcomePipeline for MulticastSegment {
    fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = camel_api::PipelineOutcome> + Send + 'a>> {
        Box::pin(async move {
            if self.parallel {
                parallel_multicast(self, exchange).await
            } else {
                sequential_multicast(self, exchange).await
            }
        })
    }
}

// ── Sequential multicast ─────────────────────────────────────────────────

async fn sequential_multicast(
    seg: &mut MulticastSegment,
    exchange: Exchange,
) -> camel_api::PipelineOutcome {
    let mut outputs = Vec::new();
    let mut last_error: Option<camel_api::CamelError> = None;
    let total = seg.branches.len();
    for (i, branch) in seg.branches.iter_mut().enumerate() {
        // Each branch gets a clone (Multicast semantics — no shared mutations).
        let mut ex = exchange.clone();
        ex.set_property(CAMEL_MULTICAST_INDEX, Value::from(i as i64));
        ex.set_property(CAMEL_MULTICAST_COMPLETE, Value::Bool(i == total - 1));
        match branch.run(ex).await {
            camel_api::PipelineOutcome::Completed(ex) => outputs.push(ex),
            camel_api::PipelineOutcome::Stopped(ex) => {
                return camel_api::PipelineOutcome::Stopped(ex);
            }
            camel_api::PipelineOutcome::Failed(err) => {
                if seg.stop_on_exception {
                    return camel_api::PipelineOutcome::Failed(err);
                }
                // stop_on_exception=false: collect error, continue.
                last_error = Some(err);
            }
        }
    }
    if let Some(err) = last_error {
        if outputs.is_empty() {
            return camel_api::PipelineOutcome::Failed(err);
        }
        // log-policy: handler-owned
        tracing::warn!(
            failed_branches = total - outputs.len(),
            branch_count = total,
            "multicast partial success: discarding failed branch outcomes"
        );
    }
    camel_api::PipelineOutcome::Completed((seg.aggregator)(outputs))
}

// ── Parallel multicast ──────────────────────────────────────────────────

/// Parallel multicast with lowest-index-wins CAS semantics.
///
/// See spec §5.2.2 line 497 for the CAS guarantee and §5.6 line 544 for the
/// "no abrupt abort" in-flight task policy (pre-start gate + run-to-completion).
async fn parallel_multicast(
    seg: &mut MulticastSegment,
    exchange: Exchange,
) -> camel_api::PipelineOutcome {
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    let stopped_seen = Arc::new(AtomicBool::new(false));
    let stopped_idx = Arc::new(AtomicUsize::new(usize::MAX));
    let semaphore = seg
        .parallel_limit
        .filter(|&limit| limit > 0)
        .map(|limit| Arc::new(Semaphore::new(limit)));
    let timeout = seg.timeout;
    let stop_on_exception = seg.stop_on_exception;
    let total = seg.branches.len();

    let mut set: JoinSet<(usize, Option<camel_api::PipelineOutcome>)> = JoinSet::new();

    for (idx, mut branch) in seg.branches.clone().into_iter().enumerate() {
        let stopped_seen = Arc::clone(&stopped_seen);
        let stopped_idx = Arc::clone(&stopped_idx);
        let sem = semaphore.clone();
        // Each branch gets its OWN clone of the exchange (Multicast semantics).
        let mut ex = exchange.clone();
        ex.set_property(CAMEL_MULTICAST_INDEX, Value::from(idx as i64));
        ex.set_property(CAMEL_MULTICAST_COMPLETE, Value::Bool(idx == total - 1));
        set.spawn(async move {
            // Pre-start gate: a lower-index branch already stopped.
            if stopped_seen.load(Ordering::SeqCst) {
                return (idx, None);
            }
            // Acquire semaphore permit if parallel_limit is set.
            let _permit: Option<tokio::sync::OwnedSemaphorePermit> = match &sem {
                Some(s) => match Arc::clone(s).acquire_owned().await {
                    Ok(p) => Some(p),
                    Err(_) => {
                        return (
                            idx,
                            Some(camel_api::PipelineOutcome::Failed(
                                camel_api::CamelError::ProcessorError("semaphore closed".into()),
                            )),
                        );
                    }
                },
                None => None,
            };
            // Re-check pre-start gate after permit acquisition.
            if stopped_seen.load(Ordering::SeqCst) {
                return (idx, None);
            }

            // Run body (Stop CAS stays inside the caught future so a Stopped
            // branch is still observed before any unwind mapping).
            let outcome = async {
                let outcome = branch.run(ex).await;
                if let camel_api::PipelineOutcome::Stopped(_) = &outcome {
                    // Lower-the-value CAS.
                    loop {
                        let cur = stopped_idx.load(Ordering::SeqCst);
                        if idx >= cur {
                            break;
                        }
                        match stopped_idx.compare_exchange_weak(
                            cur,
                            idx,
                            Ordering::SeqCst,
                            Ordering::SeqCst,
                        ) {
                            Ok(_) => break,
                            Err(actual) => {
                                if actual <= idx {
                                    break;
                                }
                            }
                        }
                    }
                    stopped_seen.store(true, Ordering::SeqCst);
                }
                outcome
            };

            // Catch unwinds inside the task so the branch index survives (a
            // dropped JoinError loses it). Segment-outcome-composition spec's
            // panicked-branch classification clause (bd rc-f88o): a panicking
            // parallel branch is zero-success attempted work under ADR-0058 and
            // maps to a representative Failed(ProcessorError) instead of being
            // silently dropped from outcome/accounting/last_error. mem::forget
            // avoids a second panic (payload Drop) outside catch_unwind, which
            // would recreate the dropped-JoinError defect; see recipient_list.rs
            // panic arm for the same contract.
            let outcome = AssertUnwindSafe(outcome).catch_unwind();
            let outcome = if let Some(dur) = timeout {
                match tokio::time::timeout(dur, outcome).await {
                    Ok(Ok(o)) => o,
                    Ok(Err(panic_payload)) => {
                        let failure = camel_api::PipelineOutcome::Failed(
                            camel_api::CamelError::ProcessorError(format!(
                                "multicast branch {idx} panicked"
                            )),
                        );
                        std::mem::forget(panic_payload);
                        failure
                    }
                    Err(_elapsed) => {
                        camel_api::PipelineOutcome::Failed(camel_api::CamelError::ProcessorError(
                            format!("multicast branch {idx} timed out after {dur:?}"),
                        ))
                    }
                }
            } else {
                match outcome.await {
                    Ok(outcome) => outcome,
                    Err(panic_payload) => {
                        let failure = camel_api::PipelineOutcome::Failed(
                            camel_api::CamelError::ProcessorError(format!(
                                "multicast branch {idx} panicked"
                            )),
                        );
                        std::mem::forget(panic_payload);
                        failure
                    }
                }
            };

            (idx, Some(outcome))
        });
    }

    // Wait for ALL in-flight branches to finish.
    let mut results: Vec<(usize, camel_api::PipelineOutcome)> = Vec::new();
    while let Some(res) = set.join_next().await {
        if let Ok((idx, Some(o))) = res {
            results.push((idx, o));
        }
    }

    // Deterministic lowest-branch-index wins for Stop.
    if stopped_seen.load(Ordering::SeqCst) {
        let winning_idx = stopped_idx.load(Ordering::SeqCst);
        if winning_idx == usize::MAX {
            tracing::warn!(
                target: "camel.phase4.multicast",
                "stopped_seen=true but stopped_idx=usize::MAX — race; falling back to pre-multicast exchange"
            );
            return camel_api::PipelineOutcome::Stopped(exchange);
        }
        let stopped_ex = results
            .iter()
            .find(|(idx, _)| *idx == winning_idx)
            .and_then(|(_, o)| match o {
                camel_api::PipelineOutcome::Stopped(ex) => Some(ex.clone()),
                _ => None,
            });
        if let Some(ex) = stopped_ex {
            return camel_api::PipelineOutcome::Stopped(ex);
        }
        tracing::warn!(
            target: "camel.phase4.multicast",
            winning_idx = winning_idx,
            "winning_idx not found — falling back to pre-multicast exchange"
        );
        return camel_api::PipelineOutcome::Stopped(exchange);
    }

    // Check for Failed outcomes.
    // stop_on_exception=true: propagate first Failed (lowest branch index).
    // stop_on_exception=false: collect last error (last-wins) and defer to the
    // shared-tail guard after aggregation.
    results.sort_by_key(|(idx, _)| *idx);
    let mut last_error: Option<camel_api::CamelError> = None;
    if stop_on_exception {
        let mut first_failed: Option<(usize, camel_api::CamelError)> = None;
        for (idx, o) in &results {
            if let camel_api::PipelineOutcome::Failed(err) = o
                && first_failed
                    .as_ref()
                    .map(|(i, _)| *i > *idx)
                    .unwrap_or(true)
            {
                first_failed = Some((*idx, err.clone()));
            }
        }
        if let Some((_, err)) = first_failed {
            return camel_api::PipelineOutcome::Failed(err);
        }
    } else {
        // Collect last error (last-wins, matching legacy LastWins semantics).
        for (_, o) in &results {
            if let camel_api::PipelineOutcome::Failed(err) = o {
                last_error = Some(err.clone());
            }
        }
    }

    // Count actual Failed slots (not pre-start-gate-skipped None slots) before
    // `results` is consumed below.
    let failed_branches = results
        .iter()
        .filter(|(_, o)| matches!(o, camel_api::PipelineOutcome::Failed(_)))
        .count();

    // Single aggregation point — Completed outcomes only.
    let completed: Vec<Exchange> = results
        .into_iter()
        .filter_map(|(_, o)| match o {
            camel_api::PipelineOutcome::Completed(ex) => Some(ex),
            _ => None,
        })
        .collect();

    if let Some(err) = last_error {
        if completed.is_empty() {
            return camel_api::PipelineOutcome::Failed(err);
        }
        // log-policy: handler-owned
        tracing::warn!(
            failed_branches,
            branch_count = total,
            "multicast partial success: discarding failed branch outcomes"
        );
    }
    camel_api::PipelineOutcome::Completed((seg.aggregator)(completed))
}

#[cfg(test)]
#[path = "multicast_segment_tests.rs"]
mod tests;
