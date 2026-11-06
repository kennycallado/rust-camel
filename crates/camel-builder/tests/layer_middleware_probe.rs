//! Probe-evidence battery for the Tower layer hooks (mission 510, Task 3.2).
//!
//! This is the durable version of the committed compile probe
//! (`openspec/changes/builder-layer-middleware/probe/src/main.rs`): the same
//! timing measurements, but driven through the REAL `camel_api::LayerFactory`,
//! `camel_api::CamelErrorAdapter`, and `From<BoxError> for CamelError` path
//! instead of probe-local shims.
//!
//! The battery runs at the PROCESSOR level (no component or route harness) so
//! the timing envelopes stay measurable. The committed probe measured
//! concurrency 1.000, timeout 1.04, and rate 1.007 (PROBE.md evidence table
//! P3c) on the same configuration shape. The blessed envelopes
//! (`specs/builder/spec.md`, "Middleware probe evidence") are:
//!
//! - concurrency: strict cap and `max_in_flight / limit <= 1.15`;
//! - timeout: `elapsed >= t` (no early firing) and `elapsed / t <= 1.25`;
//! - rate limit: `elapsed >= floor((k - 1) / r) * p` (shared state) and at
//!   most `1.25x` that required delay.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use camel_api::{
    BoxProcessor, CamelError, CamelErrorAdapter, Exchange, LayerFactory, Message, ProcessorFn,
    ProcessorLayer,
};
use tokio::task::JoinSet;
use tower::Layer;
use tower::Service;
use tower::ServiceExt;
use tower::buffer::Buffer;
use tower::limit::{ConcurrencyLimitLayer, RateLimitLayer};
use tower::timeout::TimeoutLayer;

/// Finite deadline for every concurrent battery.
///
/// The spec's bounded-completion scenario requires a per-test watchdog: a
/// readiness deadlock must surface as a test failure, never as a hung suite.
const WATCHDOG: Duration = Duration::from_secs(30);

/// Producer that echoes the exchange after `delay`, tracking concurrency.
///
/// `in_flight` counts invocations currently inside the producer;
/// `max_seen` records the high-water mark (the strict-cap measurement).
fn counting_processor(
    in_flight: Arc<AtomicUsize>,
    max_seen: Arc<AtomicUsize>,
    delay: Duration,
) -> BoxProcessor {
    BoxProcessor::new(ProcessorFn::new(move |exchange: Exchange| {
        let in_flight = Arc::clone(&in_flight);
        let max_seen = Arc::clone(&max_seen);
        async move {
            let current = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            max_seen.fetch_max(current, Ordering::SeqCst);
            tokio::time::sleep(delay).await;
            in_flight.fetch_sub(1, Ordering::SeqCst);
            Ok(exchange)
        }
    }))
}

/// Producer that echoes the exchange after `delay`.
fn slow_processor(delay: Duration) -> BoxProcessor {
    BoxProcessor::new(ProcessorFn::new(move |exchange: Exchange| async move {
        tokio::time::sleep(delay).await;
        Ok(exchange)
    }))
}

/// Drives `count` concurrent invocations, one clone per invocation.
///
/// Each invocation runs ready-then-call on ITS OWN clone — the `run_steps`
/// clone-per-invocation pattern. Cloning resets a `ConcurrencyLimit` clone's
/// permit slot while sharing its semaphore, so this both exercises the cap and
/// is the regression lock for compile-probe finding P4 (an adapter that clones
/// inside `call` deadlocks permit-style layers).
///
/// Returns `(entry_instant, response)` per invocation, so callers can measure
/// elapse from the first exchange entering the layered service.
async fn drive_concurrent(processor: &BoxProcessor, count: usize) -> Vec<(Instant, Exchange)> {
    let mut tasks = JoinSet::new();
    for i in 0..count {
        let mut clone = processor.clone();
        tasks.spawn(async move {
            let entered = Instant::now();
            let exchange = Exchange::new(Message::new(format!("m{i}")));
            let ready = clone
                .ready()
                .await
                .map_err(|err| format!("invocation {i}: readiness failed: {err}"))?;
            let response = ready
                .call(exchange)
                .await
                .map_err(|err| format!("invocation {i}: call failed: {err}"))?;
            Ok::<_, String>((entered, response))
        });
    }

    let mut completed = Vec::with_capacity(count);
    loop {
        // allow-test-wait: bounded by tokio::time::timeout(WATCHDOG) at every drive_concurrent call site
        let Some(joined) = tasks.join_next().await else {
            break;
        };
        let outcome = joined.expect("invocation task panicked");
        completed.push(outcome.expect("every invocation must succeed"));
    }
    completed
}

