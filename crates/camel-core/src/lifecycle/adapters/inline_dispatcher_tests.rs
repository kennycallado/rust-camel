//! Tests for the inline dispatcher adapter (Tasks 2.2/3.3).
//! Sibling file via `#[path]` so the production module stays scannable;
//! still in-crate for private-field access.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Duration;

use camel_api::{
    BoxProcessor, BoxProcessorExt, CamelError, Message, RouteController, RuntimeCommand, Value,
};
use camel_component_api::{
    Component, ComponentContext, ConcurrencyModel, ConsumerContext, Endpoint, NoOpComponentContext,
    ProducerContext, RuntimeObservability,
};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use super::RouteInlineDispatcher;
use crate::lifecycle::adapters::pipeline_runtime::{
    SharedPipeline, new_shared_pipeline, swap_pipeline_raw,
};
use crate::lifecycle::adapters::route_controller::DefaultRouteController;
use crate::lifecycle::adapters::route_registry::DEFAULT_SHUTDOWN_TIMEOUT;
use crate::lifecycle::application::route_definition::{BuilderStep, RouteDefinition};
use crate::lifecycle::cohort_activation::CohortActivationGate;
use crate::shared::components::domain::Registry;
use camel_component_api::InlineRouteDispatcher;

// ------------------------------------------------------------------
// Probe pipeline harness
// ------------------------------------------------------------------

/// Behavior of the probe pipeline processor.
#[derive(Clone, Copy)]
enum ProbeMode {
    /// Complete immediately, tagging the exchange with `sink=<tag>`.
    Tag(&'static str),
    /// Complete immediately with `ProcessorError`.
    Fail,
    /// Signal entry, then park until the test releases this call's
    /// ordinal; then tag `sink=gated` and complete.
    Gated,
    /// Signal entry, then park forever (until the call future is
    /// dropped).
    ParkForever,
}

/// Drop guard observing that a parked call future was dropped.
struct DropProbe(Arc<AtomicBool>);
impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[derive(Default)]
struct ProbeCore {
    entries: std::sync::Mutex<Vec<String>>,
    active: AtomicUsize,
    max_active: AtomicUsize,
    next_ordinal: AtomicUsize,
}

/// Signaling channels and shared core for one probe processor — usable
/// both inside a `SharedPipeline` (unit tests) and as a
/// `BuilderStep::Processor` inside a real controller-driven route
/// (Task 3.3 integration tests).
struct ProbeParts {
    core: Arc<ProbeCore>,
    entered_rx: mpsc::UnboundedReceiver<String>,
    release_tx: tokio::sync::watch::Sender<u32>,
    dropped: Arc<AtomicBool>,
}

impl ProbeParts {
    fn entries(&self) -> Vec<String> {
        self.core
            .entries
            .lock()
            .expect("probe entries lock")
            .clone()
    }

    fn release(&self, up_to: u32) {
        self.release_tx.send(up_to).expect("release channel alive");
    }

    async fn await_entry(&mut self) -> String {
        timeout(Duration::from_secs(2), self.entered_rx.recv())
            .await
            .expect("entry signal within 2s")
            .expect("entered channel alive")
    }
}

/// Build the probe processor for `mode` plus its signaling parts.
fn probe_processor(mode: ProbeMode) -> (BoxProcessor, ProbeParts) {
    let core = Arc::new(ProbeCore::default());
    let (entered_tx, entered_rx) = mpsc::unbounded_channel();
    let (release_tx, release_rx) = tokio::sync::watch::channel(0u32);
    let dropped = Arc::new(AtomicBool::new(false));
    let processor = BoxProcessor::from_fn({
        let core = Arc::clone(&core);
        let entered_tx = entered_tx.clone();
        let release_rx = release_rx.clone();
        let dropped = Arc::clone(&dropped);
        move |mut exchange: camel_api::Exchange| {
            let core = Arc::clone(&core);
            let entered_tx = entered_tx.clone();
            let mut release_rx = release_rx.clone();
            let dropped = Arc::clone(&dropped);
            async move {
                let tag = exchange.input.body.as_text().unwrap_or("?").to_string();
                core.entries
                    .lock()
                    .expect("probe entries lock")
                    .push(tag.clone());
                let active = core
                    .active
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    + 1;
                core.max_active
                    .fetch_max(active, std::sync::atomic::Ordering::SeqCst);
                let _ = entered_tx.send(tag);
                match mode {
                    ProbeMode::Tag(t) => {
                        core.active
                            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                        exchange.set_property("sink", t);
                        Ok(exchange)
                    }
                    ProbeMode::Fail => {
                        core.active
                            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                        Err(CamelError::ProcessorError("planned probe failure".into()))
                    }
                    ProbeMode::Gated => {
                        let ordinal = core
                            .next_ordinal
                            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                            as u32
                            + 1;
                        release_rx
                            .wait_for(|v| *v >= ordinal)
                            .await
                            .expect("release channel alive");
                        core.active
                            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                        exchange.set_property("sink", "gated");
                        Ok(exchange)
                    }
                    ProbeMode::ParkForever => {
                        let _probe = DropProbe(Arc::clone(&dropped));
                        std::future::pending::<()>().await;
                        unreachable!("ParkForever future never completes")
                    }
                }
            }
        }
    });
    (
        processor,
        ProbeParts {
            core,
            entered_rx,
            release_tx,
            dropped,
        },
    )
}

struct ProbeHarness {
    pipeline: SharedPipeline,
    parts: ProbeParts,
}

impl ProbeHarness {
    fn new(mode: ProbeMode) -> Self {
        let (processor, parts) = probe_processor(mode);
        let pipeline = new_shared_pipeline(processor);
        Self { pipeline, parts }
    }

