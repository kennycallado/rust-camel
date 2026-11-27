//! Route-level tests pinning the `direct-dispatch` concurrent-admission
//! requirements.
//!
//! `concurrent_consumer_parallel_admission` started as the red test for
//! "No producer-side serialization for Concurrent consumers":
//! one `DirectProducer` — shared by clones, exactly the one `to:` step shape —
//! dispatches two exchanges into a route declared `.concurrent(2)` whose
//! pipeline sleeps 200 ms. Pre-fix the producer's own semaphore serializes
//! the two calls, so only one pipeline is in flight and the overlap snapshot
//! is `(1, 0)`; post-fix both pipelines must be in flight before either
//! completes.
//!
//! A second test pins the channel-path timeout parity: the producer deadline
//! bounds admission wait too, and a caller that gives up cancels its dispatch
//! at the next step boundary (bd rc-29t24): a queued exchange never enters
//! the pipeline, an admitted one finishes its running step and stops.
//!
//! Runtime contract: the current-thread `#[tokio::test]` flavor is REQUIRED
//! by `with_time_control()` — a `multi_thread` flavor panics inside
//! `tokio::time::pause()`.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use camel_api::aggregator::AggregatorConfig;
use camel_api::{BoxProcessor, CamelError, Exchange, Message};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_direct::DirectComponent;
use camel_test::CamelTestContext;
use tokio::sync::Notify;

/// Resolve a ready producer for `uri` through the harness registry.
///
/// Mirrors the inner block of `send_await_reply`
/// (`crates/camel-test/tests/integration_test.rs:377-390`) without its
/// startup-race retry loop: the direct consumer registers before `start()`
/// returns, so the endpoint is dispatchable here.
async fn resolve_producer(h: &CamelTestContext, uri: &str) -> BoxProcessor {
    let ctx = h.ctx().lock().await; // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
    let producer_ctx = ctx.producer_context();
    let registry = ctx.registry();
    let component = registry
        .get("direct")
        .expect("direct component not registered");
    let endpoint = component
        .create_endpoint(uri, &*ctx)
        .expect("failed to create direct endpoint");
    endpoint
        .create_producer(
            Arc::new(camel_component_api::NoOpComponentContext),
            &producer_ctx,
        )
        .expect("failed to create direct producer")
}

