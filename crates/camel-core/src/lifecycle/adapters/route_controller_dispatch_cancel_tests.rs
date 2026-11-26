//! Dispatch-cancellation tests for the route-controller pipeline entry
//! (dispatch-cancellation Tasks 3–5).
//!
//! Task 3 covers the carrier wiring (both spawn sites and the inline
//! dispatcher), Task 4 the retry bypass, and Task 5 the release proof plus the
//! route-stop/drain regressions.
//!
//! Sibling file via `#[path]` (same shape as the drainclaim suite): the
//! controller, its `ConsumerContext` capture harness, and the task-local
//! trigger under test are all in-crate internals.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};
use tokio_util::sync::CancellationToken;

use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{
    BoxProcessor, BoxProcessorExt, CamelError, Exchange, Message, OpaqueProcessor, RouteController,
};
use camel_component_api::consumer::ExchangeEnvelope;
use camel_component_api::{
    Component, ConcurrencyModel, Consumer, ConsumerContext, Endpoint, ProducerContext,
    RuntimeObservability,
};

use crate::lifecycle::adapters::route_compiler::DISPATCH_CANCEL;
use crate::lifecycle::application::route_definition::{BuilderStep, RouteDefinition};
use crate::shared::components::domain::Registry;

use super::DefaultRouteController;

/// ConsumerContext clones captured per consumer boot (same shape as the
/// drainclaim harness).
type CapturedCtxs = Arc<Mutex<Vec<ConsumerContext>>>;

struct CaptureComponent {
    captured: CapturedCtxs,
}

struct CaptureEndpoint {
    captured: CapturedCtxs,
}

struct CaptureConsumer {
    captured: CapturedCtxs,
}

impl Component for CaptureComponent {
    fn scheme(&self) -> &str {
        "capture"
    }
    fn create_endpoint(
        &self,
        _uri: &str,
        _ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(CaptureEndpoint {
            captured: Arc::clone(&self.captured),
        }))
    }
}

impl Endpoint for CaptureEndpoint {
    fn uri(&self) -> &str {
        "capture"
    }
    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(CaptureConsumer {
            captured: Arc::clone(&self.captured),
        }))
    }
    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(camel_api::IdentityProcessor))
    }
}

#[async_trait::async_trait]
impl Consumer for CaptureConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        self.captured
            .lock()
            .expect("captured lock")
            .push(ctx.clone());
        ctx.cancel_token().cancelled().await;
        Ok(())
    }
    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
}

/// Gated probe parts: entry signal, entry count, park selector, release barrier.
struct GatedParts {
    entered_rx: mpsc::UnboundedReceiver<u32>,
    entered: Arc<AtomicU32>,
    park_ordinal: Arc<AtomicU32>,
    release_tx: tokio::sync::watch::Sender<u32>,
}

/// Pipeline processor that signals entry, then parks on the release barrier
/// only for the invocation ordinal named by `park_ordinal` (the test writes
/// the ordinal it wants parked; later invocations pass through).
fn gated_step(first_park_ordinal: u32) -> (BoxProcessor, GatedParts) {
    let (entered_tx, entered_rx) = mpsc::unbounded_channel::<u32>();
    let (release_tx, release_rx) = tokio::sync::watch::channel(0u32);
    let ordinal = Arc::new(AtomicU32::new(0));
    let entered = Arc::new(AtomicU32::new(0));
    let park_ordinal = Arc::new(AtomicU32::new(first_park_ordinal));
    let park_selector = Arc::clone(&park_ordinal);
    let entered_counter = Arc::clone(&entered);
    let processor = BoxProcessor::from_fn(move |mut ex: Exchange| {
        let entered_tx = entered_tx.clone();
        let mut release_rx = release_rx.clone();
        let ordinal = Arc::clone(&ordinal);
        let entered = Arc::clone(&entered_counter);
        let park_selector = Arc::clone(&park_selector);
        async move {
            let mine = ordinal.fetch_add(1, Ordering::SeqCst) + 1;
            entered.fetch_add(1, Ordering::SeqCst);
            let _ = entered_tx.send(mine);
            if park_selector.load(Ordering::SeqCst) == mine {
                release_rx
                    .wait_for(|v| *v >= mine)
                    .await
                    .expect("release channel alive");
            }
            ex.set_property("sink", "gated");
            Ok(ex)
        }
    });
    (
        processor,
        GatedParts {
            entered_rx,
            entered,
            park_ordinal,
            release_tx,
        },
    )
}