    fn entries(&self) -> Vec<String> {
        self.parts.entries()
    }

    fn max_active(&self) -> usize {
        self.parts
            .core
            .max_active
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    fn release(&self, up_to: u32) {
        self.parts.release(up_to)
    }

    async fn await_entry(&mut self) -> String {
        self.parts.await_entry().await
    }
}

fn test_exchange(tag: &str) -> camel_api::Exchange {
    camel_api::Exchange::new(Message::new(tag))
}

/// A dispatcher over `pipeline` with its own cancel token, drain counter,
/// and an OPEN cohort gate (single-boot, mid-flight conditions).
fn open_dispatcher(
    pipeline: SharedPipeline,
) -> (
    Arc<RouteInlineDispatcher>,
    CancellationToken,
    Arc<std::sync::atomic::AtomicU64>,
) {
    let cancel = CancellationToken::new();
    let drain = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let gate = Arc::new(CohortActivationGate::new_closed());
    gate.open();
    let dispatcher = Arc::new(RouteInlineDispatcher::new(
        "probe-route".to_string(),
        pipeline,
        cancel.clone(),
        Arc::clone(&drain),
        gate,
    ));
    (dispatcher, cancel, drain)
}

fn as_capability(d: &Arc<RouteInlineDispatcher>) -> Arc<dyn InlineRouteDispatcher> {
    Arc::clone(d) as Arc<dyn InlineRouteDispatcher>
}

// ------------------------------------------------------------------
// Dispatcher behavior tests
// ------------------------------------------------------------------

#[tokio::test]
async fn dispatch_holds_snapshot_through_completion() {
    let mut harness = ProbeHarness::new(ProbeMode::Gated);
    let (dispatcher, _cancel, _drain) = open_dispatcher(harness.pipeline.clone());
    let cap = as_capability(&dispatcher);

    // First dispatch parks inside the OLD snapshot.
    let first = tokio::spawn(cap.dispatch(test_exchange("1")));
    assert_eq!(harness.await_entry().await, "1");

    // Swap the pipeline source mid-dispatch.
    swap_pipeline_raw(
        &harness.pipeline,
        BoxProcessor::from_fn(|mut ex: camel_api::Exchange| async move {
            ex.set_property("sink", "new");
            Ok(ex)
        }),
        vec![],
    );

    // Release the parked call: it completes against the OLD snapshot.
    harness.release(1);
    let done = timeout(Duration::from_secs(2), first)
        .await
        .expect("first dispatch completes within 2s")
        .expect("task join");
    assert_eq!(
        done.expect("old snapshot completes the in-flight call")
            .property("sink")
            .and_then(|v| v.as_str()),
        Some("gated")
    );

    // The NEXT dispatch picks up the new snapshot.
    let second = cap.dispatch(test_exchange("2")).await;
    assert_eq!(
        second
            .expect("new snapshot dispatch")
            .property("sink")
            .and_then(|v| v.as_str()),
        Some("new")
    );
    // The old snapshot executed exactly the in-flight call; the swapped
    // pipeline (a separate processor, not wired to this harness) handled
    // the second.
    assert_eq!(harness.entries(), vec!["1".to_string()]);
}

#[tokio::test]
async fn dispatch_decrements_in_flight_exactly_once() {
    // Success path.
    let harness = ProbeHarness::new(ProbeMode::Tag("ok"));
    let (dispatcher, _cancel, drain) = open_dispatcher(harness.pipeline.clone());
    let cap = as_capability(&dispatcher);
    let baseline = drain.load(std::sync::atomic::Ordering::SeqCst);

    let _ = cap.dispatch(test_exchange("a")).await;
    assert_eq!(
        drain.load(std::sync::atomic::Ordering::SeqCst),
        baseline,
        "drain counter restored after success"
    );

    // Error path: processor returns Err.
    let err_harness = ProbeHarness::new(ProbeMode::Fail);
    let (err_dispatcher, _c, err_drain) = open_dispatcher(err_harness.pipeline.clone());
    let err_cap = as_capability(&err_dispatcher);
    let err_baseline = err_drain.load(std::sync::atomic::Ordering::SeqCst);

    let result = err_cap.dispatch(test_exchange("b")).await;
    assert!(result.is_err(), "failing pipeline must surface the error");
    assert_eq!(
        err_drain.load(std::sync::atomic::Ordering::SeqCst),
        err_baseline,
        "drain counter restored after error"
    );
}

#[tokio::test]
async fn dispatch_serializes_concurrent_callers_fifo() {
    let mut harness = ProbeHarness::new(ProbeMode::Gated);
    let (dispatcher, _cancel, _drain) = open_dispatcher(harness.pipeline.clone());
    let cap = as_capability(&dispatcher);

    // Caller A enters the pipeline first and parks there, holding the
    // admission permit.
    let a = tokio::spawn(cap.dispatch(test_exchange("A")));
    assert_eq!(harness.await_entry().await, "A");

    // Caller B queues behind A on the admission mutex: never enters the
    // pipeline while A is in flight.
    let b = tokio::spawn(cap.dispatch(test_exchange("B")));
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(harness.entries(), vec!["A".to_string()], "B must wait");

    // Complete A, then B enters and completes — in call order.
    harness.release(1);
    let a_out = timeout(Duration::from_secs(2), a)
        .await
        .expect("A completes")
        .expect("A join")
        .expect("A ok");
    assert_eq!(
        a_out.property("sink").and_then(|v| v.as_str()),
        Some("gated")
    );
    assert_eq!(harness.await_entry().await, "B");
    harness.release(2);
    let _b_out = timeout(Duration::from_secs(2), b)
        .await
        .expect("B completes")
        .expect("B join")
        .expect("B ok");

    assert_eq!(
        harness.entries(),
        vec!["A".to_string(), "B".to_string()],
        "executions complete in call order"
    );
    assert_eq!(harness.max_active(), 1, "executions must not interleave");
}

#[tokio::test]
async fn dispatch_parks_on_startup_cohort() {
    let harness = ProbeHarness::new(ProbeMode::Tag("ok"));
    let cancel = CancellationToken::new();
    let drain = Arc::new(std::sync::atomic::AtomicU64::new(0));
    // Closed gate: the startup cohort has not completed.
    let gate = Arc::new(CohortActivationGate::new_closed());
    let dispatcher = Arc::new(RouteInlineDispatcher::new(
        "probe-route".to_string(),
        harness.pipeline.clone(),
        cancel.clone(),
        Arc::clone(&drain),
        Arc::clone(&gate),
    ));
    let cap = as_capability(&dispatcher);

    let parked = tokio::spawn(cap.dispatch(test_exchange("1")));
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        harness.entries().is_empty(),
        "dispatch must park while the cohort gate is closed"
    );
    assert!(!parked.is_finished());

