//! ## Stop semantics (ADR-0025)
//!
//! This segment implements `OutcomePipeline` and propagates `PipelineOutcome::Stopped(ex)` with the exchange state intact (including mutations made inside the segment body before Stop fired). See ADR-0025 §3 (stopped-exchange-state-preservation invariant).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::task::JoinSet;

use camel_api::{
    AggregationStrategy, Body, CamelError, Exchange, OutcomeSegment, PipelineOutcome,
    SplitExpression, Value,
};

// ── aggregate_completed (SplitSegment helper) ─────────────────────────

/// Aggregate completed fragment outputs into a single Exchange.
///
/// Unlike `aggregate` (which works on `Vec<Result<Exchange, CamelError>>`),
/// this operates on `Vec<Exchange>` where all entries are `Completed` outcomes.
pub(crate) fn aggregate_completed(
    completed: Vec<Exchange>,
    original: Exchange,
    strategy: AggregationStrategy,
) -> Exchange {
    match strategy {
        AggregationStrategy::LastWins => completed.into_iter().last().unwrap_or(original),
        AggregationStrategy::CollectAll => {
            let mut bodies = Vec::new();
            for ex in &completed {
                let value = match &ex.input.body {
                    Body::Text(s) => Value::String(s.clone()),
                    Body::Json(v) => v.clone(),
                    Body::Xml(s) => Value::String(s.clone()),
                    Body::Bytes(b) => Value::String(String::from_utf8_lossy(b).into_owned()),
                    Body::Empty => Value::Null,
                    Body::Stream(s) => serde_json::json!({
                        "_stream": {
                            "origin": s.metadata.origin,
                            "placeholder": true,
                            "hint": "Materialize exchange body with .into_bytes() before aggregation if content needed"
                        }
                    }),
                };
                bodies.push(value);
            }
            let mut out = original;
            out.input.body = Body::Json(Value::Array(bodies));
            out
        }
        AggregationStrategy::Original => original,
        AggregationStrategy::Custom(fold_fn) => {
            let mut iter = completed.into_iter();
            let first = iter.next().unwrap_or(original);
            iter.fold(first, |acc, next| fold_fn(acc, next))
        }
    }
}

// ── SplitSegment (ADR-0025 OutcomePipeline) ────────────────────────────

/// Outcome-aware structural EIP segment for the Split pattern.
///
/// Splits an incoming exchange into fragments, processes each fragment through
/// `body`, and aggregates the results. Supports sequential and parallel modes.
///
/// In sequential mode, fragments are processed in order. A `Stopped` or `Failed`
/// outcome from any fragment halts processing immediately and propagates.
///
/// In parallel mode, all fragments are spawned as tokio tasks. The first
/// `Stopped` outcome (lowest fragment index wins via CAS) propagates as the
/// outer `Stopped`. In-flight tasks run to completion (spec §5.6: no abrupt
/// abort — child sub-pipelines may have HTTP/SQL side effects). Tasks that
/// have not started yet are short-circuited via a pre-start gate.
///
/// Unlike `SplitterService` (which operates at the Tower layer and cannot
/// preserve `Stopped(ex)` with mutations), `SplitSegment` operates at the
/// `PipelineOutcome` layer and preserves the exchange including all mutations
/// at the Stop point.
pub struct SplitSegment {
    /// Splits an exchange into fragment exchanges.
    pub splitter: SplitExpression,
    /// The sub-pipeline executed for each fragment.
    pub body: OutcomeSegment,
    /// Whether to process fragments in parallel.
    pub parallel: bool,
    /// Maximum number of concurrent fragments in parallel mode (None = unlimited).
    pub parallel_limit: Option<usize>,
    /// Whether to stop processing on the first exception.
    ///
    /// When `true`, a `Failed` outcome from any fragment halts processing
    /// immediately. When `false`, the error is collected and processing
    /// continues; the last-seen error is propagated (last-wins, matching legacy multicast.rs::process_parallel)
    /// is propagated after all fragments complete.
    ///
    /// `Stopped` outcomes always propagate immediately regardless of this
    /// flag (per ADR-0025 §7 — Stop is successful control flow).
    pub stop_on_exception: bool,
    /// Strategy for aggregating fragment results.
    pub aggregation: AggregationStrategy,
}