#[tokio::test]
async fn concurrent_consumer_parallel_admission() {
    let (h, _tc) = CamelTestContext::builder()
        .with_component(DirectComponent::new())
        .with_mock()
        .with_time_control()
        .build()
        .await;

    // The direct consumer registers in the registry before `mark_ready()`, so
    // a completed `start()` implies a dispatchable entry — no real sleep.
    tokio::time::resume();

    let entered = Arc::new(AtomicUsize::new(0));
    let entered_step = Arc::clone(&entered);
    let route = RouteBuilder::from("direct:work")
        .route_id("admission-target")
        .concurrent(2)
        .process(move |ex: Exchange| {
            let entered = Arc::clone(&entered_step);
            async move {
                entered.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(200)).await; // allow-test-sleep: paused (virtual) clock — the 200 ms pipeline latency is the behaviour under test
                Ok(ex)
            }
        })
        .to("mock:done")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    // Measured phase: paused time turns both the pipeline sleeps and the
    // elapsed bound into virtual durations.
    tokio::time::pause();
    let t0 = tokio::time::Instant::now();

    let producer = resolve_producer(&h, "direct:work").await;
    let p1 = producer.clone();
    let p2 = producer.clone();

    let completed = Arc::new(AtomicUsize::new(0));
    let completed_a = Arc::clone(&completed);
    let completed_b = Arc::clone(&completed);

    // `oneshot` takes the service by value; the clones share the producer
    // state, which is what one `to:` step does.
    let a = tokio::spawn(async move {
        let result = tower::ServiceExt::oneshot(p1, Exchange::new(Message::new("p0"))).await;
        completed_a.fetch_add(1, Ordering::SeqCst);
        result
    });
    let b = tokio::spawn(async move {
        let result = tower::ServiceExt::oneshot(p2, Exchange::new(Message::new("p1"))).await;
        completed_b.fetch_add(1, Ordering::SeqCst);
        result
    });

    // Drive both spawned calls until the first parks on its 200 ms sleep.
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    let snap = (
        entered.load(Ordering::SeqCst),
        completed.load(Ordering::SeqCst),
    );

    // Join without manually advancing time: the paused clock auto-advances
    // through the pipeline sleeps while the runtime is idle on the join.
    let (ra, rb) = tokio::time::timeout(Duration::from_millis(500), async { (a.await, b.await) })
        .await
        .expect("both calls must complete within 500 ms virtual");

    let ra = ra.expect("p0 task panicked").expect("p0 call failed");
    let rb = rb.expect("p1 task panicked").expect("p1 call failed");
    assert_eq!(
        ra.input.body.as_text(),
        Some("p0"),
        "first reply must carry the original body"
    );
    assert_eq!(
        rb.input.body.as_text(),
        Some("p1"),
        "second reply must carry the original body"
    );

    h.mock()
        .get_endpoint("done")
        .unwrap()
        .assert_exchange_count(2)
        .await;

    assert_eq!(
        snap,
        (2, 0),
        "both 200 ms pipelines must be in flight and neither complete before either completes"
    );
    assert!(
        t0.elapsed() < Duration::from_millis(350),
        "parallel calls must complete under 350 ms virtual, got {:?}",
        t0.elapsed()
    );

    h.stop().await;
}

/// Poll until the context reports no in-flight exchange, so a queued dispatch
/// has actually been dequeued (and cancelled) before the assertions run.
async fn await_no_in_flight(h: &CamelTestContext) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let in_flight = {
            let ctx = h.ctx().lock().await; // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
            ctx.total_in_flight()
        };
        if in_flight == 0 {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "in-flight exchange did not settle within 2s (now {in_flight})"
        );
        tokio::task::yield_now().await;
    }
}

/// Snapshot of the pipeline trace, taken without holding the lock.
fn events_snapshot(events: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    events.lock().expect("events mutex poisoned").clone()
}