    // Cohort completes — the dispatch then executes.
    gate.open();
    let done = timeout(Duration::from_secs(2), parked)
        .await
        .expect("dispatch completes after cohort opens")
        .expect("task join")
        .expect("dispatch ok");
    assert_eq!(done.property("sink").and_then(|v| v.as_str()), Some("ok"));
    assert_eq!(harness.entries(), vec!["1".to_string()]);
}

#[tokio::test]
async fn dispatch_yields_every_32_hops() {
    let harness = ProbeHarness::new(ProbeMode::Tag("ok"));
    let (dispatcher, _cancel, _drain) = open_dispatcher(harness.pipeline.clone());
    let cap = as_capability(&dispatcher);

    for i in 0..100 {
        let _ = cap.dispatch(test_exchange(&format!("hop-{i}"))).await;
    }

    assert!(
        dispatcher.hop_budget_for_test() >= 100,
        "hop budget counts every completed dispatch, got {}",
        dispatcher.hop_budget_for_test()
    );
    assert!(
        dispatcher.yields_for_test() >= 3,
        "yield site must fire at least every 32 hops (100 hops → ≥3), got {}",
        dispatcher.yields_for_test()
    );
}

#[tokio::test]
async fn dispatch_consumer_cancel_during_admission_returns_consumer_stopping() {
    let harness = ProbeHarness::new(ProbeMode::Tag("ok"));
    let (dispatcher, cancel, drain) = open_dispatcher(harness.pipeline.clone());
    let baseline = drain.load(std::sync::atomic::Ordering::SeqCst);
    let cap = as_capability(&dispatcher);

    // Externally hold the admission permit.
    let guard = dispatcher.admission_for_test().lock().await;

    let parked = tokio::spawn(cap.dispatch(test_exchange("1")));
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        harness.entries().is_empty(),
        "dispatch must stay blocked on admission"
    );

    cancel.cancel();
    let result = timeout(Duration::from_secs(1), parked)
        .await
        .expect("cancel resolves within 1s")
        .expect("task join");
    assert!(
        matches!(result, Err(CamelError::ConsumerStopping)),
        "expected ConsumerStopping, got {result:?}"
    );
    assert!(harness.entries().is_empty(), "pipeline never entered");
    assert_eq!(
        drain.load(std::sync::atomic::Ordering::SeqCst),
        baseline,
        "in-flight counter restored"
    );
    drop(guard);
}

