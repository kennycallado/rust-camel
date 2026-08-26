//! F8 cohort-activation regression (rc-ava7 discovery, rc-jxkj barrier,
//! rc-iuuk amended hold).
//!
//! Two routes boot as one startup cohort: A (Immediate, one deterministic
//! first emission whose pipeline step stops sibling B) and B (Explicit
//! startup handshake). Without the cohort activation barrier, A's first
//! exchange dispatches while the cohort is still mid-start and its
//! StopRoute(B) reaches the bus pre-validation with B's aggregate at
//! `Starting` — the deterministic `invalid transition: Starting -> Stopped`
//! rejection (application/commands.rs pre-validation +
//! domain/route_runtime.rs state machine; the same rejection the rc-slvd
//! task 2.2 deviation note in route_controller_tests.rs documented).
//!
//! - Positive: the first dispatch parks on the cohort gate until
//!   `start_context` activates the cohort (B committed `Started`), so the
//!   recorded StopRoute is Ok and B reaches Stopped.
//! - Negative control: the gate is force-opened mid-hold to simulate the
//!   ungated dispatch; the recorded StopRoute is Err of the
//!   "invalid transition" class.
//!
//! In-crate rationale: the harness needs `RouteControllerHandle::cohort_gate`
//! (cfg(test), pub(crate)) and `RuntimeExecutionHandle::execute_runtime_command`
//! (pub(crate)) — neither is exported, so the regression lives inside
//! camel-core beside route_controller_tests.rs.
//!
//! Why the hold is B's EXPLICIT startup handshake (rc-iuuk): the earlier
//! sync-hook hold std-blocked a pooled worker on the controller actor task
//! and the multi-thread scheduler nondeterministically starved sibling
//! actor-spawned tasks (~40-50% flake). Parking inside
//! `await_consumer_startup` is an ASYNC park — the actor yields, A's
//! consumer and drain tasks keep being polled, and the boot order stays
//! realistic (A's StartRoute completes first, then B's parks the actor
//! mid-cohort).
//!
//! A's ingress latch is consumer-side (amended tasks.md step 2 sanctions a
//! test-owned consumer because the component registry mints the timer's
//! consumer): the gate parks dispatch between the drain's envelope recv
//! and the pipeline run, so a pipeline-step latch could only fire
//! post-activation and the positive test's await-then-assert flow would be
//! vacuous. `EmitOnce` is the deterministic-first-tick Immediate consumer
//! the timer analogue cannot be — the component registry mints the timer's
//! consumer, so the emission latch must live in a consumer the test owns.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{oneshot, watch};
use tower::Service;

use camel_api::{
    BoxProcessor, CamelError, Exchange, Message, OpaqueProcessor, RuntimeCommand,
    RuntimeCommandResult,
};
use camel_component_api::{
    Component, ComponentContext, Consumer, ConsumerContext, ConsumerStartupMode, Endpoint,
    ProducerContext, RuntimeObservability,
};

use crate::context::{CamelContext, RuntimeExecutionHandle};
use crate::lifecycle::application::route_definition::{BuilderStep, RouteDefinition};

const ROUTE_A: &str = "cohort-regression-a";
const ROUTE_B: &str = "cohort-regression-b";

// ── Sibling B: test-controlled EXPLICIT consumer ──────────────────────────
//
// start() signals `entered` as its FIRST action (proves the actor reached
// B's start and Phase-1 already persisted `Starting`), parks on the
// test-owned `hold` watch, and calls ctx.mark_ready() ONLY after release —
// the controller actor parks ASYNCHRONOUSLY in await_consumer_startup for
// the whole hold (no sync-hook blocking, no emit_start_route_event).

struct HeldExplicitConsumer {
    entered_tx: watch::Sender<bool>,
    hold_rx: watch::Receiver<bool>,
}

#[async_trait]
impl Consumer for HeldExplicitConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        let _ = self.entered_tx.send(true);
        while !*self.hold_rx.borrow_and_update() {
            if self.hold_rx.changed().await.is_err() {
                break;
            }
        }
        ctx.mark_ready();
        ctx.cancelled().await;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }

    fn startup_mode(&self) -> ConsumerStartupMode {
        ConsumerStartupMode::Explicit
    }
}

struct HeldExplicitEndpoint {
    entered_tx: watch::Sender<bool>,
    hold_rx: watch::Receiver<bool>,
}