#[tokio::test]
async fn direct_timeout_still_counts_admission_wait() {
    let (h, _tc) = CamelTestContext::builder()
        .with_component(DirectComponent::new())
        .with_mock()
        .with_time_control()
        .build()
        .await;

    // The direct consumer registers in the registry before `mark_ready()`, so
    // a completed `start()` implies a dispatchable entry — no real sleep.
    tokio::time::resume();

    let entered = Arc::new(AtomicUsize::new(0));
    let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let release = Arc::new(Notify::new());

    let entered_step = Arc::clone(&entered);
    let events_step = Arc::clone(&events);
    let release_step = Arc::clone(&release);
    let route = RouteBuilder::from("direct:work")
        .route_id("timeout-target")
        .concurrent(1)
        .process(move |ex: Exchange| {
            let entered = Arc::clone(&entered_step);
            let events = Arc::clone(&events_step);
            let release = Arc::clone(&release_step);
            async move {
                entered.fetch_add(1, Ordering::SeqCst);
                let body = ex.input.body.as_text().unwrap_or_default().to_string();
                events
                    .lock()
                    .expect("events mutex poisoned")
                    .push(format!("begin:{body}"));
                if body == "p0" {
                    // Parked on purpose: this is the admission wait under test.
                    release.notified().await;
                }
                events
                    .lock()
                    .expect("events mutex poisoned")
                    .push(format!("end:{body}"));
                Ok(ex)
            }
        })
        .to("mock:done")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    // Measured phase: the producer deadline is a virtual duration.
    tokio::time::pause();

    // ONE producer: `timeout_ms=100` lives on its config, so every clone
    // shares the short deadline (the one-`to:`-step shape).
    let producer = resolve_producer(&h, "direct:work?timeout_ms=100").await;
    let p0 = producer.clone();
    let p1 = producer.clone();

    // p0's caller future: one poll enqueues the exchange and starts its
    // 100 ms deadline. It stays unpolled afterwards — this caller future is
    // what held the old shared producer permit pre-fix.
    let mut p0_call = p0.clone();
    let mut f0 = tower::Service::call(&mut p0_call, Exchange::new(Message::new("p0")));
    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);
    assert!(
        f0.as_mut().poll(&mut cx).is_pending(),
        "p0's first poll must be pending: it awaits the parked pipeline reply"
    );

    // Bounded yields only: paused time does not advance here, so p0's
    // deadline stays untouched while the drain task dequeues it.
    for _ in 0..50 {
        if events_snapshot(&events) == ["begin:p0"] {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        events_snapshot(&events),
        ["begin:p0"],
        "the drain task must dequeue p0 and park it in the pipeline"
    );

    // p1 resolves through its own 100 ms producer timeout; the 300 ms
    // wrapper only bounds the test.
    let mut p1_call = p1.clone();
    let p1_future = tower::Service::call(&mut p1_call, Exchange::new(Message::new("p1")));
    let p1_result = tokio::time::timeout(Duration::from_millis(300), p1_future)
        .await
        .expect("p1 must resolve within 300 ms virtual");
    assert!(
        matches!(&p1_result, Err(CamelError::ProcessorError(m)) if m == "direct:work call timed out"),
        "p1 must fail with the canonical direct timeout, got {p1_result:?}"
    );
    assert_eq!(
        events_snapshot(&events),
        ["begin:p0"],
        "p1 must not enter the pipeline while p0 holds the only Concurrent slot"
    );

    // The p0 caller deadline expired in the same virtual window, so p0 is
    // cancelled at its next step boundary: its running step still completes,
    // the following `to: mock` step does not run.
    let f0_result = tokio::time::timeout(Duration::from_millis(300), f0)
        .await
        .expect("p0 must resolve within 300 ms virtual once its deadline elapsed");
    assert!(
        matches!(&f0_result, Err(CamelError::ProcessorError(m)) if m == "direct:work call timed out"),
        "p0 must fail with the canonical direct timeout, got {f0_result:?}"
    );

    // Release the parked pipeline. p0 finishes the step it is parked in and
    // stops at the boundary; p1 never entered. Both callers abandoned their
    // wait, so neither reaches the following `to: mock` step.
    // Resume virtual time so the release propagates in real wall time.
    tokio::time::resume();
    release.notify_one();
    for _ in 0..200 {
        if events_snapshot(&events).len() >= 2 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        events_snapshot(&events),
        ["begin:p0", "end:p0"],
        "p0 finishes its running step; p1 never enters the pipeline"
    );
    // Both callers abandoned their wait, so neither exchange reaches the next
    // step: settle first so a queued p1 is provably dequeued and cancelled.
    await_no_in_flight(&h).await;
    h.mock()
        .get_endpoint("done")
        .unwrap()
        .assert_exchange_count(0)
        .await;

    h.stop().await;
}

