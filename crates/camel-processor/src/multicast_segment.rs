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
    /// immediately (sequential) or is propagated after all in-flight branches
    /// complete (parallel). When `false`, failures are collected and processing
    /// continues; the last error (last-wins, matching legacy `LastWins` aggregation)
    /// is propagated after all branches complete.
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
        return camel_api::PipelineOutcome::Failed(err);
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
    // stop_on_exception=false: collect last error (last-wins) and propagate at end.
    results.sort_by_key(|(idx, _)| *idx);
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
        let mut last_error: Option<camel_api::CamelError> = None;
        for (_, o) in &results {
            if let camel_api::PipelineOutcome::Failed(err) = o {
                last_error = Some(err.clone());
            }
        }
        if let Some(err) = last_error {
            return camel_api::PipelineOutcome::Failed(err);
        }
    }

    // All Completed — aggregate.
    let completed: Vec<Exchange> = results
        .into_iter()
        .filter_map(|(_, o)| match o {
            camel_api::PipelineOutcome::Completed(ex) => Some(ex),
            _ => None,
        })
        .collect();
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

    // ── Test B: sequential stop_on_exception=false ───────────────────

    #[tokio::test]
    async fn multicast_sequential_stop_on_exception_false() {
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
                exchanges.into_iter().last().unwrap_or_default()
            }),
        };

        let ex = Exchange::new(Message::new("test"));
        let result = OutcomePipeline::run(&mut seg, ex).await;

        // With stop_on_exception=false, error propagated at end, all branches execute.
        assert!(
            matches!(result, PipelineOutcome::Failed(_)),
            "should propagate error at end"
        );
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
        // Deterministic bodies: always-pass, always-fail-err1, always-fail-err2.
        fn always_pass_body() -> OutcomeSegment {
            #[derive(Clone)]
            struct PassBody;
            impl camel_api::OutcomePipeline for PassBody {
                fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
                    Box::new(PassBody)
                }
                fn run<'a>(
                    &'a mut self,
                    exchange: Exchange,
                ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
                    Box::pin(async move { PipelineOutcome::Completed(exchange) })
                }
            }
            OutcomeSegment::new(Box::new(PassBody))
        }
        fn always_fail_body(msg: &'static str) -> OutcomeSegment {
            #[derive(Clone)]
            struct FailBody {
                msg: &'static str,
            }
            impl camel_api::OutcomePipeline for FailBody {
                fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
                    Box::new(self.clone())
                }
                fn run<'a>(
                    &'a mut self,
                    _exchange: Exchange,
                ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
                    let msg = self.msg;
                    Box::pin(async move {
                        PipelineOutcome::Failed(camel_api::CamelError::ProcessorError(
                            msg.to_string(),
                        ))
                    })
                }
            }
            OutcomeSegment::new(Box::new(FailBody { msg }))
        }

        let target: Arc<dyn Fn(Vec<Exchange>) -> Exchange + Send + Sync> =
            Arc::new(|exchanges: Vec<Exchange>| exchanges.into_iter().last().unwrap_or_default());

        let mut seg = MulticastSegment {
            branches: vec![
                always_fail_body("err1"), // branch 0 fails with err1
                always_pass_body(),       // branch 1 completes
                always_fail_body("err2"), // branch 2 fails with err2
            ],
            parallel: true,
            parallel_limit: None,
            stop_on_exception: false,
            timeout: None,
            aggregator: target,
        };

        let ex = Exchange::new(Message::new("test"));
        let result = OutcomePipeline::run(&mut seg, ex).await;

        // stop_on_exception=false → last error (highest idx) propagated.
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
        struct FastPassBody;
        impl camel_api::OutcomePipeline for FastPassBody {
            fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
                Box::new(FastPassBody)
            }
            fn run<'a>(
                &'a mut self,
                exchange: Exchange,
            ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
                Box::pin(async move { PipelineOutcome::Completed(exchange) })
            }
        }

        let target: Arc<dyn Fn(Vec<Exchange>) -> Exchange + Send + Sync> =
            Arc::new(|exchanges: Vec<Exchange>| exchanges.into_iter().last().unwrap_or_default());

        let mut seg = MulticastSegment {
            branches: vec![
                OutcomeSegment::new(Box::new(SlowBody)), // branch 0: 200ms (times out)
                OutcomeSegment::new(Box::new(FastPassBody)), // branch 1: completes
            ],
            parallel: true,
            parallel_limit: None,
            stop_on_exception: false,
            timeout: Some(std::time::Duration::from_millis(50)),
            aggregator: target,
        };

        let ex = Exchange::new(Message::new("test"));
        let result = OutcomePipeline::run(&mut seg, ex).await;

        // With stop_on_exception=false and timeout, timeout error is collected
        // as last_error and propagated.
        assert!(
            matches!(result, PipelineOutcome::Failed(_)),
            "Expected Failed due to timeout with stop_on_exception=false, got {result:?}"
        );
    }
}