impl Endpoint for HeldExplicitEndpoint {
    fn uri(&self) -> &str {
        "heldexplicit:bind"
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(HeldExplicitConsumer {
            entered_tx: self.entered_tx.clone(),
            hold_rx: self.hold_rx.clone(),
        }))
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Err(CamelError::ProcessorError(
            "heldexplicit does not support producers".into(),
        ))
    }
}

struct HeldExplicitComponent {
    entered_tx: watch::Sender<bool>,
    hold_rx: watch::Receiver<bool>,
}

impl Component for HeldExplicitComponent {
    fn scheme(&self) -> &str {
        "heldexplicit"
    }

    fn create_endpoint(
        &self,
        _uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(HeldExplicitEndpoint {
            entered_tx: self.entered_tx.clone(),
            hold_rx: self.hold_rx.clone(),
        }))
    }
}

// ── Route A: Immediate consumer, deterministic first tick ─────────────────
//
// Emits exactly ONE exchange into the route channel (the ingress the
// regression rides on), latches `emitted` after the send completes — the
// consumer-side ingress latch — then serves its loop-style lifetime. The
// timer analogue cannot latch its own emission (the registry mints its
// consumer), which is why the latch lives in a consumer the test owns.

struct EmitOnceConsumer {
    emitted_tx: watch::Sender<bool>,
}

#[async_trait]
impl Consumer for EmitOnceConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        ctx.send(Exchange::new(Message::new("cohort-tick"))).await?;
        let _ = self.emitted_tx.send(true);
        ctx.cancelled().await;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
}

struct EmitOnceEndpoint {
    emitted_tx: watch::Sender<bool>,
}

impl Endpoint for EmitOnceEndpoint {
    fn uri(&self) -> &str {
        "emitonce:tick"
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(EmitOnceConsumer {
            emitted_tx: self.emitted_tx.clone(),
        }))
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Err(CamelError::ProcessorError(
            "emitonce does not support producers".into(),
        ))
    }
}

struct EmitOnceComponent {
    emitted_tx: watch::Sender<bool>,
}

impl Component for EmitOnceComponent {
    fn scheme(&self) -> &str {
        "emitonce"
    }

    fn create_endpoint(
        &self,
        _uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(EmitOnceEndpoint {
            emitted_tx: self.emitted_tx.clone(),
        }))
    }
}

// ── A's single pipeline step: StopRoute(B) with observation ────────────────
//
// Sets the dispatch-observation flag BEFORE the execute call, records the
// outcome in the shared slot, and passes the exchange through.

type StopRouteOutcome = Result<RuntimeCommandResult, CamelError>;

#[derive(Clone)]
struct StopRouteStep {
    exec: RuntimeExecutionHandle,
    target: String,
    dispatch_observed: Arc<AtomicBool>,
    result_slot: Arc<Mutex<Option<StopRouteOutcome>>>,
}

impl Service<Exchange> for StopRouteStep {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn std::future::Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut TaskContext<'_>) -> Poll<Result<(), CamelError>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let exec = self.exec.clone();
        let target = self.target.clone();
        let dispatch_observed = Arc::clone(&self.dispatch_observed);
        let result_slot = Arc::clone(&self.result_slot);
        Box::pin(async move {
            dispatch_observed.store(true, Ordering::SeqCst);
            let result = exec
                .execute_runtime_command(RuntimeCommand::StopRoute {
                    route_id: target,
                    command_id: "cohort-regression-stop-b".to_string(),
                    causation_id: None,
                })
                .await;
            *result_slot.lock().expect("result slot lock") = Some(result); // allow-unwrap
            Ok(exchange)
        })
    }
}

// ── Shared harness ─────────────────────────────────────────────────────────

/// Boot task: runs `ctx.start()` (parks while B's handshake is held),
/// reports the start result, then parks on `done` and stops the context
/// for cleanup — keeping ctx ownership out of the test body so the test
/// can drive its latches concurrently with the parked boot.
struct Boot {
    start_rx: oneshot::Receiver<Result<(), CamelError>>,
    done_tx: oneshot::Sender<()>,
    join: tokio::task::JoinHandle<()>,
}