/// Pipeline processor that counts its invocation and forwards the exchange.
fn recording_step(counter: Arc<AtomicU32>) -> BoxProcessor {
    BoxProcessor::from_fn(move |ex: Exchange| {
        let counter = Arc::clone(&counter);
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(ex)
        }
    })
}

/// Pipeline processor that increments `entries` on entry, signals the test,
/// and fails with `ProcessorError` — the dispatch-cancel retry hook must
/// keep the entry count at exactly 1 (no redelivery after the abort).
fn failing_step(entries: Arc<AtomicU32>) -> (BoxProcessor, mpsc::UnboundedReceiver<u32>) {
    let (entered_tx, entered_rx) = mpsc::unbounded_channel::<u32>();
    let processor = BoxProcessor::from_fn(move |_ex: Exchange| {
        let entered_tx = entered_tx.clone();
        let entries = Arc::clone(&entries);
        async move {
            let mine = entries.fetch_add(1, Ordering::SeqCst) + 1;
            let _ = entered_tx.send(mine);
            Err(CamelError::ProcessorError("retry me".to_string()))
        }
    });
    (processor, entered_rx)
}

fn controller(captured: CapturedCtxs) -> DefaultRouteController {
    let registry = Arc::new(Mutex::new(Registry::new()));
    registry
        .lock()
        .expect("registry lock")
        .register(Arc::new(CaptureComponent { captured }));
    DefaultRouteController::new(
        registry,
        Arc::new(camel_api::NoopPlatformService::default()),
    )
}

async fn await_capture(captured: &CapturedCtxs) -> ConsumerContext {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        if let Some(ctx) = captured.lock().expect("captured lock").first().cloned() {
            return ctx;
        }
        tokio::time::sleep_until(tokio::time::Instant::now() + Duration::from_millis(10)).await;
    }
    panic!("consumer context was not captured within 2s");
}

/// Poll until the controller's global in-flight gauge reaches `want`
/// (bounded; panics past the deadline).
async fn await_total(counter: &camel_api::InFlightGauge, want: u64) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while counter.total() != want {
        assert!(
            tokio::time::Instant::now() < deadline,
            "global in-flight counter did not reach {want} within 2s (now {})",
            counter.total()
        );
        tokio::time::sleep_until(tokio::time::Instant::now() + Duration::from_millis(10)).await;
    }
}

fn test_exchange(tag: &str) -> Exchange {
    Exchange::new(Message::new(tag))
}

/// Start a two-step `capture:` route (step 1 gated, step 2 recording) with an
/// optional concurrency override, open the startup cohort, and return the
/// controller plus the captured consumer context.
///
/// `gated_step(1)` is the parking selector for both topologies used here: only
/// the first invocation of step 1 (the first exchange) parks; every later
/// invocation — the release test's survivor — passes straight through.
async fn start_two_step_route(
    captured: &CapturedCtxs,
    route_id: &str,
    from_uri: &str,
    step1: BoxProcessor,
    step2: BoxProcessor,
    concurrency: Option<ConcurrencyModel>,
) -> (DefaultRouteController, ConsumerContext) {
    let mut controller = controller(Arc::clone(captured));
    let mut route = RouteDefinition::new(
        from_uri,
        vec![
            BuilderStep::Processor(OpaqueProcessor(step1)),
            BuilderStep::Processor(OpaqueProcessor(step2)),
        ],
    )
    .with_route_id(route_id);
    if let Some(model) = concurrency {
        route = route.with_concurrency(model);
    }
    controller.add_route(route).await.expect("route add");
    controller.start_route(route_id).await.expect("route start");
    controller.activate_cohort();
    let ctx = await_capture(captured).await;
    (controller, ctx)
}