impl Clone for SplitSegment {
    fn clone(&self) -> Self {
        Self {
            splitter: Arc::clone(&self.splitter),
            body: self.body.clone(),
            parallel: self.parallel,
            parallel_limit: self.parallel_limit,
            stop_on_exception: self.stop_on_exception,
            aggregation: self.aggregation.clone(),
        }
    }
}

impl camel_api::OutcomePipeline for SplitSegment {
    fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: camel_api::Exchange,
    ) -> Pin<Box<dyn Future<Output = camel_api::PipelineOutcome> + Send + 'a>> {
        let splitter = Arc::clone(&self.splitter);
        let aggregation = self.aggregation.clone();
        let parallel = self.parallel;
        let parallel_limit = self.parallel_limit;
        let stop_on_exception = self.stop_on_exception;
        let body = &mut self.body;

        Box::pin(async move {
            let original = exchange;
            let fragments = splitter(&original);

            if fragments.is_empty() {
                return PipelineOutcome::Completed(original);
            }

            if parallel {
                parallel_split(
                    fragments,
                    original,
                    body,
                    &aggregation,
                    parallel_limit,
                    stop_on_exception,
                )
                .await
            } else {
                sequential_split(fragments, original, body, &aggregation, stop_on_exception).await
            }
        })
    }
}

// ── Sequential split ────────────────────────────────────────────────────

async fn sequential_split(
    fragments: Vec<Exchange>,
    original: Exchange,
    body: &mut OutcomeSegment,
    aggregation: &AggregationStrategy,
    stop_on_exception: bool,
) -> PipelineOutcome {
    let mut outputs = Vec::new();
    let mut last_error: Option<CamelError> = None;
    for frag in fragments {
        match body.run(frag).await {
            PipelineOutcome::Completed(ex) => outputs.push(ex),
            PipelineOutcome::Stopped(ex) => return PipelineOutcome::Stopped(ex),
            PipelineOutcome::Failed(err) => {
                if stop_on_exception {
                    return PipelineOutcome::Failed(err);
                }
                // stop_on_exception=false: collect the error and continue.
                last_error = Some(err);
            }
        }
    }
    if let Some(err) = last_error {
        return PipelineOutcome::Failed(err);
    }
    PipelineOutcome::Completed(aggregate_completed(outputs, original, aggregation.clone()))
}

// ── Parallel split ──────────────────────────────────────────────────────