/// Walks the public `source()` chain (three hops max) looking for the Tower
/// timeout marker, exactly as a consumer would after the `From<BoxError>`
/// mapping: `CamelError::ProcessorErrorWithSource` delegates to
/// `OpaqueErrorSource`, which returns the boxed error directly from its own
/// `source()`.
fn source_chain_reaches_elapsed(err: &CamelError) -> bool {
    let mut current = std::error::Error::source(err);
    for _ in 0..3 {
        let Some(node) = current else { return false };
        if node
            .downcast_ref::<tower::timeout::error::Elapsed>()
            .is_some()
        {
            return true;
        }
        current = std::error::Error::source(node);
    }
    false
}

/// `RateLimit` state is not `Clone`; sharing it across processor clones needs
/// `tower::buffer::Buffer`, which spawns a worker on the current tokio runtime
/// (a documented runtime precondition — this battery always builds the wrap
/// inside `#[tokio::test]`).
struct BufferedRateLimit {
    rate: u64,
    period: Duration,
    bound: usize,
}

impl ProcessorLayer for BufferedRateLimit {
    fn wrap(&self, inner: BoxProcessor) -> Result<BoxProcessor, CamelError> {
        let rate_limited = RateLimitLayer::new(self.rate, self.period).layer(inner);
        let buffered = Buffer::new(rate_limited, self.bound);
        Ok(BoxProcessor::new(CamelErrorAdapter::new(buffered)))
    }
}