#[tokio::test]
async fn cancelled_dispatch_stops_before_the_next_step() {
    let captured: CapturedCtxs = Arc::new(Mutex::new(Vec::new()));
    let mut controller = controller(Arc::clone(&captured));
    let (gated, mut parts) = gated_step(1);
    let step2 = Arc::new(AtomicU32::new(0));
    let route = RouteDefinition::new(
        "capture:src",
        vec![
            BuilderStep::Processor(OpaqueProcessor(gated)),
            BuilderStep::Processor(OpaqueProcessor(recording_step(Arc::clone(&step2)))),
        ],
    )
    .with_route_id("rt-dispatch-cancel-next-step");
    controller.add_route(route).await.unwrap();
    controller
        .start_route("rt-dispatch-cancel-next-step")
        .await
        .unwrap();
    controller.activate_cohort();

    let ctx = await_capture(&captured).await;

    // Leg A (real trigger): abandon the waiter mid-flight. The timeout-bounded
    // join resolves only after the wait future — and its dispatch guard — is
    // dropped, which cancels the stamped token.
    let waiter = tokio::spawn({
        let ctx = ctx.clone();
        async move { ctx.send_and_wait(test_exchange("leg-a")).await }
    });
    timeout(Duration::from_secs(2), parts.entered_rx.recv())
        .await
        .expect("step 1 entry within 2s")
        .expect("entry channel alive");
    waiter.abort();
    let _ = timeout(Duration::from_secs(2), waiter).await;
    parts.release_tx.send(1).expect("release channel alive");

    await_total(&controller.in_flight_total, 0).await;
    assert_eq!(
        step2.load(Ordering::SeqCst),
        0,
        "a dispatch-cancelled pipeline must stop before the next step"
    );
    assert_eq!(
        parts.entered.load(Ordering::SeqCst),
        1,
        "step 1 must have been entered exactly once in leg A"
    );

    // Leg B (observable outcome): a manually stamped envelope. Park the second
    // invocation so the test drops the guard while step 1 is in flight.
    parts.park_ordinal.store(2, Ordering::SeqCst);
    let mut ex = test_exchange("leg-b");
    let dispatch = ex.begin_dispatch_cancel();
    let (tx, reply_rx) = tokio::sync::oneshot::channel();
    let envelope = ExchangeEnvelope {
        exchange: ex,
        reply_tx: Some(tx),
        in_flight_claim: None,
    };
    let sender = ctx.sender();
    timeout(Duration::from_secs(2), sender.send(envelope))
        .await
        .expect("sender send within 2s")
        .expect("channel alive");
    timeout(Duration::from_secs(2), parts.entered_rx.recv())
        .await
        .expect("step 1 entry within 2s")
        .expect("entry channel alive");
    drop(dispatch);
    parts.release_tx.send(2).expect("release channel alive");

    let reply = timeout(Duration::from_secs(2), reply_rx)
        .await
        .expect("reply within 2s")
        .expect("reply channel alive");
    assert!(
        matches!(reply, Err(CamelError::ConsumerStopping)),
        "cancelled dispatch must surface ConsumerStopping, got {reply:?}"
    );
    assert_eq!(
        step2.load(Ordering::SeqCst),
        0,
        "step 2 must not run after the dispatch is cancelled"
    );
    await_total(&controller.in_flight_total, 0).await;

    controller
        .stop_route("rt-dispatch-cancel-next-step")
        .await
        .unwrap();
}

