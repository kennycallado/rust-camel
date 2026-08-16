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
                PipelineOutcome::Failed(camel_api::CamelError::ProcessorError("fast-fail".into()))
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
            Box::pin(
                async move { PipelineOutcome::Failed(camel_api::CamelError::ProcessorError(msg)) },
            )
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

// ── Panic mapping (segment-outcome-composition, bd rc-f88o) ──────────

/// Body whose `run` future panics with `msg` when polled (inside the
/// spawned branch task), not at `run()` call time.
fn panicking_body(msg: &str) -> OutcomeSegment {
    let msg = String::from(msg);
    #[derive(Clone)]
    struct Panicking {
        msg: String,
    }
    impl camel_api::OutcomePipeline for Panicking {
        fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
            Box::new(self.clone())
        }
        fn run<'a>(
            &'a mut self,
            _exchange: Exchange,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PipelineOutcome> + Send + 'a>>
        {
            let msg = self.msg.clone();
            Box::pin(async move {
                panic!("{}", msg);
            })
        }
    }
    OutcomeSegment::new(Box::new(Panicking { msg }))
}

#[tokio::test]
async fn multicast_parallel_panic_branch_counted_as_failed_partial_success() {
    let mut seg = MulticastSegment {
        branches: vec![
            tagged_completed_body("b0", std::time::Duration::from_millis(10)),
            always_failed_body("errA"),
            panicking_body("boom"),
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

    // Only the success branch aggregates; the Failed branch AND the
    // panicked branch are both discarded from aggregation. The panic
    // accounting (failed_branches=2, branch_count=3) is pinned by
    // multicast_parallel_partial_success_warn_fields below via the return
    // value — this test must not capture tracing fields because a
    // panicking branch races the thread-local warn dispatch under CPU
    // starvation (bd rc-u9hs).
    match result {
        PipelineOutcome::Completed(ex) => {
            let body = ex.body_as::<String>().unwrap_or_default();
            assert_eq!(
                body, "b0",
                "only the success branch aggregates; panic and failed discarded"
            );
        }
        other => panic!("expected Completed with body b0, got {other:?}"),
    }
}

/// Pins the partial-success `warn!` fields (failed_branches, branch_count)
/// deterministically (segment-outcome-composition, bd rc-f88o). Uses
/// non-panicking failed branches: a panicking branch races the thread-local
/// dispatch under load (bd rc-u9hs), while ordinary Failed outcomes reach
/// the thread-local subscriber on the same current thread.
#[tokio::test]
async fn multicast_parallel_partial_success_warn_fields() {
    use std::sync::Mutex;
    use tracing::field::{Field, Visit};

    #[derive(Default)]
    struct Captured {
        failed_branches: Option<usize>,
        branch_count: Option<usize>,
    }

    struct FieldsVisitor<'a>(&'a mut Captured);
    impl Visit for FieldsVisitor<'_> {
        fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
        fn record_u64(&mut self, field: &Field, value: u64) {
            // usize fields are recorded via record_u64 in tracing 0.1.
            if field.name() == "failed_branches" {
                self.0.failed_branches = Some(value as usize);
            } else if field.name() == "branch_count" {
                self.0.branch_count = Some(value as usize);
            }
        }
    }

    let shared: Arc<Mutex<Captured>> = Arc::new(Mutex::new(Captured::default()));

    #[derive(Clone)]
    struct TestSubscriber {
        shared: Arc<Mutex<Captured>>,
    }
    impl tracing::Subscriber for TestSubscriber {
        fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_non_zero_u64(std::num::NonZeroU64::MIN)
        }
        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
        fn enter(&self, _span: &tracing::span::Id) {}
        fn exit(&self, _span: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            if event.metadata().level() != &tracing::Level::WARN {
                return;
            }
            let mut captured = Captured::default();
            let mut visitor = FieldsVisitor(&mut captured);
            event.record(&mut visitor);
            if let Ok(mut guard) = self.shared.lock() {
                if captured.failed_branches.is_some() {
                    guard.failed_branches = captured.failed_branches;
                }
                if captured.branch_count.is_some() {
                    guard.branch_count = captured.branch_count;
                }
            }
        }
    }

    let _guard = tracing::subscriber::set_default(TestSubscriber {
        shared: Arc::clone(&shared),
    });
    // Parallel tests race tracing's per-callsite interest cache against this
    // thread-local subscriber; force a rebuild so the callsites below
    // re-evaluate against it (bd rc-u9hs).
    tracing::callsite::rebuild_interest_cache();

    let mut seg = MulticastSegment {
        branches: vec![
            tagged_completed_body("b0", std::time::Duration::ZERO),
            always_failed_body("errA"),
            always_failed_body("errB"),
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

    match result {
        PipelineOutcome::Completed(ex) => {
            let body = ex.body_as::<String>().unwrap_or_default();
            assert_eq!(body, "b0", "only the success branch aggregates");
        }
        other => panic!("expected Completed with body b0, got {other:?}"),
    }

    let captured = shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(
        captured.failed_branches,
        Some(2),
        "both failed branches must count as discarded failures"
    );
    assert_eq!(
        captured.branch_count,
        Some(3),
        "branch_count must be the total branch count"
    );
}

#[tokio::test]
async fn multicast_parallel_all_branches_panicked_returns_failed() {
    let mut seg = MulticastSegment {
        branches: vec![panicking_body("boom-0"), panicking_body("boom-1")],
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

    // ADR-0058: an all-panicked run is zero-success attempted work and MUST
    // NOT launder to Completed. Highest-index branch supplies last_error.
    match result {
        PipelineOutcome::Failed(err) => {
            let msg = format!("{err}");
            assert!(
                msg.contains("multicast branch 1 panicked"),
                "expected highest-index panic error, got: {msg}"
            );
        }
        other => panic!("expected Failed from all-panicked run, got {other:?}"),
    }
}

#[tokio::test]
async fn multicast_parallel_mixed_failed_and_panicked_reports_panic_as_last_error() {
    let mut seg = MulticastSegment {
        branches: vec![always_failed_body("errA"), panicking_body("boom")],
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

    // The panicked higher-index branch supplies the representative error,
    // proving the panicked branch entered results accounting at all.
    match result {
        PipelineOutcome::Failed(err) => {
            let msg = format!("{err}");
            assert!(
                msg.contains("multicast branch 1 panicked"),
                "expected panic error from highest-index branch, got: {msg}"
            );
            assert!(
                !msg.contains("errA"),
                "the failed branch's error must not be the representative error, got: {msg}"
            );
        }
        other => panic!("expected Failed with the panic error, got {other:?}"),
    }
}

// ── Panic precedence + payload safety (Task 1.2) ───────────────────

/// Body whose `run` future panics with a payload whose `Drop` panics —
/// exercises the `mem::forget` arm that suppresses the payload's second
/// panic. Without the forget, the payload's Drop would panic outside
/// `catch_unwind` and the branch would vanish into a dropped JoinError.
fn panicking_drop_bomb_body() -> OutcomeSegment {
    struct PanicDropBomb;
    impl Drop for PanicDropBomb {
        fn drop(&mut self) {
            panic!("drop bomb fired");
        }
    }
    #[derive(Clone)]
    struct PanickingDropBomb;
    impl camel_api::OutcomePipeline for PanickingDropBomb {
        fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
            Box::new(self.clone())
        }
        fn run<'a>(
            &'a mut self,
            _exchange: Exchange,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PipelineOutcome> + Send + 'a>>
        {
            Box::pin(async move {
                std::panic::panic_any(PanicDropBomb);
            })
        }
    }
    OutcomeSegment::new(Box::new(PanickingDropBomb))
}

#[tokio::test]
async fn multicast_stopped_branch_wins_over_panicked() {
    // ADR-0025 §3 Stop precedence: a panicked parallel branch's synthetic
    // Failed slot must not corrupt the Stop-winner scan. The stopped
    // branch's index still wins.
    //
    // Both branch bodies rendezvous at a barrier before either resolves, so
    // the panicking branch has already passed the pre-start gate and is
    // guaranteed to be polled (and panic). Without the barrier, the stopped
    // branch could set `stopped_seen` first and the panicking branch would
    // early-return `(idx, None)` without its panic future ever firing —
    // leaving the synthetic-Failed-slot invariant unpinned.
    use std::sync::atomic::AtomicBool;
    use tokio::sync::Barrier;

    let panic_polled = Arc::new(AtomicBool::new(false));
    let barrier = Arc::new(Barrier::new(2));

    #[derive(Clone)]
    struct SyncStopped {
        barrier: Arc<Barrier>,
    }
    impl camel_api::OutcomePipeline for SyncStopped {
        fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
            Box::new(self.clone())
        }
        fn run<'a>(
            &'a mut self,
            mut exchange: Exchange,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PipelineOutcome> + Send + 'a>>
        {
            let barrier = Arc::clone(&self.barrier);
            Box::pin(async move {
                barrier.wait().await;
                exchange.input.body = "stop-body".into();
                PipelineOutcome::Stopped(exchange)
            })
        }
    }

    #[derive(Clone)]
    struct SyncPanicking {
        barrier: Arc<Barrier>,
        polled: Arc<AtomicBool>,
    }
    impl camel_api::OutcomePipeline for SyncPanicking {
        fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
            Box::new(self.clone())
        }
        fn run<'a>(
            &'a mut self,
            _exchange: Exchange,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PipelineOutcome> + Send + 'a>>
        {
            let barrier = Arc::clone(&self.barrier);
            let polled = Arc::clone(&self.polled);
            Box::pin(async move {
                barrier.wait().await;
                polled.store(true, Ordering::SeqCst);
                panic!("boom");
            })
        }
    }

    let mut seg = MulticastSegment {
        branches: vec![
            OutcomeSegment::new(Box::new(SyncStopped {
                barrier: Arc::clone(&barrier),
            })),
            OutcomeSegment::new(Box::new(SyncPanicking {
                barrier: Arc::clone(&barrier),
                polled: Arc::clone(&panic_polled),
            })),
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
        other => panic!("Stopped must win over a panicked branch, got: {other:?}"),
    }

    assert!(
        panic_polled.load(Ordering::SeqCst),
        "panicking branch must have been polled before Stop won"
    );
}

#[tokio::test]
async fn multicast_branch_panicking_inside_timeout_window_maps_to_failed() {
    // The timeout wrapper's Ok(Err(payload)) arm (panic caught) must map to
    // "multicast branch {idx} panicked" — not the elapsed (timed-out) arm.
    // Generous timeout: the panic wins the race over the 10s window.
    let mut seg = MulticastSegment {
        branches: vec![panicking_body("boom")],
        parallel: true,
        parallel_limit: None,
        stop_on_exception: false,
        timeout: Some(std::time::Duration::from_secs(10)),
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
                msg.contains("multicast branch 0 panicked"),
                "expected panic-mapped error, got: {msg}"
            );
            assert!(
                !msg.contains("timed out"),
                "panic must map to the panic arm, not the timeout arm, got: {msg}"
            );
        }
        other => panic!("expected Failed from panicking branch, got {other:?}"),
    }
}

#[tokio::test]
async fn multicast_panicking_drop_payload_maps_once_without_double_panic() {
    // The panic payload's Drop panics; mem::forget in the catch arm prevents
    // a second panic outside catch_unwind (which would drop the branch into
    // a JoinError and lose the representative failure).
    let mut seg = MulticastSegment {
        branches: vec![panicking_drop_bomb_body()],
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
        PipelineOutcome::Failed(err) => {
            let msg = format!("{err}");
            assert!(
                msg.contains("multicast branch 0 panicked"),
                "expected panic-mapped error, got: {msg}"
            );
        }
        other => panic!("expected Failed from panicking-drop payload, got {other:?}"),
    }
}
