//! ## Stop semantics (ADR-0025)
//!
//! This segment implements `OutcomePipeline` and propagates `PipelineOutcome::Stopped(ex)`
//! with the exchange state intact. See ADR-0025 §3.

use std::future::Future;
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

            // Run body with optional per-branch timeout.
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

            let outcome = if let Some(dur) = timeout {
                match tokio::time::timeout(dur, outcome).await {
                    Ok(o) => o,
                    Err(_elapsed) => {
                        camel_api::PipelineOutcome::Failed(camel_api::CamelError::ProcessorError(
                            format!("multicast branch {idx} timed out after {dur:?}"),
                        ))
                    }
                }
            } else {
                outcome.await
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
mod tests {
    use super::*;
    use camel_api::{Message, OutcomePipeline, OutcomeSegment, PipelineOutcome};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Body that always returns Completed and increments the counter.
    fn counting_passing_body(counter: Arc<AtomicUsize>) -> OutcomeSegment {
        counting_body(counter, usize::MAX) // never fails
    }

    /// Body that fails on the `fail_at`-th invocation (0-indexed: fail_at=0 fails first call).
    fn counting_body(counter: Arc<AtomicUsize>, fail_at: usize) -> OutcomeSegment {
        #[derive(Clone)]
        struct CountBody {
            counter: Arc<AtomicUsize>,
            fail_at: usize,
        }
        impl camel_api::OutcomePipeline for CountBody {
            fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
                Box::new(self.clone())
            }
            fn run<'a>(
                &'a mut self,
                exchange: Exchange,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PipelineOutcome> + Send + 'a>>
            {
                let count = self.counter.fetch_add(1, Ordering::SeqCst);
                let fail_at = self.fail_at;
                Box::pin(async move {
                    if count == fail_at {
                        PipelineOutcome::Failed(camel_api::CamelError::ProcessorError(format!(
                            "fail at {count}"
                        )))
                    } else {
                        PipelineOutcome::Completed(exchange)
                    }
                })
            }
        }
        OutcomeSegment::new(Box::new(CountBody { counter, fail_at }))
    }

    // ── Test A: sequential stop_on_exception=true ────────────────────

    #[tokio::test]
    async fn multicast_sequential_stop_on_exception_true() {
        let invocations = Arc::new(AtomicUsize::new(0));
        let mut seg = MulticastSegment {
            branches: vec![
                counting_passing_body(Arc::clone(&invocations)),
                counting_body(Arc::clone(&invocations), 1), // fail on 2nd call (idx 1)
                counting_passing_body(Arc::clone(&invocations)),
            ],
            parallel: false,
            parallel_limit: None,
            stop_on_exception: true,
            timeout: None,
            aggregator: Arc::new(|exchanges: Vec<Exchange>| {
                exchanges.into_iter().last().unwrap_or_default()
            }),
        };

        let ex = Exchange::new(Message::new("test"));
        let result = OutcomePipeline::run(&mut seg, ex).await;

        assert!(
            matches!(result, PipelineOutcome::Failed(_)),
            "stop_on_exception=true should propagate failure"
        );
        // Only 2 branches executed (0 passed, 1 failed); 2 never runs.
        assert_eq!(invocations.load(Ordering::SeqCst), 2);
    }

    // ── Test B: sequential partial-success aggregation ───────────────

    #[tokio::test]
    async fn multicast_sequential_partial_success_aggregates_successes() {
        let invocations = Arc::new(AtomicUsize::new(0));
        let mut seg = MulticastSegment {
            branches: vec![
                counting_passing_body(Arc::clone(&invocations)),
                counting_body(Arc::clone(&invocations), 1), // fail on 2nd call
                counting_passing_body(Arc::clone(&invocations)),
            ],
            parallel: false,
            parallel_limit: None,
            stop_on_exception: false,
            timeout: None,
            aggregator: Arc::new(|exchanges: Vec<Exchange>| {
                Exchange::new(Message::new(format!("n={}", exchanges.len())))
            }),
        };

        let ex = Exchange::new(Message::new("test"));
        let result = OutcomePipeline::run(&mut seg, ex).await;

        // With stop_on_exception=false, partial success aggregates the successful
        // branches only; the failed branch's output is discarded.
        match result {
            PipelineOutcome::Completed(ex) => {
                let body = ex.body_as::<String>().unwrap_or_default();
                assert_eq!(
                    body, "n=2",
                    "should aggregate the 2 successful branches only"
                );
            }
            other => panic!("expected Completed with body n=2, got {other:?}"),
        }
        assert_eq!(invocations.load(Ordering::SeqCst), 3);
    }

    // ── Test C: parallel_limit enforcement ───────────────────────────

    #[tokio::test(flavor = "multi_thread")]
    async fn multicast_parallel_limit_enforcement() {
        let concurrent = Arc::new(AtomicUsize::new(0));
        let max_concurrent = Arc::new(AtomicUsize::new(0));

        #[derive(Clone)]
        struct LimitedBody {
            concurrent: Arc<AtomicUsize>,
            max_concurrent: Arc<AtomicUsize>,
        }
        impl camel_api::OutcomePipeline for LimitedBody {
            fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
                Box::new(self.clone())
            }
            fn run<'a>(
                &'a mut self,
                exchange: Exchange,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PipelineOutcome> + Send + 'a>>
            {
                let c = Arc::clone(&self.concurrent);
                let mc = Arc::clone(&self.max_concurrent);
                Box::pin(async move {
                    let current = c.fetch_add(1, Ordering::SeqCst) + 1;
                    mc.fetch_max(current, Ordering::SeqCst);
                    tokio::task::yield_now().await;
                    c.fetch_sub(1, Ordering::SeqCst);
                    PipelineOutcome::Completed(exchange)
                })
            }
        }

        let target: Arc<dyn Fn(Vec<Exchange>) -> Exchange + Send + Sync> =
            Arc::new(|exchanges: Vec<Exchange>| exchanges.into_iter().last().unwrap_or_default());

        let mut seg = MulticastSegment {
            branches: (0..6)
                .map(|_| {
                    OutcomeSegment::new(Box::new(LimitedBody {
                        concurrent: Arc::clone(&concurrent),
                        max_concurrent: Arc::clone(&max_concurrent),
                    }))
                })
                .collect(),
            parallel: true,
            parallel_limit: Some(2),
            stop_on_exception: true,
            timeout: None,
            aggregator: target,
        };

        let ex = Exchange::new(Message::new("test"));
        let result = OutcomePipeline::run(&mut seg, ex).await;
        assert!(
            matches!(result, PipelineOutcome::Completed(_)),
            "Expected Completed, got {result:?}"
        );

        assert!(
            max_concurrent.load(Ordering::SeqCst) <= 2,
            "parallel_limit=2 but observed max concurrency {}",
            max_concurrent.load(Ordering::SeqCst)
        );
    }

    // ── Test D: timeout exceeded ─────────────────────────────────────

    #[tokio::test(flavor = "multi_thread")]
    async fn multicast_timeout_exceeded() {
        // Branch that takes 200ms; timeout set to 50ms.
        #[derive(Clone)]
        struct SlowBody;
        impl camel_api::OutcomePipeline for SlowBody {
            fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
                Box::new(self.clone())
            }
            fn run<'a>(
                &'a mut self,
                exchange: Exchange,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PipelineOutcome> + Send + 'a>>
            {
                Box::pin(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    PipelineOutcome::Completed(exchange)
                })
            }
        }

        let target: Arc<dyn Fn(Vec<Exchange>) -> Exchange + Send + Sync> =
            Arc::new(|exchanges: Vec<Exchange>| exchanges.into_iter().last().unwrap_or_default());

        let mut seg = MulticastSegment {
            branches: vec![
                OutcomeSegment::new(Box::new(SlowBody)),
                counting_passing_body(Arc::new(AtomicUsize::new(0))),
            ],
            parallel: true,
            parallel_limit: None,
            stop_on_exception: true,
            timeout: Some(std::time::Duration::from_millis(50)),
            aggregator: target,
        };

        let ex = Exchange::new(Message::new("test"));
        let result = OutcomePipeline::run(&mut seg, ex).await;

        // With stop_on_exception=true and a timeout, first Failed propagates.
        assert!(
            matches!(result, PipelineOutcome::Failed(_)),
            "Expected Failed due to timeout, got {result:?}"
        );
    }

    // ── Test E: stop_on_exception=false propagates last error (parallel) ──

    #[tokio::test(flavor = "multi_thread")]
    async fn multicast_parallel_stop_on_exception_false_propagates_last_error() {
        let target: Arc<dyn Fn(Vec<Exchange>) -> Exchange + Send + Sync> =
            Arc::new(|exchanges: Vec<Exchange>| exchanges.into_iter().last().unwrap_or_default());

        let mut seg = MulticastSegment {
            branches: vec![
                always_failed_body("err1"), // branch 0 fails with err1
                always_failed_body("err2"), // branch 1 fails with err2
            ],
            parallel: true,
            parallel_limit: None,
            stop_on_exception: false,
            timeout: None,
            aggregator: target,
        };

        let ex = Exchange::new(Message::new("test"));
        let result = OutcomePipeline::run(&mut seg, ex).await;

        // stop_on_exception=false, zero-success → last error (highest idx) propagated.
        match result {
            PipelineOutcome::Failed(err) => {
                let msg = format!("{err}");
                assert!(
                    msg.contains("err2"),
                    "Expected last error 'err2' (from highest-index branch), got: {msg}"
                );
            }
            other => panic!("Expected Failed(err2) with last-wins semantics, got {other:?}"),
        }
    }

    // ── Test F: timeout + stop_on_exception=false propagates timeout error ──

    #[tokio::test(flavor = "multi_thread")]
    async fn multicast_parallel_timeout_stop_on_exception_false_propagates_timeout_error() {
        #[derive(Clone)]
        struct SlowBody;
        impl camel_api::OutcomePipeline for SlowBody {
            fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
                Box::new(SlowBody)
            }
            fn run<'a>(
                &'a mut self,
                exchange: Exchange,
            ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
                Box::pin(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    PipelineOutcome::Completed(exchange)
                })
            }
        }
        #[derive(Clone)]
        struct FastFailBody;
        impl camel_api::OutcomePipeline for FastFailBody {
            fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
                Box::new(FastFailBody)
            }
            fn run<'a>(
                &'a mut self,
                _exchange: Exchange,
            ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
                Box::pin(async move {
                    PipelineOutcome::Failed(camel_api::CamelError::ProcessorError(
                        "fast-fail".into(),
                    ))
                })
            }
        }

        let target: Arc<dyn Fn(Vec<Exchange>) -> Exchange + Send + Sync> =
            Arc::new(|exchanges: Vec<Exchange>| exchanges.into_iter().last().unwrap_or_default());

        let mut seg = MulticastSegment {
            branches: vec![
                OutcomeSegment::new(Box::new(FastFailBody)), // branch 0: fails fast
                OutcomeSegment::new(Box::new(SlowBody)),     // branch 1: 200ms (times out)
            ],
            parallel: true,
            parallel_limit: None,
            stop_on_exception: false,
            timeout: Some(std::time::Duration::from_millis(50)),
            aggregator: target,
        };

        let ex = Exchange::new(Message::new("test"));
        let result = OutcomePipeline::run(&mut seg, ex).await;

        // With stop_on_exception=false and a timeout, the timeout error from
        // branch 1 (highest-index failure) is the last-wins error and propagates.
        match result {
            PipelineOutcome::Failed(err) => {
                let msg = format!("{err}");
                assert!(
                    msg.contains("timed out"),
                    "Expected timeout error from highest-index branch, got: {msg}"
                );
            }
            other => {
                panic!("Expected Failed due to timeout with stop_on_exception=false, got {other:?}")
            }
        }
    }

    // ── Test G: parallel partial-success aggregation ───────────────────

    #[tokio::test(flavor = "multi_thread")]
    async fn multicast_parallel_partial_success_aggregates_successes() {
        // Delta-spec scenario: branches 0 and 2 Completed, branch 1 Failed.
        let mut seg = MulticastSegment {
            branches: vec![
                tagged_completed_body("b0", std::time::Duration::from_millis(10)),
                always_failed_body("err-mid"),
                tagged_completed_body("b2", std::time::Duration::ZERO),
            ],
            parallel: true,
            parallel_limit: None,
            stop_on_exception: false,
            timeout: None,
            aggregator: Arc::new(|exchanges: Vec<Exchange>| {
                let bodies: Vec<String> = exchanges
                    .iter()
                    .map(|ex| ex.body_as::<String>().unwrap_or_default())
                    .collect();
                Exchange::new(Message::new(bodies.join("|")))
            }),
        };

        let ex = Exchange::new(Message::new("inbound"));
        let result = OutcomePipeline::run(&mut seg, ex).await;

        // With stop_on_exception=false, partial success aggregates the successful
        // branches only, in branch-index order (pinned by the body tags): the
        // failed branch's output is discarded. Branch 2 completes first (no
        // delay), so without the index-order sort the aggregator would see
        // "b2|b0" — the "b0|b2" assertion pins results.sort_by_key.
        match result {
            PipelineOutcome::Completed(ex) => {
                let body = ex.body_as::<String>().unwrap_or_default();
                assert_eq!(
                    body, "b0|b2",
                    "should aggregate only branches 0 and 2, in branch-index order"
                );
            }
            other => panic!("expected Completed with body b0|b2, got {other:?}"),
        }
    }

    // ── ADR-0058 regression: zero-success + Stopped-wins (multicast already complies) ─

    fn always_failed_body(msg: &str) -> OutcomeSegment {
        let msg = String::from(msg);
        #[derive(Clone)]
        struct AlwaysFailed(String);
        impl camel_api::OutcomePipeline for AlwaysFailed {
            fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
                Box::new(self.clone())
            }
            fn run<'a>(
                &'a mut self,
                _exchange: Exchange,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PipelineOutcome> + Send + 'a>>
            {
                let msg = self.0.clone();
                Box::pin(async move {
                    PipelineOutcome::Failed(camel_api::CamelError::ProcessorError(msg))
                })
            }
        }
        OutcomeSegment::new(Box::new(AlwaysFailed(msg)))
    }

    fn always_completed_body() -> OutcomeSegment {
        #[derive(Clone)]
        struct AlwaysCompleted;
        impl camel_api::OutcomePipeline for AlwaysCompleted {
            fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
                Box::new(self.clone())
            }
            fn run<'a>(
                &'a mut self,
                exchange: Exchange,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PipelineOutcome> + Send + 'a>>
            {
                Box::pin(async move { PipelineOutcome::Completed(exchange) })
            }
        }
        OutcomeSegment::new(Box::new(AlwaysCompleted))
    }

    fn tagged_completed_body(tag: &str, delay: std::time::Duration) -> OutcomeSegment {
        let tag = String::from(tag);
        #[derive(Clone)]
        struct TaggedCompleted {
            tag: String,
            delay: std::time::Duration,
        }
        impl camel_api::OutcomePipeline for TaggedCompleted {
            fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
                Box::new(self.clone())
            }
            fn run<'a>(
                &'a mut self,
                mut exchange: Exchange,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PipelineOutcome> + Send + 'a>>
            {
                let tag = self.tag.clone();
                let delay = self.delay;
                Box::pin(async move {
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                    exchange.input.body = tag.into();
                    PipelineOutcome::Completed(exchange)
                })
            }
        }
        OutcomeSegment::new(Box::new(TaggedCompleted { tag, delay }))
    }

    fn always_stopped_body() -> OutcomeSegment {
        #[derive(Clone)]
        struct AlwaysStopped;
        impl camel_api::OutcomePipeline for AlwaysStopped {
            fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
                Box::new(self.clone())
            }
            fn run<'a>(
                &'a mut self,
                mut exchange: Exchange,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PipelineOutcome> + Send + 'a>>
            {
                Box::pin(async move {
                    exchange.input.body = "stop-body".into();
                    PipelineOutcome::Stopped(exchange)
                })
            }
        }
        OutcomeSegment::new(Box::new(AlwaysStopped))
    }

    #[tokio::test]
    async fn multicast_all_branches_failed_no_stopped_returns_failed() {
        // ADR-0058: zero-success (all branches Failed, no Stopped) MUST return
        // Failed, not Completed(original). Multicast already complies; this
        // locks the behavior.
        let mut seg = MulticastSegment {
            branches: vec![
                always_failed_body("branch-a-failed"),
                always_failed_body("branch-b-failed"),
            ],
            parallel: false,
            parallel_limit: None,
            stop_on_exception: false,
            timeout: None,
            aggregator: Arc::new(|exchanges: Vec<Exchange>| {
                exchanges.into_iter().last().unwrap_or_default()
            }),
        };

        let ex = Exchange::new(Message::new("inbound"));
        let result = OutcomePipeline::run(&mut seg, ex).await;

        match result {
            PipelineOutcome::Failed(err) => {
                let msg = format!("{err}");
                assert!(
                    msg.contains("branch-b-failed"),
                    "zero-success must carry the iteration-last error (branch-b-failed), got: {msg}"
                );
            }
            other => panic!("zero-success multicast must return Failed, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn multicast_sequential_partial_success_two_branches() {
        // Delta-spec scenario: branch 0 Completed, branch 1 Failed, no Stopped.
        let mut seg = MulticastSegment {
            branches: vec![always_completed_body(), always_failed_body("boom")],
            parallel: false,
            parallel_limit: None,
            stop_on_exception: false,
            timeout: None,
            aggregator: Arc::new(|exchanges: Vec<Exchange>| {
                Exchange::new(Message::new(format!("n={}", exchanges.len())))
            }),
        };

        let ex = Exchange::new(Message::new("inbound"));
        let result = OutcomePipeline::run(&mut seg, ex).await;

        match result {
            PipelineOutcome::Completed(ex) => {
                let body = ex.body_as::<String>().unwrap_or_default();
                assert_eq!(body, "n=1", "should aggregate the 1 successful branch only");
            }
            other => panic!("expected Completed with body n=1, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn multicast_stopped_branch_wins_over_failed() {
        // ADR-0058 Stopped-wins: when a branch returns Stopped, multicast
        // propagates Stopped (intentional halt per ADR-0025 section 3) and
        // does NOT return Failed or Completed.
        let mut seg = MulticastSegment {
            branches: vec![
                always_completed_body(),
                always_failed_body("branch-b-failed"),
                always_stopped_body(),
            ],
            parallel: false,
            parallel_limit: None,
            stop_on_exception: false,
            timeout: None,
            aggregator: Arc::new(|exchanges: Vec<Exchange>| {
                exchanges.into_iter().last().unwrap_or_default()
            }),
        };

        let ex = Exchange::new(Message::new("inbound"));
        let result = OutcomePipeline::run(&mut seg, ex).await;

        match result {
            PipelineOutcome::Stopped(ex) => {
                let body = ex.body_as::<String>().unwrap_or_default();
                assert_eq!(
                    body, "stop-body",
                    "Stopped must carry the stopped branch's exchange body"
                );
            }
            other => panic!("Stopped branch must win over Completed and Failed, got: {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn multicast_parallel_stopped_branch_wins_over_failed() {
        // ADR-0058 Stopped-wins in parallel mode: the lowest-index CAS
        // selection picks the stopped branch (only one Stopped branch, so the
        // winner is deterministic), in-flight tasks run to completion, and the
        // stopped exchange propagates — not Failed or Completed.
        let mut seg = MulticastSegment {
            branches: vec![
                always_completed_body(),
                always_failed_body("branch-b-failed"),
                always_stopped_body(),
            ],
            parallel: true,
            parallel_limit: None,
            stop_on_exception: false,
            timeout: None,
            aggregator: Arc::new(|exchanges: Vec<Exchange>| {
                exchanges.into_iter().last().unwrap_or_default()
            }),
        };

        let ex = Exchange::new(Message::new("inbound"));
        let result = OutcomePipeline::run(&mut seg, ex).await;

        match result {
            PipelineOutcome::Stopped(ex) => {
                let body = ex.body_as::<String>().unwrap_or_default();
                assert_eq!(
                    body, "stop-body",
                    "Stopped must carry the stopped branch's exchange body"
                );
            }
            other => panic!("Stopped branch must win over Completed and Failed, got: {other:?}"),
        }
    }
}