/// The complement of the timeout scenario: a dispatch already inside a step
/// when its caller gives up finishes that step, then stops before the next
/// one. The cancellation is observed at the step boundary, never mid-step.
#[tokio::test]
async fn abandoned_dispatch_stops_before_the_next_step() {
    let (h, _tc) = CamelTestContext::builder()
        .with_component(DirectComponent::new())
        .with_mock()
        .with_time_control()
        .build()
        .await;

    tokio::time::resume();

    let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let release = Arc::new(Notify::new());

    let events_step = Arc::clone(&events);
    let release_step = Arc::clone(&release);
    let route = RouteBuilder::from("direct:solo")
        .route_id("admitted-timeout-target")
        .concurrent(1)
        .process(move |ex: Exchange| {
            let events = Arc::clone(&events_step);
            let release = Arc::clone(&release_step);
            async move {
                let body = ex.input.body.as_text().unwrap_or_default().to_string();
                events
                    .lock()
                    .expect("events mutex poisoned")
                    .push(format!("begin:{body}"));
                release.notified().await;
                events
                    .lock()
                    .expect("events mutex poisoned")
                    .push(format!("end:{body}"));
                Ok(ex)
            }
        })
        .to("mock:done")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::pause();

    let producer = resolve_producer(&h, "direct:solo?timeout_ms=100").await;
    let mut call = producer.clone();
    let mut future = tower::Service::call(&mut call, Exchange::new(Message::new("only")));
    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);
    assert!(
        future.as_mut().poll(&mut cx).is_pending(),
        "the first poll enqueues the exchange and awaits the parked reply"
    );
    for _ in 0..50 {
        if events_snapshot(&events) == ["begin:only"] {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        events_snapshot(&events),
        ["begin:only"],
        "the exchange must have entered the pipeline before the deadline elapses"
    );

    let result = tokio::time::timeout(Duration::from_millis(300), future)
        .await
        .expect("the caller must resolve within 300 ms virtual");
    assert!(
        matches!(&result, Err(CamelError::ProcessorError(m)) if m == "direct:solo call timed out"),
        "the caller fails with the canonical direct timeout, got {result:?}"
    );

    // Resume virtual time so the release propagates in real wall time.
    tokio::time::resume();
    release.notify_one();
    for _ in 0..200 {
        if events_snapshot(&events).len() >= 2 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        events_snapshot(&events),
        ["begin:only", "end:only"],
        "the running step finishes even though its caller gave up"
    );
    await_no_in_flight(&h).await;
    h.mock()
        .get_endpoint("done")
        .unwrap()
        .assert_exchange_count(0)
        .await;

    h.stop().await;
}