#[tokio::test]
async fn dispatch_consumer_cancel_during_execution_returns_consumer_stopping() {
    let mut harness = ProbeHarness::new(ProbeMode::ParkForever);
    let (dispatcher, cancel, drain) = open_dispatcher(harness.pipeline.clone());
    let baseline = drain.load(std::sync::atomic::Ordering::SeqCst);
    let cap = as_capability(&dispatcher);

    let parked = tokio::spawn(cap.dispatch(test_exchange("1")));
    assert_eq!(harness.await_entry().await, "1");

    cancel.cancel();
    let result = timeout(Duration::from_secs(1), parked)
        .await
        .expect("cancel resolves within 1s")
        .expect("task join");
    assert!(
        matches!(result, Err(CamelError::ConsumerStopping)),
        "expected ConsumerStopping, got {result:?}"
    );
    assert!(
        harness
            .parts
            .dropped
            .load(std::sync::atomic::Ordering::SeqCst),
        "operation future (pipeline call) must be dropped"
    );
    assert!(
        dispatcher.admission_for_test().try_lock().is_ok(),
        "admission permit must be released"
    );
    assert_eq!(
        drain.load(std::sync::atomic::Ordering::SeqCst),
        baseline,
        "in-flight counter restored"
    );
}

// ------------------------------------------------------------------
// Publication tests (controller harness)
// ------------------------------------------------------------------

/// Captured consumer contexts, one per consumer boot (start, restart,
/// resume each push a fresh entry) — the Task 3.3 tests compare
/// capabilities across boots.
type CapturedCtxs = Arc<std::sync::Mutex<Vec<ConsumerContext>>>;

struct ProbeComponent {
    captured: CapturedCtxs,
}

struct ProbeEndpoint {
    captured: CapturedCtxs,
}

struct ProbeConsumer {
    captured: CapturedCtxs,
}

impl Component for ProbeComponent {
    fn scheme(&self) -> &str {
        "probe"
    }
    fn create_endpoint(
        &self,
        _uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(ProbeEndpoint {
            captured: Arc::clone(&self.captured),
        }))
    }
}