#[tokio::test]
async fn uncancelled_dispatch_is_unchanged() {
    let captured: CapturedCtxs = Arc::new(Mutex::new(Vec::new()));
    let mut controller = controller(Arc::clone(&captured));
    let step_a = Arc::new(AtomicU32::new(0));
    let step_b = Arc::new(AtomicU32::new(0));
    let route = RouteDefinition::new(
        "capture:src2",
        vec![
            BuilderStep::Processor(OpaqueProcessor(recording_step(Arc::clone(&step_a)))),
            BuilderStep::Processor(OpaqueProcessor(recording_step(Arc::clone(&step_b)))),
        ],
    )
    .with_route_id("rt-dispatch-unchanged");
    controller.add_route(route).await.unwrap();
    controller
        .start_route("rt-dispatch-unchanged")
        .await
        .unwrap();
    controller.activate_cohort();

    let ctx = await_capture(&captured).await;
    let reply = timeout(
        Duration::from_secs(2),
        ctx.send_and_wait(test_exchange("unchanged")),
    )
    .await
    .expect("reply within 2s")
    .expect("uncancelled dispatch must succeed");

    assert_eq!(step_a.load(Ordering::SeqCst), 1);
    assert_eq!(step_b.load(Ordering::SeqCst), 1);
    assert_eq!(
        reply.input.body.as_text(),
        Some("unchanged"),
        "reply body must equal the input body"
    );
    assert!(
        reply.dispatch_cancel().is_none(),
        "a completed reply must carry no dispatch marker"
    );
    await_total(&controller.in_flight_total, 0).await;

    controller
        .stop_route("rt-dispatch-unchanged")
        .await
        .unwrap();
}

#[tokio::test]
async fn inline_entry_without_marker_shadows_outer_dispatch_scope() {
    let captured: CapturedCtxs = Arc::new(Mutex::new(Vec::new()));
    let mut controller = controller(Arc::clone(&captured));
    let step_a = Arc::new(AtomicU32::new(0));
    let step_b = Arc::new(AtomicU32::new(0));
    let route = RouteDefinition::new(
        "capture:direct-src",
        vec![
            BuilderStep::Processor(OpaqueProcessor(recording_step(Arc::clone(&step_a)))),
            BuilderStep::Processor(OpaqueProcessor(recording_step(Arc::clone(&step_b)))),
        ],
    )
    .with_route_id("rt-dispatch-inline-shadow");
    controller.add_route(route).await.unwrap();
    controller
        .start_route("rt-dispatch-inline-shadow")
        .await
        .unwrap();
    controller.activate_cohort();

    let ctx = await_capture(&captured).await;
    let dispatcher = ctx.inline_dispatcher().expect("inline capability");

    let outer_token = CancellationToken::new();
    outer_token.cancel();
    let result = timeout(
        Duration::from_secs(2),
        DISPATCH_CANCEL.scope(Some(outer_token.clone()), async move {
            dispatcher.dispatch(test_exchange("inline")).await
        }),
    )
    .await
    .expect("inline dispatch within 2s");

    assert!(
        result.is_ok(),
        "inline entry must shadow the cancelled outer scope: {result:?}"
    );
    assert_eq!(
        step_a.load(Ordering::SeqCst),
        1,
        "step A must run once (an inherited scope would stop before step 1)"
    );
    assert_eq!(step_b.load(Ordering::SeqCst), 1, "step B must run once");

    controller
        .stop_route("rt-dispatch-inline-shadow")
        .await
        .unwrap();
}

#[tokio::test]
async fn dispatch_cancelled_stopping_bypasses_the_error_handler() {
    let captured: CapturedCtxs = Arc::new(Mutex::new(Vec::new()));
    let mut controller = controller(Arc::clone(&captured));
    let entries = Arc::new(AtomicU32::new(0));
    let (failing, mut entered_rx) = failing_step(Arc::clone(&entries));
    let route = RouteDefinition::new(
        "capture:src3",
        vec![BuilderStep::Processor(OpaqueProcessor(failing))],
    )
    .with_route_id("rt-dispatch-eh-bypass")
    .with_error_handler(
        ErrorHandlerConfig::log_only()
            .on_exception(|e| matches!(e, CamelError::ProcessorError(_)))
            .retry(3)
            .with_backoff(Duration::from_millis(200), 2.0, Duration::from_millis(400))
            .build(),
    );
    controller.add_route(route).await.unwrap();
    controller
        .start_route("rt-dispatch-eh-bypass")
        .await
        .unwrap();
    controller.activate_cohort();

    let ctx = await_capture(&captured).await;

    // Spawn the dispatch, then abort + join the waiter while the error
    // handler is in its first backoff sleep: dropping the wait future drops
    // its dispatch guard, which cancels the stamped token.
    let waiter = tokio::spawn({
        let ctx = ctx.clone();
        async move { ctx.send_and_wait(test_exchange("eh-bypass")).await }
    });
    timeout(Duration::from_secs(2), entered_rx.recv())
        .await
        .expect("step entry within 2s")
        .expect("entry channel alive");
    waiter.abort();
    let _ = timeout(Duration::from_secs(2), waiter).await;

    // Wait past the backoff delay so the retry loop wakes on the cancelled
    // side of its sleep and the pre-invoke hook skips the redelivery.
    tokio::time::sleep_until(tokio::time::Instant::now() + Duration::from_millis(500)).await;

    assert_eq!(
        entries.load(Ordering::SeqCst),
        1,
        "the pre-invoke hook must stop the redelivery (step entered exactly once)"
    );
    await_total(&controller.in_flight_total, 0).await;
    timeout(
        Duration::from_secs(2),
        controller.stop_route("rt-dispatch-eh-bypass"),
    )
    .await
    .expect("route stop within 2s")
    .expect("route stop succeeds");
}