/// Pins the channel-path FIFO ordering for a Sequential consumer.
///
/// A top-level aggregate step whose completion carries a Timeout condition
/// compiles to the aggregate split: one forward loop dequeues envelopes and
/// runs the pre-pipeline to completion before submitting to the aggregator.
/// Three exchanges are enqueued in the poll order p0, p1, p2 — the explicit
/// poll order establishes channel arrival order. p0 parks the pre-stage
/// until released; p1/p2 must therefore stay pending at their first poll and
/// the pre-stage trace must be one-at-a-time, non-interleaved.
#[tokio::test]
async fn sequential_channel_consumer_keeps_fifo() {
    let (h, _tc) = CamelTestContext::builder()
        .with_component(DirectComponent::new())
        .with_mock()
        .with_time_control()
        .build()
        .await;

    // The direct consumer registers in the registry before `mark_ready()`, so
    // a completed `start()` implies a dispatchable entry — no real sleep.
    tokio::time::resume();

    let entered = Arc::new(AtomicUsize::new(0));
    let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let release = Arc::new(Notify::new());

    let entered_step = Arc::clone(&entered);
    let events_step = Arc::clone(&events);
    let release_step = Arc::clone(&release);

    // The timeout condition on the completion is what makes this a top-level
    // aggregate split; the pre-stage below runs serially in its forward loop.
    let route = RouteBuilder::from("direct:agg")
        .route_id("agg-split-target")
        .process(move |ex: Exchange| {
            let entered = Arc::clone(&entered_step);
            let events = Arc::clone(&events_step);
            let release = Arc::clone(&release_step);
            async move {
                entered.fetch_add(1, Ordering::SeqCst);
                let body = ex.input.body.as_text().unwrap_or_default().to_string();
                events
                    .lock()
                    .expect("events mutex poisoned")
                    .push(format!("begin:{body}"));
                if body == "p0" {
                    // Parked on purpose: this is the pre-stage serialization
                    // under test. p1/p2 cannot enter while p0 holds it.
                    release.notified().await;
                }
                events
                    .lock()
                    .expect("events mutex poisoned")
                    .push(format!("end:{body}"));
                Ok(ex)
            }
        })
        .aggregate(
            AggregatorConfig::correlate_by("id")
                .complete_on_timeout(Duration::from_millis(500))
                .build()
                .expect("valid aggregator config"),
        )
        .to("mock:agg-out")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    // Measured phase: the aggregator completion timeout is a virtual duration.
    tokio::time::pause();

    // ONE producer, three clones — the one-`to:`-step shape.
    let producer = resolve_producer(&h, "direct:agg").await;
    let correlated = |body: &'static str| {
        let mut ex = Exchange::new(Message::new(body));
        // The aggregator rejects exchanges without the correlation header.
        ex.input
            .headers
            .insert("id".to_string(), serde_json::json!("bucket-a"));
        ex
    };

    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);

    // p0 enqueues on its first poll; the forward loop then dequeues it and
    // parks it in the pre-stage. Bounded yields only: paused time does not
    // advance while the test task yields.
    let mut p0 = producer.clone();
    let mut f0 = tower::Service::call(&mut p0, correlated("p0"));
    assert!(
        f0.as_mut().poll(&mut cx).is_pending(),
        "p0's first poll must be pending: it awaits the parked pre-stage reply"
    );
    for _ in 0..50 {
        if events_snapshot(&events) == ["begin:p0"] {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        events_snapshot(&events),
        ["begin:p0"],
        "the forward loop must dequeue p0 and park it in the pre-stage"
    );

    // The explicit poll order p1 then p2 ESTABLISHES channel arrival order.
    // Both must stay pending: the single forward loop is still inside p0's
    // pre-stage, so neither can be admitted.
    let mut p1 = producer.clone();
    let mut f1 = tower::Service::call(&mut p1, correlated("p1"));
    assert!(
        f1.as_mut().poll(&mut cx).is_pending(),
        "p1's first poll must be pending: the forward loop is still in p0's pre-stage"
    );
    let mut p2 = producer.clone();
    let mut f2 = tower::Service::call(&mut p2, correlated("p2"));
    assert!(
        f2.as_mut().poll(&mut cx).is_pending(),
        "p2's first poll must be pending: the forward loop is still in p0's pre-stage"
    );

    // p0's park ends on the Notify, not on time; p1/p2 fall through as soon
    // as the forward loop admits them. The 500 ms wrapper bounds only
    // scheduler stalls.
    release.notify_one();
    let joined = tokio::time::timeout(Duration::from_millis(500), async {
        let r0 = f0.await;
        let r1 = f1.await;
        let r2 = f2.await;
        (r0, r1, r2)
    })
    .await;
    let (r0, r1, r2) = match joined {
        Ok(replies) => replies,
        Err(_) => panic!(
            "all three replies must resolve within 500 ms virtual; trace was {:?}",
            events_snapshot(&events)
        ),
    };
    for (label, reply) in [("p0", &r0), ("p1", &r1), ("p2", &r2)] {
        assert!(
            reply.is_ok(),
            "{label} reply must succeed, got {reply:?}; trace was {:?}",
            events_snapshot(&events)
        );
    }

    assert_eq!(
        events_snapshot(&events),
        [
            "begin:p0", "end:p0", "begin:p1", "end:p1", "begin:p2", "end:p2"
        ],
        "the pre-stage must execute one at a time in channel arrival order, no interleaving"
    );

    // Paused-time auto-advance drives the 500 ms aggregator completion
    // timeout while the wait polls idle: exactly one aggregated exchange.
    h.mock()
        .get_endpoint("agg-out")
        .unwrap()
        .await_exchanges(1, Duration::from_secs(1))
        .await;
    h.mock()
        .get_endpoint("agg-out")
        .unwrap()
        .assert_exchange_count(1)
        .await;

    h.stop().await;
}