impl Endpoint for ProbeEndpoint {
    fn uri(&self) -> &str {
        "probe"
    }
    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn camel_component_api::Consumer>, CamelError> {
        Ok(Box::new(ProbeConsumer {
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
impl camel_component_api::Consumer for ProbeConsumer {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        self.captured
            .lock()
            .expect("captured lock")
            .push(context.clone());
        // Immediate lifetime loop: park until the route stops.
        context.cancel_token().cancelled().await;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
}

/// Wait for the first captured context (the initial boot).
async fn await_captured(captured: &CapturedCtxs) -> ConsumerContext {
    await_nth_capture(captured, 0).await
}

/// Wait for the n-th (0-based) captured context — each consumer boot
/// (start, restart, resume) pushes exactly one entry.
async fn await_nth_capture(captured: &CapturedCtxs, n: usize) -> ConsumerContext {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        if let Some(ctx) = captured.lock().expect("captured lock").get(n).cloned() {
            return ctx;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("consumer context #{} was not captured within 2s", n);
}

fn probe_controller(captured: CapturedCtxs) -> DefaultRouteController {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    registry
        .lock()
        .expect("registry lock")
        .register(Arc::new(ProbeComponent { captured }));
    DefaultRouteController::new(
        registry,
        Arc::new(camel_api::NoopPlatformService::default()),
    )
}

/// Build a probe route whose single step is the probe processor, and
/// return the signaling parts alongside it.
fn probe_route_with_step(route_id: &str, mode: ProbeMode) -> (RouteDefinition, ProbeParts) {
    let (processor, parts) = probe_processor(mode);
    let route = RouteDefinition::new(
        "probe:src",
        vec![BuilderStep::Processor(camel_api::OpaqueProcessor(
            processor,
        ))],
    )
    .with_route_id(route_id.to_string());
    (route, parts)
}

/// Read the route's drain counter (the shared `drain_in_flight` the
/// dispatcher's DrainGuard increments).
fn drain_counter<'a>(
    controller: &'a DefaultRouteController,
    route_id: &str,
) -> &'a std::sync::atomic::AtomicU64 {
    &controller
        .routes
        .get(route_id)
        .unwrap_or_else(|| panic!("route {route_id} registered"))
        .drain_in_flight
}

/// Poll until the drain counter reaches `want` (bounded, panics past
/// the deadline).
async fn await_drain_count(counter: &std::sync::atomic::AtomicU64, want: u64) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while counter.load(std::sync::atomic::Ordering::SeqCst) != want {
        assert!(
            tokio::time::Instant::now() < deadline,
            "drain counter did not reach {want} within 1s"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn sequential_consumer_publishes_capability() {
    let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut controller = probe_controller(Arc::clone(&captured));

    let route = RouteDefinition::new("probe:src", vec![]).with_route_id("rt-probe-seq");
    controller.add_route(route).await.unwrap();
    controller.start_route("rt-probe-seq").await.unwrap();

    let ctx = await_captured(&captured).await;
    assert!(
        ctx.inline_dispatcher().is_some(),
        "Sequential topology must publish the inline dispatcher"
    );

    controller.stop_route("rt-probe-seq").await.unwrap();
}

#[tokio::test]
async fn concurrent_consumer_gets_no_capability() {
    let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut controller = probe_controller(Arc::clone(&captured));

    let route = RouteDefinition::new("probe:src", vec![])
        .with_route_id("rt-probe-conc")
        .with_concurrency(ConcurrencyModel::Concurrent { max: None });
    controller.add_route(route).await.unwrap();
    controller.start_route("rt-probe-conc").await.unwrap();

    let ctx = await_captured(&captured).await;
    assert!(
        ctx.inline_dispatcher().is_none(),
        "Concurrent topology must keep the capability None (channel path)"
    );

    controller.stop_route("rt-probe-conc").await.unwrap();
}

#[tokio::test]
async fn aggregate_route_never_publishes_capability() {
    let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut controller = probe_controller(Arc::clone(&captured));

    // force_completion_on_stop(true) is what materializes the aggregate
    // split (find_top_level_aggregate_requiring_split requires a timeout
    // or force-completion) — complete_when_size(10) alone compiles a
    // plain pipeline and never exercises the split topology.
    let agg_config = camel_api::AggregatorConfig::correlate_by("key")
        .complete_when_size(10)
        .force_completion_on_stop(true)
        .build()
        .unwrap();

    let route = RouteDefinition::new(
        "probe:src",
        vec![
            BuilderStep::DeclarativeSetHeader {
                key: "key".into(),
                value: camel_api::ValueSourceDef::Literal(Value::String("k1".into())),
            },
            BuilderStep::Aggregate { config: agg_config },
            BuilderStep::To("probe:sink".into()),
        ],
    )
    .with_route_id("rt-probe-agg");
    controller.add_route(route).await.unwrap();
    controller.start_route("rt-probe-agg").await.unwrap();

    // rc-2sba: a split route's `managed.pipeline` is an identity shell
    // (`compose_pipeline(vec![])`) and must never be exposed to inline
    // execution — no capability is published, so producers take the
    // channel path where the aggregate engine drives the split
    // pre/agg/post pipelines.
    let ctx = await_captured(&captured).await;
    assert!(
        ctx.inline_dispatcher().is_none(),
        "aggregate-split routes must never publish the inline dispatcher"
    );

    controller.stop_route("rt-probe-agg").await.unwrap();
}

#[tokio::test]
async fn aggregate_route_resume_never_publishes_capability() {
    let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut controller = probe_controller(Arc::clone(&captured));

    // Timeout-based split fixture: materializes the aggregate split like
    // force_completion_on_stop does, but keeps the pipeline plane alive
    // across suspend — a force-completion split tears the pipeline down
    // when the consumer exits (the aggregate force-completion monitor
    // cancels it), so the route reaches Stopped, not Suspended, and
    // resume_route would reject it. The guard under test only reads
    // `aggregate_split.is_some()`, which both fixtures set.
    let agg_config = camel_api::AggregatorConfig::correlate_by("key")
        .complete_on_timeout(Duration::from_secs(600))
        .build()
        .unwrap();

    let route = RouteDefinition::new(
        "probe:src",
        vec![
            BuilderStep::DeclarativeSetHeader {
                key: "key".into(),
                value: camel_api::ValueSourceDef::Literal(Value::String("k1".into())),
            },
            BuilderStep::Aggregate { config: agg_config },
            BuilderStep::To("probe:sink".into()),
        ],
    )
    .with_route_id("rt-probe-agg-resume");
    controller.add_route(route).await.unwrap();
    controller.start_route("rt-probe-agg-resume").await.unwrap();

    let ctx1 = await_nth_capture(&captured, 0).await;
    assert!(
        ctx1.inline_dispatcher().is_none(),
        "aggregate-split routes must never publish the inline dispatcher"
    );

    // The resume publication site mirrors start — the split topology
    // must stay channel-dispatched across the suspend/resume window too.
    controller
        .suspend_route("rt-probe-agg-resume")
        .await
        .unwrap();
    controller
        .resume_route("rt-probe-agg-resume")
        .await
        .unwrap();

    let ctx2 = await_nth_capture(&captured, 1).await;
    assert!(
        ctx2.inline_dispatcher().is_none(),
        "resumed aggregate-split routes must not republish the capability"
    );

    controller.stop_route("rt-probe-agg-resume").await.unwrap();
}

// ------------------------------------------------------------------
// Task 3.3: real-route integration — eligibility, cancellation,
// restart, resume (route_controller harness)
// ------------------------------------------------------------------

#[tokio::test]
async fn inline_consumer_stop_yields_consumer_stopping() {
    let captured: CapturedCtxs = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut controller = probe_controller(Arc::clone(&captured));
    let (route, mut harness) = probe_route_with_step("rt-inline-stop", ProbeMode::ParkForever);
    controller.add_route(route).await.unwrap();
    controller.start_route("rt-inline-stop").await.unwrap();
    controller.activate_cohort();

    let ctx = await_captured(&captured).await;
    let dispatcher = ctx
        .inline_dispatcher()
        .expect("Sequential route publishes the capability");

    // Inline dispatch parks inside the route's pipeline processor and
    // holds the shared drain counter above baseline.
    let parked = tokio::spawn(dispatcher.dispatch(test_exchange("1")));
    assert_eq!(harness.await_entry().await, "1");
    assert_eq!(
        drain_counter(&controller, "rt-inline-stop").load(std::sync::atomic::Ordering::SeqCst),
        1,
        "parked inline dispatch holds the drain counter"
    );

    // Stop the consumer route: the consumer cancels + joins quickly,
    // then the drain grace must elapse (in-flight > 0 blocks the drain
    // wait until the shutdown deadline) before the pipeline token is
    // cancelled and the parked dispatch fails.
    let stop_start = tokio::time::Instant::now();
    controller.stop_route("rt-inline-stop").await.unwrap();
    let stop_elapsed = stop_start.elapsed();

    let result = timeout(Duration::from_secs(1), parked)
        .await
        .expect("dispatch resolves after the pipeline token cancel")
        .expect("task join");
    assert!(
        matches!(result, Err(CamelError::ConsumerStopping)),
        "expected ConsumerStopping, got {result:?}"
    );
    // The drain wait runs against an absolute deadline: stop cannot
    // have returned meaningfully earlier than the full grace budget.
    assert!(
        stop_elapsed >= DEFAULT_SHUTDOWN_TIMEOUT - Duration::from_millis(500),
        "stop must wait out the drain grace before cancelling (took {stop_elapsed:?})"
    );
    assert!(
        harness.dropped.load(std::sync::atomic::Ordering::SeqCst),
        "the pipeline call future must be dropped by the cancel scope"
    );
}

#[tokio::test]
async fn inline_producer_cancel_keeps_route_alive() {
    let captured: CapturedCtxs = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut controller = probe_controller(Arc::clone(&captured));
    let (route, mut harness) = probe_route_with_step("rt-inline-cancel", ProbeMode::Gated);
    controller.add_route(route).await.unwrap();
    controller.start_route("rt-inline-cancel").await.unwrap();
    controller.activate_cohort();

    let ctx = await_captured(&captured).await;
    let dispatcher = ctx
        .inline_dispatcher()
        .expect("Sequential route publishes the capability");

    // Producer task dispatches and parks inside the gated processor.
    let producer = tokio::spawn(dispatcher.dispatch(test_exchange("1")));
    assert_eq!(harness.await_entry().await, "1");
    assert_eq!(
        drain_counter(&controller, "rt-inline-cancel").load(std::sync::atomic::Ordering::SeqCst),
        1,
        "dispatch in flight"
    );

    // Cancel the producer task mid-dispatch: dropping the dispatch
    // future must NOT touch the consumer token, and the DrainGuard
    // decrement fires exactly once.
    producer.abort();
    await_drain_count(drain_counter(&controller, "rt-inline-cancel"), 0).await;

    // The route keeps running: a fresh dispatch completes end-to-end
    // through the same (still-alive) pipeline.
    let second = tokio::spawn(dispatcher.dispatch(test_exchange("2")));
    assert_eq!(harness.await_entry().await, "2");
    harness.release(2);
    let out = timeout(Duration::from_secs(2), second)
        .await
        .expect("second dispatch completes")
        .expect("task join")
        .expect("route still dispatches after producer cancel");
    assert_eq!(out.property("sink").and_then(|v| v.as_str()), Some("gated"));
    // The aborted first call never completed; its exchange is gone.
    assert_eq!(harness.entries(), vec!["1".to_string(), "2".to_string()]);

    controller.stop_route("rt-inline-cancel").await.unwrap();
}

#[tokio::test]
async fn inline_restart_fresh_cancellation_state() {
    let captured: CapturedCtxs = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut controller = probe_controller(Arc::clone(&captured));
    let (route, _harness) = probe_route_with_step("rt-inline-restart", ProbeMode::Tag("marker"));
    controller.add_route(route).await.unwrap();
    controller.start_route("rt-inline-restart").await.unwrap();
    controller.activate_cohort();

    let ctx1 = await_nth_capture(&captured, 0).await;
    let old = ctx1
        .inline_dispatcher()
        .expect("Sequential route publishes the capability");
    // Pre-stop proof of inline selection: the published dispatcher
    // completes a dispatch against the marker pipeline.
    let pre = old
        .dispatch(test_exchange("pre"))
        .await
        .expect("pre-stop inline dispatch");
    assert_eq!(
        pre.property("sink").and_then(|v| v.as_str()),
        Some("marker")
    );

    // Full stop, then start: fresh consumer, fresh tokens, fresh drain
    // counter (stop_route_internal recreates all three).
    controller.stop_route("rt-inline-restart").await.unwrap();
    controller.start_route("rt-inline-restart").await.unwrap();

    let ctx2 = await_nth_capture(&captured, 1).await;
    let new = ctx2
        .inline_dispatcher()
        .expect("restart republishes the capability");
    assert!(
        !Arc::ptr_eq(&old, &new),
        "a fresh boot must publish a fresh dispatcher"
    );

    // The OLD dispatcher's pipeline-cancel scope died with the previous
    // boot — every dispatch through it is ConsumerStopping.
    let stale = timeout(Duration::from_secs(1), old.dispatch(test_exchange("stale")))
        .await
        .expect("stale dispatch resolves immediately");
    assert!(
        matches!(stale, Err(CamelError::ConsumerStopping)),
        "expected ConsumerStopping from the old dispatcher, got {stale:?}"
    );

    // The NEW dispatcher starts from zero in-flight, completes a marker
    // dispatch, and returns to zero.
    let counter = drain_counter(&controller, "rt-inline-restart");
    assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 0);
    let out = new
        .dispatch(test_exchange("post"))
        .await
        .expect("post-restart inline dispatch");
    assert_eq!(
        out.property("sink").and_then(|v| v.as_str()),
        Some("marker")
    );
    assert_eq!(
        counter.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "in-flight returns to baseline after the new dispatch"
    );

    controller.stop_route("rt-inline-restart").await.unwrap();
}

#[tokio::test]
async fn inline_error_taxonomy_matches_channel() {
    // Inline path: Sequential topology publishes the capability.
    let captured_inline: CapturedCtxs = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut controller = probe_controller(Arc::clone(&captured_inline));
    let (route, _harness) = probe_route_with_step("rt-tax-inline", ProbeMode::Fail);
    controller.add_route(route).await.unwrap();
    controller.start_route("rt-tax-inline").await.unwrap();
    controller.activate_cohort();

    let ctx_inline = await_captured(&captured_inline).await;
    let dispatcher = ctx_inline
        .inline_dispatcher()
        .expect("Sequential route publishes the capability");
    let inline_err = dispatcher
        .dispatch(test_exchange("inline"))
        .await
        .expect_err("failing pipeline must surface the error");

    // Channel path: Concurrent { max: Some(1) } strips the capability
    // (eligibility: real-controller shape with a bounded max), so the
    // dispatch must go through ctx.send_and_wait (the envelope path).
    let captured_channel: CapturedCtxs = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut controller_channel = probe_controller(Arc::clone(&captured_channel));
    let (channel_processor, _parts) = probe_processor(ProbeMode::Fail);
    let route_channel = RouteDefinition::new(
        "probe:src",
        vec![BuilderStep::Processor(camel_api::OpaqueProcessor(
            channel_processor,
        ))],
    )
    .with_route_id("rt-tax-channel")
    .with_concurrency(ConcurrencyModel::Concurrent { max: Some(1) });
    controller_channel.add_route(route_channel).await.unwrap();
    controller_channel
        .start_route("rt-tax-channel")
        .await
        .unwrap();
    controller_channel.activate_cohort();

    let ctx_channel = await_captured(&captured_channel).await;
    assert!(
        ctx_channel.inline_dispatcher().is_none(),
        "Concurrent {{ max: Some(1) }} must keep the capability None (channel path)"
    );
    let channel_err = timeout(
        Duration::from_secs(2),
        ctx_channel.send_and_wait(test_exchange("channel")),
    )
    .await
    .expect("channel reply within 2s")
    .expect_err("failing pipeline must surface the error");

    // b′ ownership: same CamelError variant for the same processor
    // failure on both paths — no new taxonomy for the inline topology.
    assert!(
        matches!(&inline_err, CamelError::ProcessorError(_)),
        "inline path must surface ProcessorError, got {inline_err:?}"
    );
    assert!(
        matches!(&channel_err, CamelError::ProcessorError(_)),
        "channel path must surface ProcessorError, got {channel_err:?}"
    );
    assert_eq!(
        std::mem::discriminant(&inline_err),
        std::mem::discriminant(&channel_err),
        "identical error variants for the same processor failure"
    );

    controller.stop_route("rt-tax-inline").await.unwrap();
    controller_channel
        .stop_route("rt-tax-channel")
        .await
        .unwrap();
}

#[tokio::test]
async fn inline_resume_republishes_capability() {
    // Controller side (definitive): suspend closes the consumer plane
    // (entry closed, pipeline plane alive); resume spawns a fresh
    // consumer whose fresh context must carry a republished capability
    // — otherwise the resumed registry entry silently falls back to
    // the channel path (bd rc-y4vk).
    let captured: CapturedCtxs = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut controller = probe_controller(Arc::clone(&captured));
    let (route, _harness) = probe_route_with_step("rt-inline-resume", ProbeMode::Tag("resumed"));
    controller.add_route(route).await.unwrap();
    controller.start_route("rt-inline-resume").await.unwrap();
    controller.activate_cohort();

    let ctx1 = await_nth_capture(&captured, 0).await;
    assert!(
        ctx1.inline_dispatcher().is_some(),
        "pre-suspend context carries the capability"
    );

    controller.suspend_route("rt-inline-resume").await.unwrap();
    controller.resume_route("rt-inline-resume").await.unwrap();

    let ctx2 = await_nth_capture(&captured, 1).await;
    let dispatcher = ctx2
        .inline_dispatcher()
        .expect("resume must republish the capability on the fresh context");
    // The suspended pipeline plane stayed alive — the resumed
    // dispatcher completes a real dispatch end-to-end.
    let out = dispatcher
        .dispatch(test_exchange("after-resume"))
        .await
        .expect("post-resume inline dispatch");
    assert_eq!(
        out.property("sink").and_then(|v| v.as_str()),
        Some("resumed")
    );

    controller.stop_route("rt-inline-resume").await.unwrap();

    // DirectComponent shape through the public CamelContext API: the
    // camel-direct consumer copies ctx.inline_dispatcher() into its
    // registry entry at startup (Task 2.3), so a resumed Sequential
    // `direct:` route re-registers an entry carrying the fresh
    // dispatcher (proven Some above) — and post-resume producer
    // dispatches flow through the re-registered entry.
    let mut dctx = crate::CamelContext::builder().build().await.unwrap();
    dctx.register_component(camel_component_direct::DirectComponent::new());
    dctx.add_route_definition(
        RouteDefinition::new("direct:resume", vec![]).with_route_id("rt-direct-resume"),
    )
    .await
    .unwrap();
    dctx.start().await.unwrap();

    let handle = dctx.runtime();
    handle
        .execute(RuntimeCommand::SuspendRoute {
            route_id: "rt-direct-resume".into(),
            command_id: "c-suspend".into(),
            causation_id: None,
        })
        .await
        .unwrap();
    handle
        .execute(RuntimeCommand::ResumeRoute {
            route_id: "rt-direct-resume".into(),
            command_id: "c-resume".into(),
            causation_id: Some("c-suspend".into()),
        })
        .await
        .unwrap();

    let component = dctx.registry().get("direct").unwrap();
    let endpoint = component.create_endpoint("direct:resume", &dctx).unwrap();
    let producer = endpoint
        .create_producer(Arc::new(NoOpComponentContext), &dctx.producer_context())
        .unwrap();
    let out = producer
        .clone()
        .oneshot(test_exchange("hop"))
        .await
        .expect("post-resume dispatch through the re-registered entry");
    assert!(
        out.property("sink").is_none(),
        "empty pipeline: no marker expected"
    );
}

#[tokio::test]
async fn inline_stopped_consumer_keeps_no_consumer_semantics() {
    let mut ctx = crate::CamelContext::builder().build().await.unwrap();
    ctx.register_component(camel_component_direct::DirectComponent::new());
    ctx.add_route_definition(
        RouteDefinition::new("direct:gone", vec![]).with_route_id("rt-direct-gone"),
    )
    .await
    .unwrap();
    ctx.start().await.unwrap();

    let component = ctx.registry().get("direct").unwrap();
    let endpoint = component.create_endpoint("direct:gone", &ctx).unwrap();
    let producer = endpoint
        .create_producer(Arc::new(NoOpComponentContext), &ctx.producer_context())
        .unwrap();

    // Sanity: the route runs with the inline capability published
    // (Sequential default) and dispatches fine.
    producer
        .clone()
        .oneshot(test_exchange("warm"))
        .await
        .expect("pre-stop dispatch works");

    // Full stop takes the registry-cleanup path: the consumer exits and
    // removes its entry. A subsequent dispatch must surface the
    // existing no-consumer error (fail_if_no_consumers default) —
    // exactly as for a never-registered name, with no new variant.
    ctx.runtime()
        .execute(RuntimeCommand::StopRoute {
            route_id: "rt-direct-gone".into(),
            command_id: "c-stop".into(),
            causation_id: None,
        })
        .await
        .unwrap();

    let stopped_err = producer
        .clone()
        .oneshot(test_exchange("after-stop"))
        .await
        .expect_err("stopped route must fail dispatch");
    assert!(
        matches!(stopped_err, CamelError::EndpointCreationFailed(_)),
        "expected the no-consumer error after stop, got {stopped_err:?}"
    );

    let ghost_endpoint = component.create_endpoint("direct:never", &ctx).unwrap();
    let ghost = ghost_endpoint
        .create_producer(Arc::new(NoOpComponentContext), &ctx.producer_context())
        .unwrap();
    let ghost_err = ghost
        .clone()
        .oneshot(test_exchange("never"))
        .await
        .expect_err("never-registered name must fail dispatch");
    assert!(
        matches!(ghost_err, CamelError::EndpointCreationFailed(_)),
        "expected the no-consumer error for a never-registered name, got {ghost_err:?}"
    );
    assert_eq!(
        std::mem::discriminant(&stopped_err),
        std::mem::discriminant(&ghost_err),
        "stopped consumer keeps the no-consumer semantics (identical variant)"
    );
}