#[tokio::test]
async fn cancelled_dispatch_releases_the_exchange() {
    const ROUTE: &str = "rt-dispatch-release";
    let captured: CapturedCtxs = Arc::new(Mutex::new(Vec::new()));
    let (gated, mut parts) = gated_step(1);
    let step2 = Arc::new(AtomicU32::new(0));
    let (mut controller, ctx) = start_two_step_route(
        &captured,
        ROUTE,
        "capture:release-src",
        gated,
        recording_step(Arc::clone(&step2)),
        Some(ConcurrencyModel::Concurrent { max: Some(1) }),
    )
    .await;

    // Park the only concurrent slot in step 1, then abandon the wait. The
    // timeout-bounded join resolves only after the wait future — and its
    // dispatch guard — is dropped, so the stamped token is cancelled while the
    // pipeline is still in flight.
    let waiter = tokio::spawn({
        let ctx = ctx.clone();
        async move { ctx.send_and_wait(test_exchange("abandoned")).await }
    });
    timeout(Duration::from_secs(2), parts.entered_rx.recv())
        .await
        .expect("step 1 entry within 2s")
        .expect("entry channel alive");
    assert!(
        controller.in_flight_total.total() > 0,
        "the parked exchange must be counted in flight"
    );
    waiter.abort();
    let _ = timeout(Duration::from_secs(2), waiter)
        .await
        .expect("aborted waiter joins within 2s");

    // The release lets the cancelled pipeline exit; the claim, the semaphore
    // permit and the slot must all come back. No reply is asserted for the
    // abandoned waiter — it was aborted.
    parts.release_tx.send(1).expect("release channel alive");

    let reply = timeout(
        Duration::from_secs(5),
        ctx.send_and_wait(test_exchange("survivor")),
    )
    .await
    .expect("the released concurrent slot must accept an exchange within 5s")
    .expect("the survivor dispatch must complete");
    assert!(
        reply.dispatch_cancel().is_none(),
        "a completed reply must carry no dispatch marker"
    );
    assert_eq!(
        reply.input.body.as_text(),
        Some("survivor"),
        "the survivor reply must equal its input body"
    );
    assert_eq!(
        step2.load(Ordering::SeqCst),
        1,
        "only the survivor may reach step 2 — the abandoned dispatch was cancelled"
    );
    await_total(&controller.in_flight_total, 0).await;

    timeout(Duration::from_secs(2), controller.stop_route(ROUTE))
        .await
        .expect("route stop within 2s")
        .expect("route stop succeeds");
}