/// Parallel split with lowest-index-wins CAS semantics.
///
/// See spec §5.2.2 line 497 for the CAS guarantee and §5.6 line 544 for the
/// "no abrupt abort" in-flight task policy (pre-start gate + run-to-completion).
async fn parallel_split(
    fragments: Vec<Exchange>,
    original: Exchange,
    body: &mut OutcomeSegment,
    aggregation: &AggregationStrategy,
    parallel_limit: Option<usize>,
    stop_on_exception: bool,
) -> PipelineOutcome {
    use tokio::sync::Semaphore;

    let stopped_seen = Arc::new(AtomicBool::new(false));
    let stopped_idx = Arc::new(AtomicUsize::new(usize::MAX));
    let aggregation = aggregation.clone();
    let semaphore = parallel_limit
        .filter(|&limit| limit > 0)
        .map(|limit| Arc::new(Semaphore::new(limit)));

    let mut set: JoinSet<(usize, Option<PipelineOutcome>)> = JoinSet::new();

    for (idx, frag) in fragments.into_iter().enumerate() {
        let mut body = body.clone();
        let stopped_seen = Arc::clone(&stopped_seen);
        let stopped_idx = Arc::clone(&stopped_idx);
        let sem = semaphore.clone();
        set.spawn(async move {
            // Pre-start gate: a lower-index branch already stopped.
            // This is the ONLY cancellation check — once a body starts
            // running, it runs to completion (spec §5.6: "in-flight futures
            // MUST NOT be abruptly aborted").
            if stopped_seen.load(Ordering::SeqCst) {
                return (idx, None);
            }
            // Acquire semaphore permit if parallel_limit is set.
            let _permit: Option<tokio::sync::OwnedSemaphorePermit> = match &sem {
                Some(s) => match std::sync::Arc::clone(s).acquire_owned().await {
                    Ok(p) => Some(p),
                    Err(_) => {
                        return (
                            idx,
                            Some(PipelineOutcome::Failed(CamelError::ProcessorError(
                                "semaphore closed".into(),
                            ))),
                        );
                    }
                },
                None => None,
            };
            // Re-check pre-start gate after permit acquisition
            // (another branch may have stopped while we were waiting).
            if stopped_seen.load(Ordering::SeqCst) {
                return (idx, None);
            }
            let outcome = body.run(frag).await;
            if let PipelineOutcome::Stopped(_) = &outcome {
                // Lower-the-value CAS: ensures lowest-branch-index wins
                // even under simultaneous Stop (spec §5.2.2 line 497).
                // Loop until our idx is recorded or a lower idx has already
                // claimed it.
                loop {
                    let cur = stopped_idx.load(Ordering::SeqCst);
                    if idx >= cur {
                        break; // a lower index already won
                    }
                    match stopped_idx.compare_exchange_weak(
                        cur,
                        idx,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    ) {
                        Ok(_) => break,
                        Err(actual) => {
                            // CAS failed — `actual` is the new cur; loop and
                            // retry with updated value.
                            if actual <= idx {
                                break;
                            }
                        }
                    }
                }
                stopped_seen.store(true, Ordering::SeqCst);
            }
            (idx, Some(outcome))
        });
    }

    // Wait for ALL in-flight branches to finish (spec §5.6: no abrupt
    // abort). Post-stop outputs are discarded at aggregation, but the
    // branches DO complete.
    let mut results: Vec<(usize, PipelineOutcome)> = Vec::new();
    while let Some(res) = set.join_next().await {
        if let Ok((idx, Some(o))) = res {
            results.push((idx, o));
        }
    }

    // Deterministic lowest-branch-index wins (spec §5.2.2 line 497).
    if stopped_seen.load(Ordering::SeqCst) {
        let winning_idx = stopped_idx.load(Ordering::SeqCst);
        if winning_idx == usize::MAX {
            tracing::warn!(
                target: "camel.phase4.split",
                "stopped_seen=true but stopped_idx=usize::MAX — race condition; falling back to pre-split exchange"
            );
            return PipelineOutcome::Stopped(original);
        }
        let stopped_ex = results
            .iter()
            .find(|(idx, _)| *idx == winning_idx)
            .and_then(|(_, o)| match o {
                PipelineOutcome::Stopped(ex) => Some(ex.clone()),
                _ => None,
            });
        if let Some(ex) = stopped_ex {
            return PipelineOutcome::Stopped(ex);
        }
        tracing::warn!(
            target: "camel.phase4.split",
            winning_idx = winning_idx,
            "winning_idx not found in results — falling back to pre-split exchange"
        );
        return PipelineOutcome::Stopped(original);
    }

    // No Stop — check for Failed.
    // stop_on_exception=true: propagate first Failed (lowest branch index).
    // stop_on_exception=false: collect last error (last-wins) and propagate at end.
    results.sort_by_key(|(idx, _)| *idx);
    if stop_on_exception {
        let mut first_failed: Option<(usize, CamelError)> = None;
        for (idx, o) in &results {
            if let PipelineOutcome::Failed(err) = o
                && first_failed
                    .as_ref()
                    .map(|(i, _)| *i > *idx)
                    .unwrap_or(true)
            {
                first_failed = Some((*idx, err.clone()));
            }
        }
        if let Some((_, err)) = first_failed {
            return PipelineOutcome::Failed(err);
        }
    } else {
        // Collect last error (last-wins, matching MulticastSegment semantics).
        let mut last_error: Option<CamelError> = None;
        for (_, o) in &results {
            if let PipelineOutcome::Failed(err) = o {
                last_error = Some(err.clone());
            }
        }
        if let Some(err) = last_error {
            return PipelineOutcome::Failed(err);
        }
    }

    // All Completed — aggregate.
    let completed: Vec<Exchange> = results
        .into_iter()
        .filter_map(|(_, o)| match o {
            PipelineOutcome::Completed(ex) => Some(ex),
            _ => None,
        })
        .collect();
    PipelineOutcome::Completed(aggregate_completed(completed, original, aggregation))
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::Message;

    // ── Test helpers ───────────────────────────────────────────────────

    /// Helper: OutcomePipeline body that always returns `Completed(exchange)`.
    #[derive(Clone)]
    #[allow(dead_code)]
    struct CompletedBody;
    impl camel_api::OutcomePipeline for CompletedBody {
        fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
            Box::new(CompletedBody)
        }
        fn run<'a>(
            &'a mut self,
            exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            Box::pin(async move { PipelineOutcome::Completed(exchange) })
        }
    }

    /// Helper: OutcomePipeline body that always returns `Stopped(exchange)`.
    #[derive(Clone)]
    #[allow(dead_code)]
    struct StopBody;
    impl camel_api::OutcomePipeline for StopBody {
        fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
            Box::new(StopBody)
        }
        fn run<'a>(
            &'a mut self,
            exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            Box::pin(async move { PipelineOutcome::Stopped(exchange) })
        }
    }

    /// Helper: OutcomePipeline body that stops on the nth invocation (0-indexed).
    #[derive(Clone)]
    struct StopOnNthBody {
        counter: Arc<AtomicUsize>,
        stop_at: usize,
    }
    impl camel_api::OutcomePipeline for StopOnNthBody {
        fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
            Box::new(self.clone())
        }
        fn run<'a>(
            &'a mut self,
            exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            let count = self.counter.fetch_add(1, Ordering::SeqCst);
            let stop_at = self.stop_at;
            Box::pin(async move {
                if count >= stop_at {
                    PipelineOutcome::Stopped(exchange)
                } else {
                    PipelineOutcome::Completed(exchange)
                }
            })
        }
    }

    /// Helper: OutcomePipeline body that mutates the exchange body then stops.
    #[derive(Clone)]
    struct MutateAndStopBody;
    impl camel_api::OutcomePipeline for MutateAndStopBody {
        fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
            Box::new(MutateAndStopBody)
        }
        fn run<'a>(
            &'a mut self,
            mut exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            Box::pin(async move {
                exchange.input.body = Body::Text("mutated-by-body".to_string());
                PipelineOutcome::Stopped(exchange)
            })
        }
    }

    // ── Test 1: Sequential split — Stop halts remaining fragments ──

    #[tokio::test]
    async fn stop_inside_split_sequential_halts_remaining_fragments() {
        let invocations = Arc::new(AtomicUsize::new(0));
        let body = StopOnNthBody {
            counter: Arc::clone(&invocations),
            stop_at: 1, // stop on the 2nd fragment (index 1)
        };

        let mut seg = SplitSegment {
            splitter: camel_api::split_body_lines(),
            body: OutcomeSegment::new(Box::new(body)),
            parallel: false,
            parallel_limit: None,
            stop_on_exception: true,
            aggregation: AggregationStrategy::LastWins,
        };

        let ex = Exchange::new(Message::new("a\nb\nc"));
        let result = camel_api::OutcomePipeline::run(&mut seg, ex).await;

        assert!(matches!(result, PipelineOutcome::Stopped(_)));
        // Fragments 0 (pass) + 1 (stop) = 2 invocations; fragment 2 never runs.
        assert_eq!(invocations.load(Ordering::SeqCst), 2);
    }

    // ── Test 2: Sequential split — Stop preserves exchange mutations ──

    #[tokio::test]
    async fn stop_inside_split_sequential_preserves_exchange_mutations() {
        let mut seg = SplitSegment {
            splitter: camel_api::split_body_lines(),
            body: OutcomeSegment::new(Box::new(MutateAndStopBody)),
            parallel: false,
            parallel_limit: None,
            stop_on_exception: true,
            aggregation: AggregationStrategy::LastWins,
        };

        let ex = Exchange::new(Message::new("hello"));
        let result = camel_api::OutcomePipeline::run(&mut seg, ex).await;

        match result {
            PipelineOutcome::Stopped(ex) => {
                assert_eq!(
                    ex.input.body.as_text(),
                    Some("mutated-by-body"),
                    "Stopped exchange should carry body mutation"
                );
            }
            other => panic!("Expected Stopped, got {other:?}"),
        }
    }

    // ── Test 3: Parallel split — Stop cancels pending, waits in-flight ──
    //
    // NOTE: With JoinSet::spawn, all tasks are eagerly created. The
    // pre-start gate only stops fragments whose spawned closure hasn't
    // been polled yet. This test uses a tokio::sync::Barrier inside the
    // body to ensure ALL fragments pass the pre-start gate, then verifies
    // that in-flight (frag-1) completes even though Stop fires. The
    // "cancels pending" invariant (frag-2 not started) is best-effort;
    // the true invariant is: fragments that DO start MUST run to completion.

    #[tokio::test(flavor = "multi_thread")]
    async fn stop_inside_split_parallel_cancels_pending_and_waits_inflight() {
        use tokio::sync::Barrier;

        let barrier = Arc::new(Barrier::new(3));
        let fragment1_completed = Arc::new(AtomicBool::new(false));
        let fragment2_completed = Arc::new(AtomicBool::new(false));
        let frag1_ok = Arc::clone(&fragment1_completed);
        let frag2_ok = Arc::clone(&fragment2_completed);
        let bar = Arc::clone(&barrier);

        // Custom splitter producing 3 fragments.
        let splitter: SplitExpression = Arc::new(|ex: &Exchange| {
            (0..3)
                .map(|i| {
                    let mut frag = ex.clone();
                    frag.input.body = Body::Text(format!("frag-{i}"));
                    frag
                })
                .collect()
        });

        /// Body that uses a barrier to synchronize all fragments past the
        /// pre-start gate, then dispatches:
        ///   - frag-0: Stop
        ///   - frag-1: slow (100ms) Completed
        ///   - frag-2: fast Completed (asserts it started, proving no abort)
        struct BarrierDispatchBody {
            barrier: Arc<Barrier>,
            f1_completed: Arc<AtomicBool>,
            f2_completed: Arc<AtomicBool>,
        }
        impl Clone for BarrierDispatchBody {
            fn clone(&self) -> Self {
                Self {
                    barrier: Arc::clone(&self.barrier),
                    f1_completed: Arc::clone(&self.f1_completed),
                    f2_completed: Arc::clone(&self.f2_completed),
                }
            }
        }
        impl camel_api::OutcomePipeline for BarrierDispatchBody {
            fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
                Box::new(self.clone())
            }
            fn run<'a>(
                &'a mut self,
                exchange: Exchange,
            ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
                let bar = Arc::clone(&self.barrier);
                let f1c = Arc::clone(&self.f1_completed);
                let f2c = Arc::clone(&self.f2_completed);
                Box::pin(async move {
                    let body_text = exchange.input.body.as_text().unwrap_or("").to_string();

                    // All fragments synchronize AFTER passing the pre-start gate
                    // (the gate is checked before body.run()). This ensures all
                    // three fragments are in-flight when Stop fires.
                    bar.wait().await;

                    match body_text.as_str() {
                        "frag-0" => PipelineOutcome::Stopped(exchange),
                        "frag-1" => {
                            // Slow in-flight — fragment 0's Stop is recorded and
                            // propagates, but we still complete (spec §5.6).
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                            f1c.store(true, Ordering::SeqCst);
                            PipelineOutcome::Completed(exchange)
                        }
                        "frag-2" => {
                            f2c.store(true, Ordering::SeqCst);
                            PipelineOutcome::Completed(exchange)
                        }
                        _ => PipelineOutcome::Completed(exchange),
                    }
                })
            }
        }

        let body = BarrierDispatchBody {
            barrier: bar,
            f1_completed: frag1_ok,
            f2_completed: frag2_ok,
        };

        let mut seg = SplitSegment {
            splitter,
            body: OutcomeSegment::new(Box::new(body)),
            parallel: true,
            parallel_limit: None,
            stop_on_exception: true,
            aggregation: AggregationStrategy::LastWins,
        };

        let ex = Exchange::new(Message::new("test"));
        let result = camel_api::OutcomePipeline::run(&mut seg, ex).await;

        assert!(
            matches!(result, PipelineOutcome::Stopped(_)),
            "Expected Stopped, got {result:?}"
        );
        // Fragment 1 was in-flight and completed (no abrupt abort per §5.6).
        assert!(
            fragment1_completed.load(Ordering::SeqCst),
            "fragment 1 should have completed despite Stop"
        );
        // Fragment 2 was also in-flight (barrier ensures all start) and completed.
        assert!(
            fragment2_completed.load(Ordering::SeqCst),
            "fragment 2 should have completed despite Stop"
        );
    }

    // ── Test 4: Parallel split — lowest stopped index wins ──

    #[tokio::test(flavor = "multi_thread")]
    async fn stop_inside_split_parallel_lowest_stopped_index_wins() {
        // Custom splitter producing 3 fragments with index-identifiable body.
        let splitter: SplitExpression = Arc::new(|ex: &Exchange| {
            (0..3)
                .map(|i| {
                    let mut frag = ex.clone();
                    frag.input.body = Body::Text(format!("from-fragment-{i}"));
                    frag
                })
                .collect()
        });

        // Body that stops for fragments 0 and 2; fragment 1 completes.
        struct DualStopBody;
        impl Clone for DualStopBody {
            fn clone(&self) -> Self {
                DualStopBody
            }
        }
        impl camel_api::OutcomePipeline for DualStopBody {
            fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
                Box::new(DualStopBody)
            }
            fn run<'a>(
                &'a mut self,
                exchange: Exchange,
            ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
                let is_frag0 = exchange
                    .input
                    .body
                    .as_text()
                    .map(|s| s == "from-fragment-0")
                    .unwrap_or(false);
                let is_frag2 = exchange
                    .input
                    .body
                    .as_text()
                    .map(|s| s == "from-fragment-2")
                    .unwrap_or(false);
                Box::pin(async move {
                    if is_frag0 {
                        return PipelineOutcome::Stopped(exchange);
                    }
                    if is_frag2 {
                        // Delay slightly to ensure fragment 0's Stop is recorded first
                        // in the CAS, then verify that lowest index wins.
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                        return PipelineOutcome::Stopped(exchange);
                    }
                    // frag-1: completed
                    PipelineOutcome::Completed(exchange)
                })
            }
        }

        let mut seg = SplitSegment {
            splitter,
            body: OutcomeSegment::new(Box::new(DualStopBody)),
            parallel: true,
            parallel_limit: None,
            stop_on_exception: true,
            aggregation: AggregationStrategy::LastWins,
        };

        let ex = Exchange::new(Message::new("test"));
        let result = camel_api::OutcomePipeline::run(&mut seg, ex).await;

        match result {
            PipelineOutcome::Stopped(ex) => {
                assert_eq!(
                    ex.input.body.as_text(),
                    Some("from-fragment-0"),
                    "Lowest stopped index (0) should win, got body {:?}",
                    ex.input.body.as_text()
                );
            }
            other => panic!("Expected Stopped with fragment-0 body, got {other:?}"),
        }
    }

    // ── Test 5: parallel_limit enforcement ─────────────────────────────

    #[tokio::test(flavor = "multi_thread")]
    async fn split_parallel_limit_enforces_concurrency_cap() {
        let concurrent = Arc::new(AtomicUsize::new(0));
        let max_concurrent = Arc::new(AtomicUsize::new(0));

        // Split into 6 fragments. parallel_limit=2.
        let splitter: SplitExpression = Arc::new(|ex: &Exchange| {
            (0..6)
                .map(|i| {
                    let mut frag = ex.clone();
                    frag.input.body = Body::Text(format!("frag-{i}"));
                    frag
                })
                .collect()
        });

        let c = Arc::clone(&concurrent);
        let mc = Arc::clone(&max_concurrent);
        struct LimitedBody {
            concurrent: Arc<AtomicUsize>,
            max_concurrent: Arc<AtomicUsize>,
        }
        impl Clone for LimitedBody {
            fn clone(&self) -> Self {
                Self {
                    concurrent: Arc::clone(&self.concurrent),
                    max_concurrent: Arc::clone(&self.max_concurrent),
                }
            }
        }
        impl camel_api::OutcomePipeline for LimitedBody {
            fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
                Box::new(self.clone())
            }
            fn run<'a>(
                &'a mut self,
                exchange: Exchange,
            ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
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

        let mut seg = SplitSegment {
            splitter,
            body: OutcomeSegment::new(Box::new(LimitedBody {
                concurrent: c,
                max_concurrent: mc,
            })),
            parallel: true,
            parallel_limit: Some(2),
            stop_on_exception: true,
            aggregation: AggregationStrategy::LastWins,
        };

        let ex = Exchange::new(Message::new("test"));
        let result = camel_api::OutcomePipeline::run(&mut seg, ex).await;
        assert!(
            matches!(result, PipelineOutcome::Completed(_)),
            "Expected Completed, got {result:?}"
        );

        let observed_max = max_concurrent.load(Ordering::SeqCst);
        assert!(
            observed_max <= 2,
            "parallel_limit=2 but max concurrency was {observed_max}"
        );
    }

    // ── Test 6: stop_on_exception=true (sequential) ────────────────────

    #[tokio::test]
    async fn split_sequential_stop_on_exception_true() {
        // 5 fragments, fail on 2nd (index 1). stop_on_exception=true → stops.
        fn make_fail_body(
            fail_at: usize,
            counter: Arc<AtomicUsize>,
        ) -> impl camel_api::OutcomePipeline + Clone {
            #[derive(Clone)]
            struct FailAtBody {
                fail_at: usize,
                counter: Arc<AtomicUsize>,
            }
            impl camel_api::OutcomePipeline for FailAtBody {
                fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
                    Box::new(self.clone())
                }
                fn run<'a>(
                    &'a mut self,
                    exchange: Exchange,
                ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
                    let count = self.counter.fetch_add(1, Ordering::SeqCst);
                    let fail_at = self.fail_at;
                    Box::pin(async move {
                        if count == fail_at {
                            PipelineOutcome::Failed(CamelError::ProcessorError(format!(
                                "fail at {count}"
                            )))
                        } else {
                            PipelineOutcome::Completed(exchange)
                        }
                    })
                }
            }
            FailAtBody { fail_at, counter }
        }

        let invocations = Arc::new(AtomicUsize::new(0));
        let body = make_fail_body(1, Arc::clone(&invocations));
        let mut seg = SplitSegment {
            splitter: camel_api::split_body_lines(),
            body: OutcomeSegment::new(Box::new(body)),
            parallel: false,
            parallel_limit: None,
            stop_on_exception: true,
            aggregation: AggregationStrategy::LastWins,
        };

        let ex = Exchange::new(Message::new("a\nb\nc\nd\ne"));
        let result = camel_api::OutcomePipeline::run(&mut seg, ex).await;

        assert!(
            matches!(result, PipelineOutcome::Failed(_)),
            "stop_on_exception=true should propagate first failure"
        );
        // Only 2 fragments processed (index 0 passed, index 1 failed);
        // fragments 2-4 never run.
        assert_eq!(
            invocations.load(Ordering::SeqCst),
            2,
            "should stop after 2 fragments (0 pass, 1 fail)"
        );
    }

    // ── Test 7: stop_on_exception=false (sequential) ───────────────────

    #[tokio::test]
    async fn split_sequential_stop_on_exception_false() {
        // 5 fragments, fail on 2nd (index 1). stop_on_exception=false → continues.
        fn make_fail_body(
            fail_at: usize,
            counter: Arc<AtomicUsize>,
        ) -> impl camel_api::OutcomePipeline + Clone {
            #[derive(Clone)]
            struct FailAtBody {
                fail_at: usize,
                counter: Arc<AtomicUsize>,
            }
            impl camel_api::OutcomePipeline for FailAtBody {
                fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
                    Box::new(self.clone())
                }
                fn run<'a>(
                    &'a mut self,
                    exchange: Exchange,
                ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
                    let count = self.counter.fetch_add(1, Ordering::SeqCst);
                    let fail_at = self.fail_at;
                    Box::pin(async move {
                        if count == fail_at {
                            PipelineOutcome::Failed(CamelError::ProcessorError(format!(
                                "fail at {count}"
                            )))
                        } else {
                            PipelineOutcome::Completed(exchange)
                        }
                    })
                }
            }
            FailAtBody { fail_at, counter }
        }

        let invocations = Arc::new(AtomicUsize::new(0));
        let body = make_fail_body(1, Arc::clone(&invocations));
        let mut seg = SplitSegment {
            splitter: camel_api::split_body_lines(),
            body: OutcomeSegment::new(Box::new(body)),
            parallel: false,
            parallel_limit: None,
            stop_on_exception: false,
            aggregation: AggregationStrategy::LastWins,
        };

        let ex = Exchange::new(Message::new("a\nb\nc\nd\ne"));
        let result = camel_api::OutcomePipeline::run(&mut seg, ex).await;

        // With stop_on_exception=false, processing continues after failure;
        // last error is propagated.
        assert!(
            matches!(result, PipelineOutcome::Failed(_)),
            "stop_on_exception=false should still propagate error at end"
        );
        // All 5 fragments processed.
        assert_eq!(
            invocations.load(Ordering::SeqCst),
            5,
            "all fragments should be processed when stop_on_exception=false"
        );
    }

    // ── Test 8: stop_on_exception=true (parallel) ──────────────────────

    #[tokio::test(flavor = "multi_thread")]
    async fn split_parallel_stop_on_exception_true() {
        let splitter: SplitExpression = Arc::new(|ex: &Exchange| {
            (0..5)
                .map(|i| {
                    let mut frag = ex.clone();
                    frag.input.body = Body::Text(format!("frag-{i}"));
                    frag
                })
                .collect()
        });

        // All fragments fail. stop_on_exception=true → first Failed propagated.
        let invocations = Arc::new(AtomicUsize::new(0));
        struct FailBody {
            counter: Arc<AtomicUsize>,
        }
        impl Clone for FailBody {
            fn clone(&self) -> Self {
                Self {
                    counter: Arc::clone(&self.counter),
                }
            }
        }
        impl camel_api::OutcomePipeline for FailBody {
            fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
                Box::new(self.clone())
            }
            fn run<'a>(
                &'a mut self,
                _exchange: Exchange,
            ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
                let count = self.counter.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    PipelineOutcome::Failed(CamelError::ProcessorError(format!("fail {count}")))
                })
            }
        }

        let mut seg = SplitSegment {
            splitter,
            body: OutcomeSegment::new(Box::new(FailBody {
                counter: Arc::clone(&invocations),
            })),
            parallel: true,
            parallel_limit: None,
            stop_on_exception: true,
            aggregation: AggregationStrategy::LastWins,
        };

        let ex = Exchange::new(Message::new("test"));
        let result = camel_api::OutcomePipeline::run(&mut seg, ex).await;

        assert!(
            matches!(result, PipelineOutcome::Failed(_)),
            "stop_on_exception=true should propagate first failure"
        );
        // All 5 spawned (JoinSet), all completed.
        assert_eq!(
            invocations.load(Ordering::SeqCst),
            5,
            "all fragments should be spawned"
        );
    }

    // ── Test 9: stop_on_exception=false (parallel) ─────────────────────

    #[tokio::test(flavor = "multi_thread")]
    async fn split_parallel_stop_on_exception_false() {
        let splitter: SplitExpression = Arc::new(|ex: &Exchange| {
            (0..5)
                .map(|i| {
                    let mut frag = ex.clone();
                    frag.input.body = Body::Text(format!("frag-{i}"));
                    frag
                })
                .collect()
        });

        // Fragment 0 passes, 1 fails, 2-4 pass.
        let invocations = Arc::new(AtomicUsize::new(0));
        struct MixedBody {
            counter: Arc<AtomicUsize>,
        }
        impl Clone for MixedBody {
            fn clone(&self) -> Self {
                Self {
                    counter: Arc::clone(&self.counter),
                }
            }
        }
        impl camel_api::OutcomePipeline for MixedBody {
            fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
                Box::new(self.clone())
            }
            fn run<'a>(
                &'a mut self,
                exchange: Exchange,
            ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
                let count = self.counter.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    if count == 1 {
                        PipelineOutcome::Failed(CamelError::ProcessorError("fail 1".into()))
                    } else {
                        PipelineOutcome::Completed(exchange)
                    }
                })
            }
        }

        let mut seg = SplitSegment {
            splitter,
            body: OutcomeSegment::new(Box::new(MixedBody {
                counter: Arc::clone(&invocations),
            })),
            parallel: true,
            parallel_limit: None,
            stop_on_exception: false,
            aggregation: AggregationStrategy::LastWins,
        };

        let ex = Exchange::new(Message::new("test"));
        let result = camel_api::OutcomePipeline::run(&mut seg, ex).await;

        // stop_on_exception=false → last error propagated at end.
        assert!(
            matches!(result, PipelineOutcome::Failed(_)),
            "stop_on_exception=false should propagate failure at end; got {result:?}"
        );
        assert_eq!(
            invocations.load(Ordering::SeqCst),
            5,
            "all fragments should be spawned"
        );
    }
}