fn spawn_boot(mut ctx: CamelContext) -> Boot {
    let (start_tx, start_rx) = oneshot::channel();
    let (done_tx, done_rx) = oneshot::channel();
    let join = tokio::spawn(async move {
        let result = ctx.start().await;
        let _ = start_tx.send(result);
        let _ = done_rx.await;
        let _ = ctx.stop().await;
    });
    Boot {
        start_rx,
        done_tx,
        join,
    }
}

struct CohortFixture {
    hold_tx: watch::Sender<bool>,
    entered_rx: watch::Receiver<bool>,
    emitted_rx: watch::Receiver<bool>,
    dispatch_observed: Arc<AtomicBool>,
    result_slot: Arc<Mutex<Option<StopRouteOutcome>>>,
    exec: RuntimeExecutionHandle,
    boot: Boot,
}

async fn boot_held_cohort() -> CohortFixture {
    let (hold_tx, hold_rx) = watch::channel(false);
    let (entered_tx, entered_rx) = watch::channel(false);
    let (emitted_tx, emitted_rx) = watch::channel(false);
    let dispatch_observed = Arc::new(AtomicBool::new(false));
    let result_slot: Arc<Mutex<Option<StopRouteOutcome>>> = Arc::new(Mutex::new(None));

    let mut ctx = CamelContext::builder()
        .build()
        .await
        .expect("build context"); // allow-unwrap
    ctx.register_component(HeldExplicitComponent {
        entered_tx,
        hold_rx,
    });
    ctx.register_component(EmitOnceComponent { emitted_tx });

    let exec = ctx.runtime_execution_handle();
    let stop_step = StopRouteStep {
        exec: exec.clone(),
        target: ROUTE_B.to_string(),
        dispatch_observed: Arc::clone(&dispatch_observed),
        result_slot: Arc::clone(&result_slot),
    };

    // Boot order: A (startup_order 0) starts before B (startup_order 1) —
    // A's consumer spawns first and its drain parks on the closed gate
    // while the actor later reaches B's start and parks in
    // await_consumer_startup.
    ctx.add_route_definition(
        RouteDefinition::new(
            "emitonce:tick",
            vec![BuilderStep::Processor(OpaqueProcessor(BoxProcessor::new(
                stop_step,
            )))],
        )
        .with_route_id(ROUTE_A)
        .with_startup_order(0),
    )
    .await
    .expect("add route A"); // allow-unwrap
    ctx.add_route_definition(
        RouteDefinition::new("heldexplicit:bind", vec![])
            .with_route_id(ROUTE_B)
            .with_startup_order(1),
    )
    .await
    .expect("add route B"); // allow-unwrap

    let boot = spawn_boot(ctx);
    CohortFixture {
        hold_tx,
        entered_rx,
        emitted_rx,
        dispatch_observed,
        result_slot,
        exec,
        boot,
    }
}

/// Await a `watch<bool>` latch with a generous deadline so absence asserts
/// downstream can never pass vacuously before the event fired.
async fn await_true(rx: &mut watch::Receiver<bool>, what: &'static str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !*rx.borrow_and_update() {
        tokio::time::timeout_at(deadline, rx.changed())
            .await
            .unwrap_or_else(|_| panic!("{what} must fire within 5s"))
            .unwrap_or_else(|_| panic!("watch sender must stay alive for {what}"));
    }
}