#[tokio::test]
async fn route_stop_still_cancels_in_flight_dispatch() {
    const ROUTE: &str = "rt-dispatch-stop";
    let captured: CapturedCtxs = Arc::new(Mutex::new(Vec::new()));
    let (gated, mut parts) = gated_step(1);
    let step2 = Arc::new(AtomicU32::new(0));
    let (mut controller, ctx) = start_two_step_route(
        &captured,
        ROUTE,
        "capture:stop-src",
        gated,
        recording_step(Arc::clone(&step2)),
        Some(ConcurrencyModel::Concurrent { max: Some(1) }),
    )
    .await;

    // The waiter stays live: this test pins that route stop — not the
    // dispatch guard — is what cuts the in-flight exchange.
    let waiter = tokio::spawn({
        let ctx = ctx.clone();
        async move { ctx.send_and_wait(test_exchange("stopping")).await }
    });
    timeout(Duration::from_secs(2), parts.entered_rx.recv())
        .await
        .expect("step 1 entry within 2s")
        .expect("entry channel alive");

    let route_token = controller
        .routes
        .get(ROUTE)
        .expect("route")
        .pipeline_cancel_token
        .clone();
    let stop = tokio::spawn(async move {
        controller
            .stop_route_with_budget(ROUTE, Duration::from_millis(100))
            .await
    });

    // The 100 ms drain budget expires while step 1 is parked; the stop path
    // then fires the pipeline token. Awaiting it proves the stop path ran
    // without guessing past the budget.
    timeout(Duration::from_secs(2), route_token.cancelled())
        .await
        .expect("route stop must fire the pipeline token within 2s");
    parts.release_tx.send(1).expect("release channel alive");

    let stopped = timeout(Duration::from_secs(5), stop)
        .await
        .expect("route stop within 5s")
        .expect("stop task joins");
    assert!(stopped.is_ok(), "route stop must succeed: {stopped:?}");

    let reply = timeout(Duration::from_secs(5), waiter)
        .await
        .expect("the in-flight waiter must resolve within 5s")
        .expect("waiter task joins");
    assert!(
        matches!(reply, Err(CamelError::ConsumerStopping)),
        "route stop must surface ConsumerStopping, got {reply:?}"
    );
    assert_eq!(
        step2.load(Ordering::SeqCst),
        0,
        "the pipeline boundary check must stop the stopped exchange before step 2"
    );
    assert_eq!(
        parts.entered.load(Ordering::SeqCst),
        1,
        "step 1 must have been entered exactly once"
    );
}

#[tokio::test]
async fn abandoned_wait_during_route_stop_drains_to_completion() {
    const ROUTE: &str = "rt-dispatch-drain";
    let captured: CapturedCtxs = Arc::new(Mutex::new(Vec::new()));
    let (gated, mut parts) = gated_step(1);
    let step2 = Arc::new(AtomicU32::new(0));
    let (mut controller, ctx) = start_two_step_route(
        &captured,
        ROUTE,
        "capture:drain-src",
        gated,
        recording_step(Arc::clone(&step2)),
        None,
    )
    .await;

    let waiter = tokio::spawn({
        let ctx = ctx.clone();
        async move { ctx.send_and_wait(test_exchange("drain")).await }
    });
    timeout(Duration::from_secs(2), parts.entered_rx.recv())
        .await
        .expect("step 1 entry within 2s")
        .expect("entry channel alive");

    // A generous budget keeps the drain grace active: the stop path waits for
    // the drain counter instead of cutting the exchange.
    let stop = tokio::spawn(async move {
        controller
            .stop_route_with_budget(ROUTE, Duration::from_secs(2))
            .await
    });

    // The stop path cancels the consumer-lifetime token first, which is also
    // the abandoned waiter's drain owner: the guard must defuse on drop rather
    // than cancel the dispatch.
    timeout(Duration::from_secs(2), ctx.cancel_token().cancelled())
        .await
        .expect("the stop path must cancel the consumer-lifetime token within 2s");

    // Bounded well under the 2 s stop budget: the join must resolve while the
    // drain grace is still open, so "abandoned, then drained" is ordered, not
    // inferred from two equal clocks.
    waiter.abort();
    let _ = timeout(Duration::from_millis(500), waiter)
        .await
        .expect("aborted waiter joins within 500ms");

    parts.release_tx.send(1).expect("release channel alive");

    let stopped = timeout(Duration::from_secs(5), stop)
        .await
        .expect("route stop within 5s")
        .expect("stop task joins");
    assert!(stopped.is_ok(), "route stop must succeed: {stopped:?}");
    assert_eq!(
        step2.load(Ordering::SeqCst),
        1,
        "an abandoned wait during route stop must drain to completion (step 2 runs)"
    );
    assert_eq!(
        parts.entered.load(Ordering::SeqCst),
        1,
        "step 1 must have been entered exactly once"
    );
}