/// Why this test's shape matters (compile-probe finding P4, the hardness in
/// the doc comment): `CamelErrorAdapter` never clones inside `call`; each
/// invocation acquires its permit on its own clone and consumes it there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn probe_concurrency_cap_holds_under_100_concurrent() {
    const CAP: usize = 8;
    const BURST: usize = 100;

    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));
    let producer = counting_processor(
        Arc::clone(&in_flight),
        Arc::clone(&max_seen),
        Duration::from_millis(50),
    );

    let wrapped = LayerFactory::new(ConcurrencyLimitLayer::new(CAP))
        .wrap(producer)
        .expect("ConcurrencyLimitLayer satisfies the ProcessorLayer bounds");

    let completed = tokio::time::timeout(WATCHDOG, drive_concurrent(&wrapped, BURST))
        .await
        .expect("100 concurrent invocations must finish inside the 30s watchdog");
    assert_eq!(
        completed.len(),
        BURST,
        "every concurrent invocation must complete"
    );

    let max = max_seen.load(Ordering::SeqCst);
    let ratio = max as f64 / CAP as f64;
    println!("concurrency: max in-flight {max}/{CAP} (ratio {ratio:.3}, envelope 1.15)");
    assert!(max <= CAP, "concurrency cap violated: {max} > {CAP}");
    assert!(
        ratio <= 1.15,
        "concurrency ratio {ratio:.3} exceeds the 1.15 envelope"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn probe_timeout_envelope_no_early_fire() {
    const DEADLINE: Duration = Duration::from_millis(100);

    let wrapped = LayerFactory::new(TimeoutLayer::new(DEADLINE))
        .wrap(slow_processor(Duration::from_millis(500)))
        .expect("TimeoutLayer satisfies the ProcessorLayer bounds");

    // Clock starts when the exchange enters the wrapped processor.
    let mut handle = wrapped;
    let started = Instant::now();
    let ready = handle
        .ready()
        .await
        .expect("the timeout layer is ready immediately");
    let result = ready.call(Exchange::new(Message::new("timeout"))).await;
    let elapsed = started.elapsed();

    let err = result.expect_err("a 500ms producer must hit the 100ms timeout");
    assert!(
        source_chain_reaches_elapsed(&err),
        "source chain must reach tower::timeout::error::Elapsed: {err:?}"
    );
    assert!(
        err.to_string().contains("middleware timeout elapsed"),
        "expected a timeout-classified CamelError, got: {err}"
    );

    let ratio = elapsed.as_secs_f64() / DEADLINE.as_secs_f64();
    println!(
        "timeout: elapsed {elapsed:?} against deadline {DEADLINE:?} (ratio {ratio:.3}, envelope [1.0, 1.25])"
    );
    assert!(
        elapsed >= DEADLINE,
        "timeout fired early: {elapsed:?} < {DEADLINE:?}"
    );
    assert!(
        ratio <= 1.25,
        "timeout ratio {ratio:.3} exceeds the 1.25 envelope"
    );
}

/// Rate limit state is shared through `Buffer`: a per-clone budget would
/// finish the whole burst immediately and fail the `>= required` lower bound.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn probe_rate_limit_buffered_shares_state() {
    const RATE: u64 = 5;
    const PERIOD: Duration = Duration::from_secs(1);
    const BURST: usize = 17;
    // floor((17 - 1) / 5) * 1s — the ratio denominator per the blessed spec.
    const REQUIRED_DELAY: Duration = Duration::from_secs(3);

    let factory = LayerFactory::new(BufferedRateLimit {
        rate: RATE,
        period: PERIOD,
        bound: 16,
    });
    let wrapped = factory
        .wrap(slow_processor(Duration::from_millis(10)))
        .expect("the buffered rate limit satisfies the ProcessorLayer bounds");

    let completed = tokio::time::timeout(WATCHDOG, drive_concurrent(&wrapped, BURST))
        .await
        .expect("a 17-request burst must finish inside the 30s watchdog");
    assert_eq!(completed.len(), BURST, "every request must succeed");

    // Clock starts at the first request entering the layered service.
    let first_entry = completed
        .iter()
        .map(|(entered, _)| *entered)
        .min()
        .expect("the burst is non-empty");
    let elapsed = first_entry.elapsed();

    let ratio = elapsed.as_secs_f64() / REQUIRED_DELAY.as_secs_f64();
    println!(
        "rate limit: elapsed {elapsed:?} against required {REQUIRED_DELAY:?} (ratio {ratio:.3}, envelope [1.0, 1.25])"
    );
    assert!(
        elapsed >= REQUIRED_DELAY,
        "rate limit state NOT shared across clones: {elapsed:?} < {REQUIRED_DELAY:?}"
    );
    assert!(
        ratio <= 1.25,
        "rate-limit delay ratio {ratio:.3} exceeds the 1.25 envelope"
    );
}

/// Regression lock for compile-probe finding P4: 100 invocations through one
/// wrapped handle, each doing ready-then-call on its own clone, must complete
/// inside the finite deadline. A clone-inside-`call` adapter deadlocks here
/// because the original handle keeps its unconsumed permit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn probe_same_handle_readiness_completes_bounded() {
    const CAP: usize = 4;
    const BURST: usize = 100;

    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));
    let producer = counting_processor(
        Arc::clone(&in_flight),
        Arc::clone(&max_seen),
        Duration::from_millis(50),
    );

    let wrapped = LayerFactory::new(ConcurrencyLimitLayer::new(CAP))
        .wrap(producer)
        .expect("ConcurrencyLimitLayer satisfies the ProcessorLayer bounds");

    let completed = tokio::time::timeout(WATCHDOG, drive_concurrent(&wrapped, BURST))
        .await
        .expect("same-handle readiness must complete inside the 30s watchdog");
    assert_eq!(
        completed.len(),
        BURST,
        "every invocation must complete without a readiness deadlock"
    );

    let max = max_seen.load(Ordering::SeqCst);
    let ratio = max as f64 / CAP as f64;
    println!("same-handle readiness: max in-flight {max}/{CAP} (ratio {ratio:.3}, envelope 1.15)");
    assert!(
        max <= CAP,
        "same-handle concurrency cap violated: {max} > {CAP}"
    );
    assert!(
        ratio <= 1.15,
        "same-handle concurrency ratio {ratio:.3} exceeds the 1.15 envelope"
    );
}