/// Await the step's recorded StopRoute outcome (5s deadline).
async fn await_recorded(slot: &Mutex<Option<StopRouteOutcome>>) -> StopRouteOutcome {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(outcome) = slot.lock().expect("result slot lock").take() {
            // allow-unwrap
            return outcome;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the step must record its StopRoute outcome within 5s"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn await_route_status(exec: &RuntimeExecutionHandle, route_id: &str, want: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let status = exec
            .runtime_route_status(route_id)
            .await
            .expect("route status query"); // allow-unwrap
        if status.as_deref() == Some(want) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "route {route_id} must reach {want} within 5s (now {status:?})"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// Positive: with B's handshake held, A's emitted first exchange is
/// received-but-parked by the closed cohort gate (no dispatch during the
/// hold); after release the cohort completes, the gate opens, the dispatch
/// runs, and the recorded StopRoute(B) is Ok — B reaches Stopped with no
/// invalid-transition rejection.
#[tokio::test(flavor = "multi_thread")]
async fn cohort_regression_parks_first_dispatch_until_cohort_completes() {
    let CohortFixture {
        hold_tx,
        mut entered_rx,
        mut emitted_rx,
        dispatch_observed,
        result_slot,
        exec,
        boot,
    } = boot_held_cohort().await;
    let Boot {
        start_rx,
        done_tx,
        join,
    } = boot;

    // Ingress latch (generous deadline): A's first exchange IS in the route
    // channel, and B's consumer is inside its held start() — the actor is
    // parked in await_consumer_startup with B's aggregate at Starting.
    await_true(&mut emitted_rx, "A must emit its first exchange").await;
    await_true(&mut entered_rx, "B must enter start()").await;

    // Grace window: the exchange must stay parked — dispatch observed?
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !dispatch_observed.load(Ordering::SeqCst),
        "first dispatch must park on the closed cohort gate while B's \
         handshake is held — the exchange dispatched during the cohort"
    );

    // Release B: mark_ready fires, start() returns, the cohort completes,
    // start_context activates the gate, and the parked dispatch proceeds.
    hold_tx.send_replace(true);

    let start_result = tokio::time::timeout(Duration::from_secs(5), start_rx)
        .await
        .expect("boot start report must arrive within 5s") // allow-unwrap
        .expect("boot task must deliver the start result"); // allow-unwrap
    start_result.expect("ctx.start() must return Ok once B's handshake is released"); // allow-unwrap

    // The dispatch runs and the recorded StopRoute(B) is Ok — no
    // invalid-transition: B committed Started before the gate opened.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !dispatch_observed.load(Ordering::SeqCst) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the parked dispatch must run within 5s of cohort activation"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    match await_recorded(&result_slot).await {
        Ok(_) => {}
        Err(e) => panic!(
            "StopRoute(B) after cohort completion must be Ok, got: {e} \
             (ungated dispatch rejection resurfaced)"
        ),
    }
    await_route_status(&exec, ROUTE_B, "Stopped").await;

    let _ = done_tx.send(());
    tokio::time::timeout(Duration::from_secs(5), join)
        .await
        .expect("boot task must join within 5s of the done signal") // allow-unwrap
        .expect("boot task must not panic"); // allow-unwrap
}

/// Negative control: the ungated simulation. B's handshake stays held, the
/// gate is opened DIRECTLY (shared Arc — bypasses the parked actor), and
/// A's exchange dispatches mid-hold. The recorded StopRoute(B) is Err of
/// the "invalid transition" class — B's aggregate sits at Starting during
/// the hold, so the bus pre-validation rejects the stop (application/
/// commands.rs pre-validation + domain/route_runtime.rs state machine).
#[tokio::test(flavor = "multi_thread")]
async fn cohort_regression_ungated_simulation_shows_the_rejection() {
    let CohortFixture {
        hold_tx,
        mut entered_rx,
        mut emitted_rx,
        result_slot,
        exec,
        boot,
        ..
    } = boot_held_cohort().await;
    let Boot {
        start_rx,
        done_tx,
        join,
    } = boot;

    await_true(&mut emitted_rx, "A must emit its first exchange").await;
    await_true(&mut entered_rx, "B must enter start()").await;

    // Force the gate open mid-hold: simulates the pre-barrier world where
    // the first dispatch was not parked for the cohort.
    exec.controller.cohort_gate().open();

    let recorded = await_recorded(&result_slot).await;
    match recorded {
        Err(e) => assert!(
            e.to_string().contains("invalid transition"),
            "ungated dispatch must reproduce the invalid-transition rejection \
             class (B is Starting during the hold), got: {e}"
        ),
        Ok(res) => panic!("ungated StopRoute(B) during the hold must be rejected, got Ok: {res:?}"),
    }

    // Cleanup: release B so the parked boot completes, then stop the context.
    hold_tx.send_replace(true);
    let start_result = tokio::time::timeout(Duration::from_secs(5), start_rx)
        .await
        .expect("ungated boot start report must arrive within 5s") // allow-unwrap
        .expect("boot task must deliver the start result"); // allow-unwrap
    start_result.expect("ctx.start() must return Ok once B's handshake is released"); // allow-unwrap
    let _ = done_tx.send(());
    tokio::time::timeout(Duration::from_secs(5), join)
        .await
        .expect("boot task must join within 5s of the done signal") // allow-unwrap
        .expect("boot task must not panic"); // allow-unwrap
}
