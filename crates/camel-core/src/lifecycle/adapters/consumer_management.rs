use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use camel_api::{CamelError, RuntimeCommand, RuntimeHandle, StepLifecycle, StepShutdownReason};
use camel_component_api::{
    ComponentContext, ConcurrencyModel, Consumer, ConsumerContext, ConsumerStartupMode,
    RuntimeObservability, StartupReceiver, StartupSignal,
};
use camel_endpoint::parse_uri;

use crate::lifecycle::adapters::route_helpers::{
    CrashNotification, ManagedRoute, fail_command_id, handle_is_running, publish_runtime_failure,
};
use crate::shared::components::domain::Registry;
use camel_processor::aggregator::AggregatorService;

pub(crate) fn create_route_consumer(
    rt: Arc<dyn RuntimeObservability>,
    registry: &Arc<std::sync::Mutex<Registry>>,
    from_uri: &str,
    component_ctx: &dyn ComponentContext,
) -> Result<(Box<dyn Consumer>, ConcurrencyModel), CamelError> {
    let parsed = parse_uri(from_uri)?;
    let component = {
        let guard = registry
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock"); // allow-unwrap
        guard.get_or_err(&parsed.scheme)?.clone()
    };
    let endpoint = component.create_endpoint(from_uri, component_ctx)?;
    let consumer = endpoint.create_consumer(rt)?;
    let concurrency = consumer.concurrency_model();
    Ok((consumer, concurrency))
}

/// Hard upper bound on how long the runtime waits for an `Explicit` consumer
/// to signal readiness (or failure) before treating route startup as failed.
///
/// This is defense-in-depth at the control-plane layer: an `Explicit` consumer
/// that returns `Ok` from `start()` but never calls `ctx.mark_ready()` — for
/// any reason (contract bug, a resource-bind step that never resolves, an
/// external dependency that never fires the readiness event) — would otherwise
/// hang `ctx.start()` / `h.start()` forever on `await_ready()`. Bounding the
/// await guarantees route startup ALWAYS terminates: either the consumer is
/// ready, or it fails fast with a clear startup error.
///
/// The budget is deliberately generous (90s) so a healthy consumer performing
/// legitimately slow binding — e.g. Kafka group coordination, whose
/// `session.timeout.ms` defaults to 45s and whose own assignment window is
/// `session_timeout_ms + 15s` (60s) — never trips it. Per-consumer bounds
/// (CXF `bridge_start_timeout` 5-30s, Kafka's ~60s assignment window) are sized
/// to fire FIRST and surface a precise component-specific error; this 90s net
/// is the last-resort backstop for consumers that lack (or lose) a local bound.
pub(crate) const CONSUMER_STARTUP_BUDGET: Duration = Duration::from_secs(90);

/// Grace period the detached failure watcher observes an Immediate
/// consumer's `start()` outcome before treating startup as complete
/// (rc-slvd).
///
/// Prompt `start()` errors (validation, double-start, ownership) come
/// back in microseconds of task time, so 50ms comfortably covers
/// scheduler jitter under CI load. Paid ONLY by the detached watcher —
/// no actor path waits for it.
pub(crate) const CONSUMER_IMMEDIATE_GRACE: Duration = Duration::from_millis(50);

/// Write-once outcome latches for the Immediate consumer startup
/// handshake (rc-slvd). The spawned consumer task sends on exactly ONE
/// of the two channels the moment `start()` returns; the detached
/// failure watcher selects on both plus a grace timer.
///
/// Built from `tokio::sync::watch` channels: a fresh receiver treats the
/// initial value as already seen, so `changed()` resolves ONLY on the
/// explicit later send (tokio 1.53.1 semantics — do not switch to
/// `subscribe()`). Dropping the senders without a send is
/// grace-equivalent (`RecvError` → `Ok`).
pub(crate) struct ImmediateLatches {
    ok_rx: watch::Receiver<()>,
    err_rx: watch::Receiver<String>,
}

/// Sender halves of [`ImmediateLatches`], kept alive inside the consumer
/// task until it exits so a latch closure is never the resolution
/// signal — only an explicit send is.
pub(crate) struct ImmediateLatchSenders {
    ok_tx: watch::Sender<()>,
    err_tx: watch::Sender<String>,
}

impl ImmediateLatches {
    /// Create the (senders, latches) pair. The senders move into the
    /// consumer task; the latches half is returned to the controller
    /// alongside the [`StartupReceiver`].
    pub(crate) fn pair() -> (ImmediateLatchSenders, Self) {
        let (ok_tx, ok_rx) = watch::channel(());
        let (err_tx, err_rx) = watch::channel(String::new());
        (
            ImmediateLatchSenders { ok_tx, err_tx },
            Self { ok_rx, err_rx },
        )
    }

    /// Await the Immediate consumer's outcome.
    ///
    /// - `Err(msg)` once the err latch fires (`start()` returned an
    ///   error promptly).
    /// - `Ok(())` once the ok latch fires (`start()` returned Ok
    ///   promptly) or a sender half is dropped without a send
    ///   (grace-equivalent).
    /// - `Ok(())` after `grace` elapses — the loop-style fallback for
    ///   consumers whose `start()` runs until cancellation.
    ///
    /// `biased;` ordering is load-bearing: when a latch and the grace
    /// timer are complete at the same poll, error wins over ok and ok
    /// wins over grace — no boundary race.
    pub(crate) async fn wait(self, grace: Duration) -> Result<(), String> {
        let Self {
            mut ok_rx,
            mut err_rx,
        } = self;
        let err_wait = async move {
            match err_rx.changed().await {
                Ok(()) => Err(err_rx.borrow_and_update().clone()),
                // Sender dropped without a send — grace-equivalent.
                Err(_recv_error) => Ok(()),
            }
        };
        let ok_wait = async move {
            match ok_rx.changed().await {
                Ok(()) => Ok(()),
                Err(_recv_error) => Ok(()),
            }
        };
        tokio::select! {
            biased;
            res = err_wait => res,
            res = ok_wait => res,
            _ = tokio::time::sleep(grace) => Ok(()),
        }
    }
}

impl ImmediateLatchSenders {
    /// `start()` returned `Ok` (②a) — resolves the detached failure
    /// watcher immediately, skipping the grace.
    pub(crate) fn returned_ok(&self) {
        let _ = self.ok_tx.send(());
    }

    /// `start()` returned `Err` (②b) — sent BEFORE the error-path
    /// handling so the watcher observes the failure first.
    pub(crate) fn returned_err(&self, err: String) {
        let _ = self.err_tx.send(err);
    }
}

/// Await the consumer startup handshake and map any failure to a
/// `CamelError::RouteError("Consumer {op} failed: …")`.
///
/// `op` is the operation name used in the error message (e.g. `"startup"`,
/// `"resume"`). Centralises the three controller call sites that previously
/// inlined identical `startup_rx.await_ready().map_err(...)` blocks
/// (rc-w1u9 review I-3).
///
/// The await is bounded by [`CONSUMER_STARTUP_BUDGET`]: if the consumer neither
/// signals readiness nor fails within the budget, route startup fails fast with
/// a timeout error instead of hanging indefinitely. This makes route startup
/// non-hanging for ALL `Explicit` consumers, present and future, regardless of
/// whether the component author wired a local fail-fast bound.
pub(crate) async fn await_consumer_startup(
    startup_rx: StartupReceiver,
    op: &str,
) -> Result<(), CamelError> {
    await_consumer_startup_bounded(startup_rx, op, CONSUMER_STARTUP_BUDGET).await
}

/// Budget-parameterised core of [`await_consumer_startup`]. Split out so the
/// bounded-await behaviour can be unit-tested with a small budget instead of
/// the 90s production value.
async fn await_consumer_startup_bounded(
    startup_rx: StartupReceiver,
    op: &str,
    budget: Duration,
) -> Result<(), CamelError> {
    match tokio::time::timeout(budget, startup_rx.await_ready()).await {
        Ok(inner) => {
            inner.map_err(|e| CamelError::RouteError(format!("Consumer {op} failed: {e}")))
        }
        Err(_elapsed) => Err(CamelError::RouteError(format!(
            "Consumer {op} timed out after {budget:.0?} without signalling readiness \
             (Explicit consumer never called mark_ready/mark_failed)"
        ))),
    }
}

/// Shared task↔watcher outcome cell: false = Pending (nobody accounted
/// this task's termination), true = Accounted (a body path published its
/// failure, or it completed normally through the finally-stop). Relaxed
/// ordering suffices: the TerminationGuard's oneshot send provides the
/// happens-before edge for the watcher's read.
#[derive(Clone)]
pub(crate) struct OuterOutcomeCell(Arc<AtomicBool>);

impl OuterOutcomeCell {
    pub(crate) fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub(crate) fn mark_accounted(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub(crate) fn is_accounted(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Fires the oneshot exactly once when the outer Explicit task ends, in every
/// termination mode. `Option` + `take()` because `oneshot::Sender::send`
/// consumes `self` (cannot move out of `&mut self`).
pub(crate) struct TerminationGuard {
    tx: Option<oneshot::Sender<()>>,
}

impl Drop for TerminationGuard {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(());
        }
    }
}

/// Everything the detached outer-task watcher needs, produced once by
/// `spawn_consumer_task` for Explicit-class consumers.
pub(crate) struct OuterWatcherInputs {
    /// Fired by the task-local TerminationGuard on any task end.
    pub terminated: oneshot::Receiver<()>,
    pub outcome: OuterOutcomeCell,
    /// Cancelled stop → the stop flow owns termination; silent exit.
    pub consumer_cancel: CancellationToken,
    pub route_id: String,
    pub runtime: Option<Weak<dyn RuntimeHandle>>,
    pub crash_notifier: Option<mpsc::Sender<CrashNotification>>,
}

pub(crate) fn spawn_outer_task_watcher(inputs: OuterWatcherInputs) -> JoinHandle<()> {
    tokio::spawn(run_outer_watcher(inputs))
}

async fn run_outer_watcher(inputs: OuterWatcherInputs) {
    // (a) Await termination — the ONLY wait; no timeout (the guard
    // fires when the task ends, however long it runs).
    if inputs.terminated.await.is_err() {
        // Sender dropped without firing (cannot happen with the guard;
        // defensive) — treat as accounted, stay silent.
        return;
    }
    // (b) Cancelled stop owns termination — silent.
    if inputs.consumer_cancel.is_cancelled() {
        return;
    }
    // (c) Accounted (failure already published by the body, or normal
    // completion) — silent. Sole-publisher discipline.
    if inputs.outcome.is_accounted() {
        return;
    }

    let error_msg = format!(
        "Consumer outer task terminated abnormally (panic or abort): {}",
        inputs.route_id
    );
    // log-policy: system-broken
    error!(route_id = %inputs.route_id, "{error_msg}");
    if let Some(tx) = inputs.crash_notifier
        && tx
            .send(CrashNotification {
                route_id: inputs.route_id.clone(),
                error: error_msg.clone(),
            })
            .await
            .is_err()
    {
        warn!(
            route_id = %inputs.route_id,
            "CrashNotification channel closed; crash will not be restarted"
        );
    }
    publish_runtime_failure(inputs.runtime, &inputs.route_id, &error_msg).await;
}

/// Everything the detached failure watcher needs, produced once by
/// `spawn_consumer_task` for Immediate consumers.
pub(crate) struct ImmediateWatcherInputs {
    /// Fired by the task immediately BEFORE `start()`.
    pub start_invoked: oneshot::Receiver<()>,
    pub latches: ImmediateLatches,
    /// Derived from the consumer task's JoinHandle after spawn — the
    /// watcher aborts it on the error path.
    pub abort_handle: tokio::task::AbortHandle,
    /// Fired AFTER the task's error-path block.
    pub error_path_done: oneshot::Receiver<()>,
    pub consumer_cancel: CancellationToken,
    pub route_id: String,
    /// Stamp format, per event — sole-publisher id.
    pub command_id: String,
    /// Same weak handle the deferred paths use.
    pub runtime: Option<Weak<dyn RuntimeHandle>>,
}

/// Production entry point — fixed production grace.
pub(crate) fn spawn_failure_watcher(inputs: ImmediateWatcherInputs) -> JoinHandle<()> {
    tokio::spawn(run_failure_watcher(inputs, CONSUMER_IMMEDIATE_GRACE))
}

/// Internal helper — `grace` is the test seam (loop-style test injects
/// 10ms instead of pausing time against the constant).
async fn run_failure_watcher(inputs: ImmediateWatcherInputs, grace: Duration) {
    // (a) Await start_invoked — no timeout; closed-without-firing → return.
    match inputs.start_invoked.await {
        Ok(()) => {}
        Err(_) => {
            // Task dropped pre-invocation (e.g. aborted externally).
            return;
        }
    }

    // (b) latches.wait(grace) — Ok()/grace-elapse → nothing to surface.
    let Err(msg) = inputs.latches.wait(grace).await else {
        return; // ok-latch or grace elapse; nothing to surface.
    };

    // (c) Err(msg) → await error-path-completion bounded by grace.
    let _ = tokio::time::timeout(grace, inputs.error_path_done).await;

    // Abort + cancel.
    inputs.abort_handle.abort();
    inputs.consumer_cancel.cancel();

    // Degenerate runtime: None or Weak upgrade fails → skip FailRoute.
    let runtime_handle = inputs.runtime.as_ref().and_then(|w| w.upgrade());
    let Some(rt) = runtime_handle else {
        // log-policy: system-broken
        error!(
            route_id = %inputs.route_id,
            "Immediate consumer failed but runtime handle is missing — \
             route will not reach Failed (system-broken)"
        );
        return;
    };

    // PRIMARY FailRoute attempt (t=0).
    let command = RuntimeCommand::FailRoute {
        route_id: inputs.route_id.clone(),
        error: msg.clone(),
        command_id: inputs.command_id.clone(),
        causation_id: None,
    };
    if rt.execute(command).await.is_ok() {
        // accepted or duplicate — route will reach Failed
    } else {
        // Defensive retry after grace (same command_id).
        tokio::time::sleep(grace).await;
        let retry = RuntimeCommand::FailRoute {
            route_id: inputs.route_id.clone(),
            error: msg.clone(),
            command_id: inputs.command_id.clone(),
            causation_id: None,
        };
        if let Err(e) = rt.execute(retry).await {
            // log-policy: system-broken
            error!(
                route_id = %inputs.route_id,
                error = %e,
                "Failure watcher exhausted retries — route will not reach Failed (system-broken)"
            );
        }
    }
}

pub(crate) fn spawn_consumer_task(
    route_id: String,
    mut consumer: Box<dyn Consumer>,
    consumer_ctx: ConsumerContext,
    crash_notifier: Option<mpsc::Sender<CrashNotification>>,
    runtime_for_consumer: Option<Weak<dyn RuntimeHandle>>,
    is_resume: bool,
) -> (
    JoinHandle<()>,
    StartupReceiver,
    Option<ImmediateWatcherInputs>,
    Option<OuterWatcherInputs>,
) {
    match consumer.startup_mode() {
        ConsumerStartupMode::Immediate => {
            // Controller gets a pre-resolved receiver — never yields.
            let startup_receiver = StartupReceiver::immediate();

            // Task still needs a StartupSignal for the consumer context plumbing.
            // Pre-resolve it so the defensive fallback (mark_ready on Ok return)
            // is a no-op — the controller doesn't await this signal for Immediate.
            let (startup_signal, _) = StartupSignal::pair();
            startup_signal.mark_ready();
            let consumer_ctx = consumer_ctx.with_startup(startup_signal.clone());

            let (senders, latches) = ImmediateLatches::pair();
            let (start_invoked_tx, start_invoked_rx) = oneshot::channel();
            let (error_path_done_tx, error_path_done_rx) = oneshot::channel();
            // Derive the watcher-visible cancel token from the consumer
            // context's OWN token (a child of the managed route token), so
            // the watcher's cancel() reaches child tasks the consumer
            // spawned with ctx.cancel_token() — a fresh token here would be
            // observable by nobody (spec: failed resume leaves no detached
            // child tasks). Cancelling the child does NOT cancel the
            // managed parent — sibling concerns are untouched.
            let consumer_cancel = consumer_ctx.cancel_token();
            let command_id = fail_command_id(&route_id);
            // Clone BEFORE the task takes ownership of runtime_for_consumer.
            let runtime_for_watcher = runtime_for_consumer.clone();

            let route_id_for_task = route_id.clone();
            let handle = tokio::spawn(async move {
                // Fire start_invoked BEFORE calling start() — grace starts only
                // after the invocation signal resolves.
                let _ = start_invoked_tx.send(());

                let result = consumer.start(consumer_ctx.clone()).await;
                if let Err(e) = &result {
                    // Err latch FIRST — the watcher observes the failure.
                    senders.returned_err(e.to_string());

                    let error_msg = e.to_string();
                    if is_resume {
                        // log-policy: system-broken
                        error!(route_id = %route_id_for_task, "Consumer error on resume: {e}");
                    } else {
                        // log-policy: system-broken
                        error!(route_id = %route_id_for_task, "Consumer error: {e}");
                    }

                    // CrashNotification (best-effort).
                    if let Some(tx) = &crash_notifier
                        && tx
                            .send(CrashNotification {
                                route_id: route_id_for_task.clone(),
                                error: error_msg.clone(),
                            })
                            .await
                            .is_err()
                    {
                        warn!(route_id = %route_id_for_task, "CrashNotification channel closed; crash will not be restarted");
                    }

                    // NOTE: NO publish_runtime_failure here — the watcher is the
                    // sole executor of FailRoute for Immediate consumers (sole-publisher rule).

                    // H9: clean up any resources the consumer created during the
                    // failed start before dropping it.
                    let _ = consumer.stop().await;

                    // Signal the watcher that the error path is complete.
                    let _ = error_path_done_tx.send(());
                    return;
                }

                // Ok latch — watcher exits early.
                senders.returned_ok();

                // Background task monitoring (unchanged from pre-change).
                let bg_handle = consumer.background_task_handle();

                // Defensive fallback for Explicit consumers (no-op here since
                // the signal is already pre-resolved for Immediate).
                if bg_handle.is_none() && startup_signal.mark_ready() {
                    warn!(
                        route_id = %route_id_for_task,
                        "Explicit consumer returned Ok without calling ctx.mark_ready(); \
                         applied defensive fallback. This indicates a contract violation."
                    );
                }

                if let Some(mut bg_handle) = bg_handle {
                    tokio::select! {
                        result = &mut bg_handle => {
                            match result {
                                Ok(Ok(())) => {}
                                Ok(Err(e)) if !consumer_ctx.is_cancelled() => {
                                    let error_msg = e.to_string();
                                    // log-policy: system-broken
                                    error!(route_id = %route_id_for_task, "Consumer background task failed: {error_msg}");
                                    if let Some(ref tx) = crash_notifier
                                        && tx
                                            .send(CrashNotification {
                                                route_id: route_id_for_task.clone(),
                                                error: error_msg.clone(),
                                            })
                                            .await
                                            .is_err()
                                    {
                                        warn!(route_id = %route_id_for_task, "CrashNotification channel closed; crash will not be restarted");
                                    }
                                    publish_runtime_failure(runtime_for_consumer.clone(), &route_id_for_task, &error_msg).await;
                                }
                                Ok(Err(e)) => {
                                    tracing::debug!(route_id = %route_id_for_task, "Consumer bg task error during shutdown: {e}");
                                }
                                Err(join_err) if !consumer_ctx.is_cancelled() => {
                                    let error_msg = format!("Consumer task panicked: {join_err}");
                                    // log-policy: system-broken
                                    error!(route_id = %route_id_for_task, "{error_msg}");
                                    if let Some(ref tx) = crash_notifier
                                        && tx
                                            .send(CrashNotification {
                                                route_id: route_id_for_task.clone(),
                                                error: error_msg.clone(),
                                            })
                                            .await
                                            .is_err()
                                    {
                                        warn!(route_id = %route_id_for_task, "CrashNotification channel closed; crash will not be restarted");
                                    }
                                    publish_runtime_failure(runtime_for_consumer.clone(), &route_id_for_task, &error_msg).await;
                                }
                                Err(join_err) => {
                                    tracing::debug!(
                                        route_id = %route_id_for_task,
                                        "Consumer bg task panicked during shutdown: {join_err}"
                                    );
                                }
                            }
                        }
                        _ = consumer_ctx.cancelled() => {
                            bg_handle.abort();
                        }
                    }
                }

                // "finally" — always call stop() after start() succeeds
                let _ = consumer.stop().await;
            });

            // Build the watcher inputs from the SPAWNED handle — the abort
            // handle exists the moment the task does (no back-fill slot).
            let inputs = ImmediateWatcherInputs {
                start_invoked: start_invoked_rx,
                latches,
                abort_handle: handle.abort_handle(),
                error_path_done: error_path_done_rx,
                consumer_cancel,
                route_id,
                command_id,
                runtime: runtime_for_watcher,
            };

            (handle, startup_receiver, Some(inputs), None)
        }
        // Forward-compat: any future variant is treated as Explicit so the
        // consumer must signal readiness (or its start() error) before the
        // controller treats the route as started. This is the conservative
        // fallback — assuming Immediate would risk racing past a
        // not-yet-bound listener.
        _ => {
            let (signal, receiver) = StartupSignal::pair();
            let startup_for_task = signal.clone();
            let consumer_ctx = consumer_ctx.with_startup(signal);

            // Outer-task watcher plumbing. Everything the detached watcher
            // needs is captured BEFORE the async move block takes ownership
            // (same pre-spawn-clone idiom as the Immediate arm).
            let outer_outcome = OuterOutcomeCell::new();
            let outer_outcome_for_inputs = outer_outcome.clone();
            let (term_tx, term_rx) = oneshot::channel::<()>();
            let crash_notifier_for_inputs = crash_notifier.clone();
            let route_id_for_inputs = route_id.clone();
            let runtime_for_inputs = runtime_for_consumer.clone();
            let consumer_cancel_for_inputs = consumer_ctx.cancel_token();

            let handle = tokio::spawn(async move {
                // First statement: the guard fires term_rx on Drop, covering
                // every termination mode of this task (return, panic, abort).
                let _guard = TerminationGuard { tx: Some(term_tx) };
                let result = consumer.start(consumer_ctx.clone()).await;
                if let Err(e) = &result {
                    startup_for_task.mark_failed(e.to_string());

                    let error_msg = e.to_string();
                    if is_resume {
                        // log-policy: system-broken
                        error!(route_id = %route_id, "Consumer error on resume: {e}");
                    } else {
                        // log-policy: system-broken
                        error!(route_id = %route_id, "Consumer error: {e}");
                    }

                    if let Some(tx) = crash_notifier
                        && tx
                            .send(CrashNotification {
                                route_id: route_id.clone(),
                                error: error_msg.clone(),
                            })
                            .await
                            .is_err()
                    {
                        warn!(route_id = %route_id, "CrashNotification channel closed; crash will not be restarted");
                    }

                    publish_runtime_failure(runtime_for_consumer, &route_id, &error_msg).await;
                    // Accounted BEFORE the fallible stop below — a panic in that stop must not re-publish.
                    outer_outcome.mark_accounted();

                    let _ = consumer.stop().await;
                    return;
                }

                let bg_handle = consumer.background_task_handle();

                if bg_handle.is_none() && startup_for_task.mark_ready() {
                    warn!(
                        route_id = %route_id,
                        "Explicit consumer returned Ok without calling ctx.mark_ready(); \
                         applied defensive fallback. This indicates a contract violation."
                    );
                }

                if let Some(mut bg_handle) = bg_handle {
                    tokio::select! {
                        result = &mut bg_handle => {
                            match result {
                                Ok(Ok(())) => {}
                                Ok(Err(e)) if !consumer_ctx.is_cancelled() => {
                                    let error_msg = e.to_string();
                                    // log-policy: system-broken
                                    error!(route_id = %route_id, "Consumer background task failed: {error_msg}");
                                    if let Some(ref tx) = crash_notifier
                                        && tx
                                            .send(CrashNotification {
                                                route_id: route_id.clone(),
                                                error: error_msg.clone(),
                                            })
                                            .await
                                            .is_err()
                                    {
                                        warn!(route_id = %route_id, "CrashNotification channel closed; crash will not be restarted");
                                    }
                                    publish_runtime_failure(runtime_for_consumer.clone(), &route_id, &error_msg).await;
                                    // Accounted immediately after publishing — the fallible finally-stop below must not re-publish.
                                    outer_outcome.mark_accounted();
                                }
                                Ok(Err(e)) => {
                                    tracing::debug!(route_id = %route_id, "Consumer bg task error during shutdown: {e}");
                                }
                                Err(join_err) if !consumer_ctx.is_cancelled() => {
                                    let error_msg = format!("Consumer task panicked: {join_err}");
                                    // log-policy: system-broken
                                    error!(route_id = %route_id, "{error_msg}");
                                    if let Some(ref tx) = crash_notifier
                                        && tx
                                            .send(CrashNotification {
                                                route_id: route_id.clone(),
                                                error: error_msg.clone(),
                                            })
                                            .await
                                            .is_err()
                                    {
                                        warn!(route_id = %route_id, "CrashNotification channel closed; crash will not be restarted");
                                    }
                                    publish_runtime_failure(runtime_for_consumer.clone(), &route_id, &error_msg).await;
                                    // Accounted immediately after publishing — the fallible finally-stop below must not re-publish.
                                    outer_outcome.mark_accounted();
                                }
                                Err(join_err) => {
                                    tracing::debug!(
                                        route_id = %route_id,
                                        "Consumer bg task panicked during shutdown: {join_err}"
                                    );
                                }
                            }
                        }
                        _ = consumer_ctx.cancelled() => {
                            bg_handle.abort();
                        }
                    }
                }

                let _ = consumer.stop().await;
                // Normal/shutdown exit accounted only AFTER stop() completes — a panic above stays Pending (abnormal).
                outer_outcome.mark_accounted();
            });

            let outer_inputs = OuterWatcherInputs {
                terminated: term_rx,
                outcome: outer_outcome_for_inputs,
                consumer_cancel: consumer_cancel_for_inputs,
                route_id: route_id_for_inputs,
                runtime: runtime_for_inputs,
                crash_notifier: crash_notifier_for_inputs,
            };
            (handle, receiver, None, Some(outer_inputs))
        }
    }
}

pub(super) async fn stop_route_internal(
    routes: &mut HashMap<String, ManagedRoute>,
    route_id: &str,
    shutdown_timeout: Duration,
) -> Result<(), CamelError> {
    let managed = routes
        .get_mut(route_id)
        .ok_or_else(|| CamelError::RouteError(format!("Route '{}' not found", route_id)))?;

    if !handle_is_running(&managed.consumer_handle) && !handle_is_running(&managed.pipeline_handle)
    {
        return Ok(());
    }

    info!(route_id = %route_id, "Stopping route");

    let managed = routes
        .get_mut(route_id)
        .expect("invariant: route must exist after prior existence check"); // allow-unwrap
    managed.consumer_cancel_token.cancel();

    let managed = routes
        .get_mut(route_id)
        .expect("invariant: route must exist after prior existence check"); // allow-unwrap
    if let Some(agg_svc) = &managed.agg_service {
        agg_svc.force_complete_all();
    }

    // Drop the stored channel sender and join the consumer task BEFORE the
    // drain wait. This ensures no new envelopes can appear in the channel
    // while we drain — closing the buffered-but-undequeued race window
    // (expert review Q1).
    let deadline = tokio::time::Instant::now() + shutdown_timeout;

    let managed = routes
        .get_mut(route_id)
        .expect("invariant: route must exist after prior existence check"); // allow-unwrap
    managed.channel_sender = None;

    // Take + join consumer handle first (bounded by deadline).
    let managed = routes
        .get_mut(route_id)
        .expect("invariant: route must exist after prior existence check"); // allow-unwrap
    let consumer_handle = managed.consumer_handle.take();
    let consumer_abort = consumer_handle.as_ref().map(|h| h.abort_handle());
    let consumer_budget = deadline.saturating_duration_since(tokio::time::Instant::now());
    let consumer_join_result = tokio::time::timeout(consumer_budget, async {
        if let Some(h) = consumer_handle {
            let _ = h.await;
        }
    })
    .await;
    if consumer_join_result.is_err() {
        warn!(
            route_id = %route_id,
            "Consumer task did not stop within {:.0?} — aborting",
            consumer_budget,
        );
        if let Some(h) = consumer_abort {
            h.abort();
        }
    }

    // Drain: wait for in-flight exchanges to complete. The consumer is now
    // fully stopped, so no new envelopes can enter the pipeline. The pipeline
    // tasks' CANCEL_TOKEN is still uncancelled, so run_steps does NOT fire the
    // ConsumerStopping check — exchanges finish normally (ADR-0043 amend).
    let managed = routes
        .get_mut(route_id)
        .expect("invariant: route must exist after prior existence check"); // allow-unwrap
    let drain_counter = Arc::clone(&managed.drain_in_flight);
    let drain_budget = deadline.saturating_duration_since(tokio::time::Instant::now());
    let drain_result = tokio::time::timeout(drain_budget, async {
        while drain_counter.load(std::sync::atomic::Ordering::Relaxed) > 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    if drain_result.is_err() {
        let remaining = drain_counter.load(std::sync::atomic::Ordering::Relaxed);
        warn!(
            route_id = %route_id,
            remaining_in_flight = remaining,
            "Drain timeout — cancelling {} lingering exchange(s)",
            remaining,
        );
    }

    // NOW cancel the pipeline token — only kills stragglers past the grace.
    let managed = routes
        .get_mut(route_id)
        .expect("invariant: route must exist after prior existence check"); // allow-unwrap
    managed.pipeline_cancel_token.cancel();

    // Join the pipeline task (remaining deadline budget).
    let managed = routes
        .get_mut(route_id)
        .expect("invariant: route must exist after prior existence check"); // allow-unwrap
    let pipeline_handle = managed.pipeline_handle.take();
    let pipeline_abort = pipeline_handle.as_ref().map(|h| h.abort_handle());
    let join_budget = deadline.saturating_duration_since(tokio::time::Instant::now());
    let timeout_result = tokio::time::timeout(join_budget, async {
        if let Some(h) = pipeline_handle {
            let _ = h.await;
        }
    })
    .await;

    if timeout_result.is_err() {
        warn!(
            route_id = %route_id,
            "Pipeline task did not stop within {:.0?} — aborting",
            join_budget,
        );
        if let Some(h) = pipeline_abort {
            h.abort();
        }
    }

    // Drain stateful pipeline steps in route order. Intake is cancelled and the
    // pipeline task is joined, so no process() is in flight.
    // Read from the ArcSwap snapshot — authoritative, never stale after hot-swap.
    {
        let managed = routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check"); // allow-unwrap
        let assembly = managed.pipeline.load();
        for step in &assembly.lifecycle {
            if let Err(e) = step
                .shutdown(camel_api::StepShutdownReason::RouteStop)
                .await
            {
                tracing::debug!(
                    step = step.name(),
                    error = %e,
                    "StepLifecycle shutdown failed during stop_route for route {}",
                    route_id
                );
            }
        }
    }

    // Aggregator shutdown via StepLifecycle trait dispatch (post-join).
    // force_complete_all already ran pre-join above; this drains any remaining
    // timeout tasks that the pipeline task's select loop may have spawned.
    {
        let managed = routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check"); // allow-unwrap
        if let Some(agg) = &managed.agg_service
            && let Err(e) =
                <AggregatorService as StepLifecycle>::shutdown(agg, StepShutdownReason::RouteStop)
                    .await
        {
            tracing::warn!(
                route_id = %route_id,
                error = %e,
                "Aggregator shutdown failed during stop_route"
            );
        }
    }

    let managed = routes
        .get_mut(route_id)
        .expect("invariant: route must exist after prior existence check"); // allow-unwrap
    managed.consumer_cancel_token = CancellationToken::new();
    managed.pipeline_cancel_token = CancellationToken::new();
    managed.drain_in_flight = Arc::new(std::sync::atomic::AtomicU64::new(0));

    info!(route_id = %route_id, "Route stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use crate::lifecycle::adapters::pipeline_runtime::PipelineAssembly;
    use crate::lifecycle::adapters::route_runtime_state;
    use crate::lifecycle::application::route_definition::RouteDefinition;
    use arc_swap::ArcSwap;
    use async_trait::async_trait;
    use camel_api::{BoxProcessor, IdentityProcessor};
    use camel_api::{RuntimeCommand, RuntimeCommandResult, SyncBoxProcessor};
    use tokio::sync::oneshot;

    struct FailingConsumer {
        message: &'static str,
        stop_called: Option<Arc<AtomicBool>>,
    }

    impl FailingConsumer {
        fn new(message: &'static str) -> Self {
            Self {
                message,
                stop_called: None,
            }
        }
        fn with_stop_tracking(message: &'static str) -> (Self, Arc<AtomicBool>) {
            let flag = Arc::new(AtomicBool::new(false));
            (
                Self {
                    message,
                    stop_called: Some(Arc::clone(&flag)),
                },
                flag,
            )
        }
    }

    #[async_trait]
    impl Consumer for FailingConsumer {
        async fn start(&mut self, _context: ConsumerContext) -> Result<(), CamelError> {
            Err(CamelError::RouteError(self.message.into()))
        }

        async fn stop(&mut self) -> Result<(), CamelError> {
            if let Some(flag) = &self.stop_called {
                flag.store(true, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    struct ExplicitReadyConsumer {
        stop_panics: bool,
        bg: Option<tokio::task::JoinHandle<Result<(), CamelError>>>,
    }

    #[async_trait]
    impl Consumer for ExplicitReadyConsumer {
        async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
            ctx.mark_ready();
            Ok(())
        }

        async fn stop(&mut self) -> Result<(), CamelError> {
            if self.stop_panics {
                panic!("stop exploded");
            }
            Ok(())
        }

        fn startup_mode(&self) -> ConsumerStartupMode {
            ConsumerStartupMode::Explicit
        }

        fn background_task_handle(
            &mut self,
        ) -> Option<tokio::task::JoinHandle<Result<(), CamelError>>> {
            self.bg.take()
        }
    }

    struct ParkedExplicitConsumer;

    #[async_trait]
    impl Consumer for ParkedExplicitConsumer {
        async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
            ctx.mark_ready();
            std::future::pending::<()>().await;
            Ok(())
        }

        async fn stop(&mut self) -> Result<(), CamelError> {
            Ok(())
        }

        fn startup_mode(&self) -> ConsumerStartupMode {
            ConsumerStartupMode::Explicit
        }
    }

    fn managed_route_with_handles(
        consumer_handle: Option<JoinHandle<()>>,
        pipeline_handle: Option<JoinHandle<()>>,
        channel_sender: Option<mpsc::Sender<camel_component_api::consumer::ExchangeEnvelope>>,
    ) -> ManagedRoute {
        ManagedRoute {
            definition: RouteDefinition::new("timer:test", vec![])
                .with_route_id("route-1")
                .to_info(),
            from_uri: "timer:test".into(),
            pipeline: Arc::new(ArcSwap::from_pointee(PipelineAssembly::new(
                SyncBoxProcessor::new(BoxProcessor::new(IdentityProcessor)),
                vec![],
            ))),
            concurrency: None,
            consumer_handle,
            pipeline_handle,
            consumer_cancel_token: CancellationToken::new(),
            pipeline_cancel_token: CancellationToken::new(),
            channel_sender,
            in_flight: None,
            drain_in_flight: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            aggregate_split: None,
            agg_service: None,
            compiled: route_runtime_state::CompiledRoute {
                security_policy: None,
                security_authenticator: None,
                provider_registry: None,
                security_plan: None,
            },
        }
    }

    #[test]
    fn create_route_consumer_returns_err_for_unknown_scheme() {
        use crate::lifecycle::adapters::controller_component_context::ControllerComponentContext;

        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let component_ctx = Arc::new(ControllerComponentContext::new(
            Arc::clone(&registry),
            Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            Arc::new(camel_api::NoOpMetrics),
            Arc::new(camel_api::NoopPlatformService::default()),
            Arc::new(crate::health_registry::HealthCheckRegistry::new(
                std::time::Duration::from_secs(5),
            )),
            None,
            false,
        ));
        let rt: Arc<dyn RuntimeObservability> = Arc::clone(&component_ctx) as Arc<_>;

        let err = match create_route_consumer(rt, &registry, "unknown:foo", component_ctx.as_ref())
        {
            Ok(_) => panic!("unknown scheme should fail consumer creation"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("unknown"));
    }

    #[tokio::test]
    async fn stop_route_internal_returns_not_found_when_route_absent() {
        let mut routes = HashMap::new();

        let err = stop_route_internal(&mut routes, "missing-route", Duration::from_secs(5))
            .await
            .expect_err("stopping a missing route should fail");

        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn stop_route_internal_short_circuits_when_already_stopped() {
        let (tx, _rx) = mpsc::channel(1);
        let mut routes = HashMap::new();
        routes.insert(
            "route-1".to_string(),
            managed_route_with_handles(None, None, Some(tx)),
        );

        let result = stop_route_internal(&mut routes, "route-1", Duration::from_secs(5)).await;

        assert!(result.is_ok());
        let managed = routes.get("route-1").expect("route must still exist");
        assert!(managed.channel_sender.is_some());
    }

    #[tokio::test]
    async fn spawn_consumer_task_resume_failure_sends_crash_notification() {
        let (tx, _rx) = mpsc::channel(1);
        let ctx = ConsumerContext::new(
            tx,
            CancellationToken::new(),
            "consumer-mgmt-test-route".to_string(),
        );
        let (crash_tx, mut crash_rx) = mpsc::channel(1);

        let (handle, _startup_rx, _watcher_inputs, _outer_inputs) = spawn_consumer_task(
            "route-resume".to_string(),
            Box::new(FailingConsumer::new("resume start failed")),
            ctx,
            Some(crash_tx),
            None,
            true,
        );

        handle.await.expect("consumer task should join cleanly");

        let notification = crash_rx
            .recv()
            .await
            .expect("crash notification should be sent");
        assert_eq!(notification.route_id, "route-resume");
        assert!(notification.error.contains("resume start failed"));
    }

    #[tokio::test]
    async fn start_error_calls_stop_no_leak() {
        let (tx, _rx) = mpsc::channel(1);
        let ctx =
            ConsumerContext::new(tx, CancellationToken::new(), "consumer-h9-test".to_string());
        let (crash_tx, _crash_rx) = mpsc::channel(1);

        let (consumer, stop_called) = FailingConsumer::with_stop_tracking("start failed");

        let (handle, _startup_rx, _watcher_inputs, _outer_inputs) = spawn_consumer_task(
            "route-h9".to_string(),
            Box::new(consumer),
            ctx,
            Some(crash_tx),
            None,
            false,
        );

        handle.await.expect("consumer task should join");

        assert!(
            stop_called.load(Ordering::SeqCst),
            "stop() must be called on start() error — H9 resource-leak fix"
        );
    }

    // --- GRL-001: Deferred failure crash propagation ---

    struct DeferredFailConsumer {
        handle: Option<tokio::task::JoinHandle<Result<(), CamelError>>>,
    }

    impl DeferredFailConsumer {
        fn new(err: &'static str) -> Self {
            let err_msg = err.to_string();
            let handle = tokio::task::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                Err(CamelError::ProcessorError(err_msg))
            });
            Self {
                handle: Some(handle),
            }
        }
    }

    #[async_trait]
    impl Consumer for DeferredFailConsumer {
        async fn start(&mut self, _ctx: ConsumerContext) -> Result<(), CamelError> {
            Ok(())
        }
        async fn stop(&mut self) -> Result<(), CamelError> {
            Ok(())
        }
        fn background_task_handle(
            &mut self,
        ) -> Option<tokio::task::JoinHandle<Result<(), CamelError>>> {
            self.handle.take()
        }
    }

    #[tokio::test]
    async fn spawn_consumer_task_deferred_failure_sends_crash_notification() {
        let (exchange_tx, _rx) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        let ctx = ConsumerContext::new(exchange_tx, cancel, "consumer-mgmt-test-route".to_string());
        let (crash_tx, mut crash_rx) = mpsc::channel(1);

        let (handle, _startup_rx, _watcher_inputs, _outer_inputs) = spawn_consumer_task(
            "route-deferred".to_string(),
            Box::new(DeferredFailConsumer::new("broker lost")),
            ctx,
            Some(crash_tx),
            None,
            false,
        );

        handle.await.expect("outer task should complete");

        let notification = crash_rx.recv().await.expect("crash notification expected");
        assert_eq!(notification.route_id, "route-deferred");
        assert!(notification.error.contains("broker lost"));
    }

    #[tokio::test]
    async fn spawn_consumer_task_deferred_failure_suppressed_on_cancellation() {
        let (exchange_tx, _rx) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        let ctx = ConsumerContext::new(
            exchange_tx,
            cancel.clone(),
            "consumer-mgmt-test-route".to_string(),
        );
        let (crash_tx, mut crash_rx) = mpsc::channel(1);

        // Cancel BEFORE the bg task exits — simulates graceful shutdown
        cancel.cancel();

        let (handle, _startup_rx, _watcher_inputs, _outer_inputs) = spawn_consumer_task(
            "route-cancel".to_string(),
            Box::new(DeferredFailConsumer::new("shutdown error")),
            ctx,
            Some(crash_tx),
            None,
            false,
        );

        handle.await.expect("outer task should complete");

        // Should receive NO crash notification — error during shutdown is suppressed
        crash_rx.close();
        assert!(
            crash_rx.recv().await.is_none(),
            "no crash notification expected on cancelled shutdown"
        );
    }

    #[tokio::test]
    async fn spawn_consumer_task_calls_stop_on_cancellation() {
        use std::sync::atomic::{AtomicBool, Ordering};

        static STOP_CALLED: AtomicBool = AtomicBool::new(false);

        struct StopTrackingConsumer;

        #[async_trait]
        impl Consumer for StopTrackingConsumer {
            async fn start(&mut self, _context: ConsumerContext) -> Result<(), CamelError> {
                Ok(())
            }
            async fn stop(&mut self) -> Result<(), CamelError> {
                STOP_CALLED.store(true, Ordering::SeqCst);
                Ok(())
            }
        }

        let cancel = CancellationToken::new();
        let (tx, _rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(tx, cancel.clone(), "consumer-mgmt-test-route".to_string());

        let (handle, _startup_rx, _watcher_inputs, _outer_inputs) = spawn_consumer_task(
            "test-route".into(),
            Box::new(StopTrackingConsumer),
            ctx,
            None,
            None,
            false,
        );

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel.cancel();

        let result = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        assert!(
            result.is_ok(),
            "spawn_consumer_task should complete after cancellation"
        );
        assert!(
            STOP_CALLED.load(Ordering::SeqCst),
            "consumer.stop() should have been called"
        );
    }

    // ── Task 5: Lifecycle drain ──

    struct LifecycleRecorder {
        reasons: std::sync::Mutex<Vec<camel_api::StepShutdownReason>>,
    }

    impl std::fmt::Debug for LifecycleRecorder {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("LifecycleRecorder").finish()
        }
    }

    #[async_trait]
    impl camel_api::StepLifecycle for LifecycleRecorder {
        fn name(&self) -> &'static str {
            "test-recorder"
        }
        async fn shutdown(
            &self,
            r: camel_api::StepShutdownReason,
        ) -> Result<(), camel_api::CamelError> {
            self.reasons.lock().unwrap().push(r);
            Ok(())
        }
    }

    #[tokio::test]
    async fn stop_route_drains_lifecycle_handles() {
        let recorder = Arc::new(LifecycleRecorder {
            reasons: std::sync::Mutex::new(vec![]),
        });
        let lifecycle: Arc<dyn camel_api::StepLifecycle> = recorder.clone();

        let assembly = PipelineAssembly::new(
            SyncBoxProcessor::new(BoxProcessor::new(IdentityProcessor)),
            vec![lifecycle],
        );

        let (mpsc_tx, _rx) = mpsc::channel(1);

        // Use oneshot channels to keep spawned tasks alive deterministically.
        // Tasks block on rx.await, staying "running" until the test cleans up.
        // stop_route_internal wraps the join in a timeout, so the tasks don't
        // need to complete — the timeout fires and drain proceeds.
        let (consumer_tx, consumer_rx) = oneshot::channel::<()>();
        let (pipeline_tx, pipeline_rx) = oneshot::channel::<()>();

        let consumer_handle = tokio::spawn(async {
            let _ = consumer_rx.await;
        });
        let pipeline_handle = tokio::spawn(async {
            let _ = pipeline_rx.await;
        });

        let mut routes = HashMap::new();
        routes.insert(
            "route-lifecycle-test".to_string(),
            ManagedRoute {
                definition: RouteDefinition::new("timer:test", vec![])
                    .with_route_id("route-lifecycle-test")
                    .to_info(),
                from_uri: "timer:test".into(),
                pipeline: Arc::new(ArcSwap::from_pointee(assembly)),
                concurrency: None,
                consumer_handle: Some(consumer_handle),
                pipeline_handle: Some(pipeline_handle),
                consumer_cancel_token: CancellationToken::new(),
                pipeline_cancel_token: CancellationToken::new(),
                channel_sender: Some(mpsc_tx),
                in_flight: None,
                drain_in_flight: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
                aggregate_split: None,
                agg_service: None,
                compiled: route_runtime_state::CompiledRoute {
                    security_policy: None,
                    security_authenticator: None,
                    provider_registry: None,
                    security_plan: None,
                },
            },
        );

        // Short timeout: the spawned tasks block on oneshot receivers and never
        // complete, so the join times out and the test proceeds to drain lifecycle.
        let result = stop_route_internal(
            &mut routes,
            "route-lifecycle-test",
            Duration::from_millis(500),
        )
        .await;
        assert!(result.is_ok(), "stop_route_internal should succeed");

        let reasons = recorder.reasons.lock().unwrap();
        assert_eq!(
            *reasons,
            vec![camel_api::StepShutdownReason::RouteStop],
            "lifecycle.shutdown should have been called with RouteStop once"
        );

        // Clean up: drop senders so spawned tasks can complete.
        drop(consumer_tx);
        drop(pipeline_tx);
    }

    // ── D-M7: shutdown-timeout aborts tasks (no detach) ──

    struct AbortFlag(Arc<AtomicBool>);
    impl Drop for AbortFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn shutdown_timeout_aborts_tasks_not_detach() {
        let abort_flag = Arc::new(AtomicBool::new(false));
        let flag_clone = Arc::clone(&abort_flag);

        // Spawn a task that blocks forever, owning a Drop guard.
        // On abort(), tokio drops the future -> Drop runs -> flag set.
        // On detach (the bug), the future stays alive -> flag stays false.
        let consumer_handle = tokio::spawn(async move {
            let _guard = AbortFlag(flag_clone);
            std::future::pending::<()>().await;
        });

        let mut routes = HashMap::new();
        let route = managed_route_with_handles(Some(consumer_handle), None, None);
        routes.insert("route-1".to_string(), route);

        // Tiny timeout — the blocking task can't finish.
        stop_route_internal(&mut routes, "route-1", Duration::from_millis(50))
            .await
            .expect("stop_route_internal should succeed");

        // Bounded poll loop: avoids fixed-sleep flake risk under CI overload.
        for _ in 0..100 {
            if abort_flag.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert!(
            abort_flag.load(Ordering::SeqCst),
            "consumer task must be aborted (Drop ran) — if false, the task was detached (D-M7 bug)"
        );
    }

    #[tokio::test]
    async fn shutdown_timeout_aborts_both_consumer_and_pipeline() {
        let consumer_flag = Arc::new(AtomicBool::new(false));
        let pipeline_flag = Arc::new(AtomicBool::new(false));
        let cf = Arc::clone(&consumer_flag);
        let pf = Arc::clone(&pipeline_flag);

        let consumer_handle = tokio::spawn(async move {
            let _guard = AbortFlag(cf);
            std::future::pending::<()>().await;
        });
        let pipeline_handle = tokio::spawn(async move {
            let _guard = AbortFlag(pf);
            std::future::pending::<()>().await;
        });

        let mut routes = HashMap::new();
        let route = managed_route_with_handles(Some(consumer_handle), Some(pipeline_handle), None);
        routes.insert("route-both".to_string(), route);

        stop_route_internal(&mut routes, "route-both", Duration::from_millis(50))
            .await
            .expect("stop_route_internal should succeed");

        // Bounded poll loop (avoids fixed-sleep flake risk)
        for _ in 0..100 {
            if consumer_flag.load(Ordering::SeqCst) && pipeline_flag.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert!(
            consumer_flag.load(Ordering::SeqCst),
            "consumer task must be aborted"
        );
        assert!(
            pipeline_flag.load(Ordering::SeqCst),
            "pipeline task must be aborted"
        );
    }

    // ── D-L7: CrashNotification send-error logging ──

    #[tokio::test]
    async fn test_crash_notification_warns_on_closed_channel() {
        use tracing_subscriber::prelude::*;
        let warn_seen = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let warn_seen_clone = warn_seen.clone();

        let layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::sink)
            .with_filter(tracing_subscriber::filter::filter_fn(move |meta| {
                if meta.level() == &tracing::Level::WARN {
                    warn_seen_clone.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                true
            }));

        let _guard = tracing_subscriber::registry().with(layer).set_default();

        let (exchange_tx, _exchange_rx) = mpsc::channel(1);
        let ctx = ConsumerContext::new(
            exchange_tx,
            CancellationToken::new(),
            "consumer-warn-test".to_string(),
        );
        let (crash_tx, crash_rx) = mpsc::channel::<CrashNotification>(1);
        drop(crash_rx); // close the channel so send fails

        let (handle, _startup_rx, _watcher_inputs, _outer_inputs) = spawn_consumer_task(
            "route-warn".to_string(),
            Box::new(FailingConsumer::new("start failed")),
            ctx,
            Some(crash_tx),
            None,
            false,
        );

        handle.await.expect("consumer task should join cleanly");

        assert!(
            warn_seen.load(std::sync::atomic::Ordering::SeqCst),
            "expected warn! log when CrashNotification send fails on closed channel — \
             D-L7: let _ = silently drops send error, no restart triggered"
        );
    }

    // ── Immediate consumer returning Ok must NOT log "contract violation" ──

    #[tokio::test]
    async fn immediate_consumer_returning_ok_emits_no_contract_violation_warn() {
        // Regression: an Immediate consumer (timer, cron, file, …) whose
        // start() returns Ok must not trip the defensive fallback. The
        // fallback's mark_ready() must be a no-op because the Immediate
        // signal is pre-resolved to Ready. Before the fix the Pending seed
        // made mark_ready return true and logged a spurious warning on every
        // natural completion.
        use tracing_subscriber::prelude::*;
        let warn_seen = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Count only real dispatched warns from this module. A `filter_fn`
        // side-effect is unusable here: filter callbacks also run during
        // callsite interest registration, so any warn callsite visible in the
        // process (other tests' emissions, incl. same-module ones) trips an
        // unscoped or target-scoped level check (bd rc-u9hs family).
        // `on_event` fires only for events actually dispatched to this layer.
        struct WarnCounter {
            seen: std::sync::Arc<std::sync::atomic::AtomicBool>,
        }
        impl<C> tracing_subscriber::Layer<C> for WarnCounter
        where
            C: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
        {
            fn on_event(
                &self,
                event: &tracing::Event<'_>,
                _ctx: tracing_subscriber::layer::Context<'_, C>,
            ) {
                if *event.metadata().level() == tracing::Level::WARN
                    && event.metadata().target()
                        == "camel_core::lifecycle::adapters::consumer_management"
                {
                    self.seen.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
        }

        let _guard = tracing_subscriber::registry()
            .with(WarnCounter {
                seen: warn_seen.clone(),
            })
            .set_default();

        struct ImmediateOkConsumer;
        #[async_trait]
        impl Consumer for ImmediateOkConsumer {
            async fn start(&mut self, _context: ConsumerContext) -> Result<(), CamelError> {
                Ok(())
            }
            async fn stop(&mut self) -> Result<(), CamelError> {
                Ok(())
            }
        }

        let (exchange_tx, _exchange_rx) = mpsc::channel(1);
        let ctx = ConsumerContext::new(
            exchange_tx,
            CancellationToken::new(),
            "immediate-ok-test".to_string(),
        );

        let (handle, _startup_rx, _watcher_inputs, _outer_inputs) = spawn_consumer_task(
            "route-immediate".to_string(),
            Box::new(ImmediateOkConsumer),
            ctx,
            None,
            None,
            false,
        );

        handle.await.expect("consumer task should join cleanly");

        assert!(
            !warn_seen.load(std::sync::atomic::Ordering::SeqCst),
            "Immediate consumer returning Ok must not emit a WARN — \
             the defensive fallback should be a no-op (signal pre-resolved)"
        );
    }

    // ── rc-w1u9: ConsumerStartupMode handshake tests live in
    // `handshake_tests.rs` (declared at the bottom of this file). They were
    // extracted to keep this file under the thermo-nuclear size ceiling. ──

    // ── Startup-budget backstop: await_consumer_startup must NEVER hang ──

    #[tokio::test]
    async fn await_consumer_startup_times_out_when_never_ready() {
        // An Explicit consumer that never calls mark_ready/mark_failed must
        // NOT hang route startup: the bounded await surfaces a timeout error.
        // Hold the signal so the receiver stays Pending forever (dropping it
        // would resolve via the "sender dropped" path instead of the timeout).
        let (_signal, receiver) = StartupSignal::pair();

        let result =
            await_consumer_startup_bounded(receiver, "startup", Duration::from_millis(50)).await;

        let err = result.expect_err("must fail — an unresolved startup cannot succeed");
        assert!(
            err.to_string().contains("timed out"),
            "expected timeout error, got: {err}"
        );
    }

    #[tokio::test]
    async fn await_consumer_startup_returns_ready_before_budget() {
        // A consumer that marks ready promptly resolves without waiting the
        // full budget (the timeout wrapper is transparent on the happy path).
        let (signal, receiver) = StartupSignal::pair();
        signal.mark_ready();
        let result =
            await_consumer_startup_bounded(receiver, "startup", Duration::from_secs(30)).await;
        assert!(result.is_ok(), "prompt readiness must resolve Ok");
    }

    #[tokio::test]
    async fn await_consumer_startup_propagates_mark_failed_error() {
        // A consumer that fails fast (mark_failed) surfaces its error, not the
        // generic timeout — the failure reason must reach the operator.
        let (signal, receiver) = StartupSignal::pair();
        signal.mark_failed("broker unreachable".to_string());
        let err = await_consumer_startup_bounded(receiver, "startup", Duration::from_secs(30))
            .await
            .expect_err("mark_failed must surface as Err");
        assert!(
            err.to_string().contains("broker unreachable"),
            "expected the consumer's failure reason, got: {err}"
        );
    }

    // ── rc-slvd: Immediate consumers' prompt start() outcome is observed
    // by the detached failure watcher (not the controller) ──

    /// Shared recording fake for watcher tests.
    struct RecordingRuntime {
        recorder: Arc<std::sync::Mutex<Vec<RuntimeCommand>>>,
        reject: bool,
    }
    #[async_trait::async_trait]
    impl camel_api::RuntimeCommandBus for RecordingRuntime {
        async fn execute(&self, cmd: RuntimeCommand) -> Result<RuntimeCommandResult, CamelError> {
            self.recorder.lock().expect("lock").push(cmd.clone());
            if self.reject {
                Err(CamelError::RouteError("rejected".into()))
            } else {
                Ok(RuntimeCommandResult::Accepted)
            }
        }
    }
    #[async_trait::async_trait]
    impl camel_api::RuntimeQueryBus for RecordingRuntime {
        async fn ask(
            &self,
            _query: camel_api::RuntimeQuery,
        ) -> Result<camel_api::RuntimeQueryResult, CamelError> {
            Ok(camel_api::RuntimeQueryResult::RouteStatus {
                route_id: "test".into(),
                status: "Started".into(),
            })
        }
    }

    #[tokio::test]
    async fn outer_task_watcher_silent_when_accounted() {
        let recorder = Arc::new(std::sync::Mutex::new(Vec::new()));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(RecordingRuntime {
            recorder: Arc::clone(&recorder),
            reject: false,
        });
        let (terminated_tx, terminated) = oneshot::channel();
        let outcome = OuterOutcomeCell::new();
        outcome.mark_accounted();
        let watcher = spawn_outer_task_watcher(OuterWatcherInputs {
            terminated,
            outcome,
            consumer_cancel: CancellationToken::new(),
            route_id: "route-accounted".to_string(),
            runtime: Some(Arc::downgrade(&runtime)),
            crash_notifier: None,
        });
        terminated_tx
            .send(())
            .expect("termination signal must be delivered");

        tokio::time::timeout(Duration::from_secs(2), watcher)
            .await
            .expect("watcher must complete") // allow-unwrap: test-only
            .expect("watcher task must join");
        assert!(recorder.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn outer_task_watcher_silent_when_cancelled() {
        let recorder = Arc::new(std::sync::Mutex::new(Vec::new()));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(RecordingRuntime {
            recorder: Arc::clone(&recorder),
            reject: false,
        });
        let consumer_cancel = CancellationToken::new();
        consumer_cancel.cancel();
        let (terminated_tx, terminated) = oneshot::channel();
        let watcher = spawn_outer_task_watcher(OuterWatcherInputs {
            terminated,
            outcome: OuterOutcomeCell::new(),
            consumer_cancel,
            route_id: "route-cancelled".to_string(),
            runtime: Some(Arc::downgrade(&runtime)),
            crash_notifier: None,
        });
        terminated_tx
            .send(())
            .expect("termination signal must be delivered");

        tokio::time::timeout(Duration::from_secs(2), watcher)
            .await
            .expect("watcher must complete") // allow-unwrap: test-only
            .expect("watcher task must join");
        assert!(recorder.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn outer_task_watcher_publishes_when_pending() {
        let recorder = Arc::new(std::sync::Mutex::new(Vec::new()));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(RecordingRuntime {
            recorder: Arc::clone(&recorder),
            reject: false,
        });
        let (terminated_tx, terminated) = oneshot::channel();
        let watcher = spawn_outer_task_watcher(OuterWatcherInputs {
            terminated,
            outcome: OuterOutcomeCell::new(),
            consumer_cancel: CancellationToken::new(),
            route_id: "route-pending".to_string(),
            runtime: Some(Arc::downgrade(&runtime)),
            crash_notifier: None,
        });
        terminated_tx
            .send(())
            .expect("termination signal must be delivered");

        tokio::time::timeout(Duration::from_secs(2), watcher)
            .await
            .expect("watcher must complete") // allow-unwrap: test-only
            .expect("watcher task must join");
        let commands = recorder.lock().expect("lock");
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            RuntimeCommand::FailRoute { error, .. } => {
                assert!(error.contains("terminated abnormally"));
            }
            other => panic!("expected FailRoute, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn outer_task_watcher_failroute_on_panic_in_stop() {
        let recorder = Arc::new(std::sync::Mutex::new(Vec::new()));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(RecordingRuntime {
            recorder: Arc::clone(&recorder),
            reject: false,
        });
        let token = CancellationToken::new();
        let (tx, _rx) = mpsc::channel(1);
        let ctx = ConsumerContext::new(tx, token.clone(), "route-stop-panic".to_string());

        let (handle, startup_rx, _watcher_inputs, outer_inputs) = spawn_consumer_task(
            "route-stop-panic".to_string(),
            Box::new(ExplicitReadyConsumer {
                stop_panics: true,
                bg: None,
            }),
            ctx,
            None,
            Some(Arc::downgrade(&runtime)),
            false,
        );
        let startup = tokio::time::timeout(Duration::from_secs(2), startup_rx.await_ready())
            .await
            .expect("startup receiver must resolve") // allow-unwrap: test-only
            ;
        assert!(startup.is_ok());

        let watcher = spawn_outer_task_watcher(
            outer_inputs.expect("Explicit consumer must yield outer watcher inputs"), // allow-unwrap: test-only
        );
        let join = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("consumer task must terminate") // allow-unwrap: test-only
            ;
        assert!(join.is_err(), "stop panic must produce a JoinError");
        tokio::time::timeout(Duration::from_secs(2), watcher)
            .await
            .expect("watcher must complete") // allow-unwrap: test-only
            .expect("watcher task must join"); // allow-unwrap: test-only

        let commands = recorder.lock().expect("recorder lock"); // allow-unwrap: test-only
        assert_eq!(commands.len(), 1);
        assert!(matches!(
            &commands[0],
            RuntimeCommand::FailRoute { error, .. } if error.contains("terminated abnormally")
        ));
    }

    #[tokio::test]
    async fn outer_task_watcher_silent_on_normal_completion() {
        let recorder = Arc::new(std::sync::Mutex::new(Vec::new()));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(RecordingRuntime {
            recorder: Arc::clone(&recorder),
            reject: false,
        });
        let token = CancellationToken::new();
        let (tx, _rx) = mpsc::channel(1);
        let ctx = ConsumerContext::new(tx, token, "route-normal".to_string());

        let (handle, startup_rx, _watcher_inputs, outer_inputs) = spawn_consumer_task(
            "route-normal".to_string(),
            Box::new(ExplicitReadyConsumer {
                stop_panics: false,
                bg: None,
            }),
            ctx,
            None,
            Some(Arc::downgrade(&runtime)),
            false,
        );
        let startup = tokio::time::timeout(Duration::from_secs(2), startup_rx.await_ready())
            .await
            .expect("startup receiver must resolve") // allow-unwrap: test-only
            ;
        assert!(startup.is_ok());
        let watcher = spawn_outer_task_watcher(
            outer_inputs.expect("Explicit consumer must yield outer watcher inputs"), // allow-unwrap: test-only
        );
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("consumer task must terminate") // allow-unwrap: test-only
            .expect("consumer task must join") // allow-unwrap: test-only
            ;
        tokio::time::timeout(Duration::from_secs(2), watcher)
            .await
            .expect("watcher must complete") // allow-unwrap: test-only
            .expect("watcher task must join"); // allow-unwrap: test-only
        assert!(recorder.lock().expect("recorder lock").is_empty()); // allow-unwrap: test-only
    }

    #[tokio::test]
    async fn outer_task_watcher_silent_on_normal_completion_with_bg() {
        let recorder = Arc::new(std::sync::Mutex::new(Vec::new()));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(RecordingRuntime {
            recorder: Arc::clone(&recorder),
            reject: false,
        });
        let token = CancellationToken::new();
        let (tx, _rx) = mpsc::channel(1);
        let ctx = ConsumerContext::new(tx, token, "route-normal-bg".to_string());

        let (handle, startup_rx, _watcher_inputs, outer_inputs) = spawn_consumer_task(
            "route-normal-bg".to_string(),
            Box::new(ExplicitReadyConsumer {
                stop_panics: false,
                bg: Some(tokio::spawn(async { Ok::<(), CamelError>(()) })),
            }),
            ctx,
            None,
            Some(Arc::downgrade(&runtime)),
            false,
        );
        let startup = tokio::time::timeout(Duration::from_secs(2), startup_rx.await_ready())
            .await
            .expect("startup receiver must resolve") // allow-unwrap: test-only
            ;
        assert!(startup.is_ok());
        let watcher = spawn_outer_task_watcher(
            outer_inputs.expect("Explicit consumer must yield outer watcher inputs"), // allow-unwrap: test-only
        );
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("consumer task must terminate") // allow-unwrap: test-only
            .expect("consumer task must join") // allow-unwrap: test-only
            ;
        tokio::time::timeout(Duration::from_secs(2), watcher)
            .await
            .expect("watcher must complete") // allow-unwrap: test-only
            .expect("watcher task must join"); // allow-unwrap: test-only
        assert!(recorder.lock().expect("recorder lock").is_empty()); // allow-unwrap: test-only
    }

    #[tokio::test]
    async fn outer_task_watcher_silent_on_cancel() {
        let recorder = Arc::new(std::sync::Mutex::new(Vec::new()));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(RecordingRuntime {
            recorder: Arc::clone(&recorder),
            reject: false,
        });
        let token = CancellationToken::new();
        let (tx, _rx) = mpsc::channel(1);
        let ctx = ConsumerContext::new(tx, token.clone(), "route-cancel".to_string());

        let (handle, startup_rx, _watcher_inputs, outer_inputs) = spawn_consumer_task(
            "route-cancel".to_string(),
            Box::new(ParkedExplicitConsumer),
            ctx,
            None,
            Some(Arc::downgrade(&runtime)),
            false,
        );
        let startup = tokio::time::timeout(Duration::from_secs(2), startup_rx.await_ready())
            .await
            .expect("startup receiver must resolve") // allow-unwrap: test-only
            ;
        assert!(startup.is_ok());
        let watcher = spawn_outer_task_watcher(
            outer_inputs.expect("Explicit consumer must yield outer watcher inputs"), // allow-unwrap: test-only
        );
        token.cancel();
        handle.abort();
        let join = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("aborted consumer task must terminate") // allow-unwrap: test-only
            ;
        assert!(join.is_err());
        tokio::time::timeout(Duration::from_secs(2), watcher)
            .await
            .expect("watcher must complete") // allow-unwrap: test-only
            .expect("watcher task must join"); // allow-unwrap: test-only
        assert!(recorder.lock().expect("recorder lock").is_empty()); // allow-unwrap: test-only
    }

    #[tokio::test]
    async fn outer_task_watcher_failroute_on_uncancelled_abort() {
        let recorder = Arc::new(std::sync::Mutex::new(Vec::new()));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(RecordingRuntime {
            recorder: Arc::clone(&recorder),
            reject: false,
        });
        let runtime_weak = Arc::downgrade(&runtime);
        let token = CancellationToken::new();
        let (tx, _rx) = mpsc::channel(1);
        let ctx = ConsumerContext::new(tx, token, "route-uncancelled-abort".to_string());
        let (crash_tx, mut crash_rx) = mpsc::channel(1);

        let (handle, startup_rx, _watcher_inputs, outer_inputs) = spawn_consumer_task(
            "route-uncancelled-abort".to_string(),
            Box::new(ParkedExplicitConsumer),
            ctx,
            Some(crash_tx),
            Some(runtime_weak),
            false,
        );
        let startup = tokio::time::timeout(Duration::from_secs(2), startup_rx.await_ready())
            .await
            .expect("startup receiver must resolve") // allow-unwrap: test-only
            ;
        assert!(startup.is_ok());
        let watcher = spawn_outer_task_watcher(
            outer_inputs.expect("Explicit consumer must yield outer watcher inputs"), // allow-unwrap: test-only
        );
        handle.abort();
        let join = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("aborted consumer task must terminate") // allow-unwrap: test-only
            ;
        assert!(join.is_err());
        tokio::time::timeout(Duration::from_secs(2), watcher)
            .await
            .expect("watcher must complete") // allow-unwrap: test-only
            .expect("watcher task must join"); // allow-unwrap: test-only

        {
            let commands = recorder.lock().expect("recorder lock"); // allow-unwrap: test-only
            assert_eq!(commands.len(), 1);
            assert!(matches!(
                &commands[0],
                RuntimeCommand::FailRoute { error, .. } if error.contains("terminated abnormally")
            ));
        }
        let notification = tokio::time::timeout(Duration::from_secs(2), crash_rx.recv())
            .await
            .expect("crash notification must arrive") // allow-unwrap: test-only
            .expect("crash notification must be sent"); // allow-unwrap: test-only
        assert_eq!(notification.route_id, "route-uncancelled-abort");
    }

    #[tokio::test]
    async fn outer_task_watcher_no_double_publish_when_stop_panics_after_bg_publish() {
        let recorder = Arc::new(std::sync::Mutex::new(Vec::new()));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(RecordingRuntime {
            recorder: Arc::clone(&recorder),
            reject: false,
        });
        let token = CancellationToken::new();
        let (tx, _rx) = mpsc::channel(1);
        let ctx = ConsumerContext::new(tx, token, "route-bg-stop-panic".to_string());

        let (handle, startup_rx, _watcher_inputs, outer_inputs) = spawn_consumer_task(
            "route-bg-stop-panic".to_string(),
            Box::new(ExplicitReadyConsumer {
                stop_panics: true,
                bg: Some(tokio::spawn(async {
                    Err(CamelError::RouteError("bg died".into()))
                })),
            }),
            ctx,
            None,
            Some(Arc::downgrade(&runtime)),
            false,
        );
        let startup = tokio::time::timeout(Duration::from_secs(2), startup_rx.await_ready())
            .await
            .expect("startup receiver must resolve") // allow-unwrap: test-only
            ;
        assert!(startup.is_ok());
        let watcher = spawn_outer_task_watcher(
            outer_inputs.expect("Explicit consumer must yield outer watcher inputs"), // allow-unwrap: test-only
        );
        let join = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("consumer task must terminate") // allow-unwrap: test-only
            ;
        assert!(join.is_err(), "stop panic must produce a JoinError");
        tokio::time::timeout(Duration::from_secs(2), watcher)
            .await
            .expect("watcher must complete") // allow-unwrap: test-only
            .expect("watcher task must join"); // allow-unwrap: test-only

        let commands = recorder.lock().expect("recorder lock"); // allow-unwrap: test-only
        assert_eq!(commands.len(), 1);
        assert!(matches!(
            &commands[0],
            RuntimeCommand::FailRoute { error, .. }
                if error.contains("bg died") && !error.contains("terminated abnormally")
        ));
    }

    #[tokio::test]
    async fn immediate_fast_error_watcher_fails_route() {
        // An Immediate consumer whose start() fails fast: the watcher
        // issues FailRoute, the controller never sees the error.
        use camel_api::RuntimeCommand;
        use std::sync::Mutex;

        let recorder: Arc<Mutex<Vec<RuntimeCommand>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder_clone = Arc::clone(&recorder);

        let runtime: Arc<dyn RuntimeHandle> = Arc::new(RecordingRuntime {
            recorder: recorder_clone,
            reject: false,
        });
        let runtime_weak = Arc::downgrade(&runtime);

        let (tx, _rx) = mpsc::channel(1);
        let ctx = ConsumerContext::new(tx, CancellationToken::new(), "immediate-err".to_string());

        let (handle, startup_rx, watcher_inputs, _outer_inputs) = spawn_consumer_task(
            "route-immediate-err".to_string(),
            Box::new(FailingConsumer::new("boom")),
            ctx,
            None,
            Some(runtime_weak),
            false,
        );

        // Controller gets pre-resolved receiver — instant Ok.
        let result = tokio::time::timeout(Duration::from_millis(100), startup_rx.await_ready())
            .await
            .expect("immediate receiver must not block");
        assert!(result.is_ok(), "immediate receiver resolves Ok");

        // Spawn the watcher.
        let inputs = watcher_inputs.expect("Immediate consumer must yield watcher inputs");
        let watcher = spawn_failure_watcher(inputs);

        // Wait for both tasks to complete.
        handle.await.expect("consumer task must join");
        let _ = tokio::time::timeout(Duration::from_secs(2), watcher).await;

        // The watcher issued exactly one FailRoute.
        let cmds = recorder.lock().expect("lock");
        assert_eq!(cmds.len(), 1, "expected exactly one FailRoute from watcher");
        match &cmds[0] {
            RuntimeCommand::FailRoute {
                route_id, error, ..
            } => {
                assert_eq!(route_id, "route-immediate-err");
                assert!(error.contains("boom"));
            }
            other => panic!("expected FailRoute, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn immediate_prompt_ok_watcher_exits_early() {
        // A spawn-and-return Immediate consumer (start() returns Ok):
        // the watcher exits early via the ok latch, no FailRoute issued.
        use camel_api::RuntimeCommand;
        use std::sync::Mutex;

        let recorder: Arc<Mutex<Vec<RuntimeCommand>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder_clone = Arc::clone(&recorder);

        struct ImmediateOkConsumer;
        #[async_trait]
        impl Consumer for ImmediateOkConsumer {
            async fn start(&mut self, _ctx: ConsumerContext) -> Result<(), CamelError> {
                Ok(())
            }
            async fn stop(&mut self) -> Result<(), CamelError> {
                Ok(())
            }
        }

        let runtime: Arc<dyn RuntimeHandle> = Arc::new(RecordingRuntime {
            recorder: recorder_clone,
            reject: false,
        });
        let runtime_weak = Arc::downgrade(&runtime);

        let (tx, _rx) = mpsc::channel(1);
        let ctx = ConsumerContext::new(tx, CancellationToken::new(), "immediate-ok".to_string());

        let (handle, startup_rx, watcher_inputs, _outer_inputs) = spawn_consumer_task(
            "route-immediate-ok".to_string(),
            Box::new(ImmediateOkConsumer),
            ctx,
            None,
            Some(runtime_weak),
            false,
        );

        // Controller gets pre-resolved receiver.
        let result = tokio::time::timeout(Duration::from_millis(100), startup_rx.await_ready())
            .await
            .expect("immediate receiver must not block");
        assert!(result.is_ok());

        // Spawn the watcher.
        let inputs = watcher_inputs.expect("Immediate consumer must yield watcher inputs");
        let watcher = spawn_failure_watcher(inputs);

        // Watcher should exit quickly (ok latch fires).
        let watcher_result = tokio::time::timeout(Duration::from_secs(1), watcher).await;
        assert!(
            watcher_result.is_ok(),
            "watcher must exit within 1s on ok latch"
        );

        // No FailRoute was issued.
        {
            let cmds = recorder.lock().expect("lock");
            assert!(
                cmds.is_empty(),
                "no FailRoute expected on ok path, got {cmds:?}"
            );
        }

        handle.await.expect("consumer task must join");
    }

    #[tokio::test]
    async fn immediate_loop_style_watcher_exits_after_grace() {
        // A loop-style Immediate consumer (start() runs until cancellation):
        // the watcher exits after the grace elapses, no FailRoute issued.
        // Uses the injected-grace seam (run_failure_watcher with 10ms).
        use camel_api::RuntimeCommand;
        use std::sync::Mutex;

        struct ImmediateLoopConsumer;
        #[async_trait]
        impl Consumer for ImmediateLoopConsumer {
            async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
                ctx.cancelled().await;
                Ok(())
            }
            async fn stop(&mut self) -> Result<(), CamelError> {
                Ok(())
            }
        }

        let recorder: Arc<Mutex<Vec<RuntimeCommand>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder_clone = Arc::clone(&recorder);
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(RecordingRuntime {
            recorder: recorder_clone,
            reject: false,
        });
        let runtime_weak = Arc::downgrade(&runtime);

        let cancel = CancellationToken::new();
        let (tx, _rx) = mpsc::channel(1);
        let ctx = ConsumerContext::new(tx, cancel.clone(), "immediate-loop".to_string());

        let (handle, _startup_rx, watcher_inputs, _outer_inputs) = spawn_consumer_task(
            "route-immediate-loop".to_string(),
            Box::new(ImmediateLoopConsumer),
            ctx,
            None,
            Some(runtime_weak),
            false,
        );

        let inputs = watcher_inputs.expect("Immediate consumer must yield watcher inputs");

        // Call run_failure_watcher directly with injected grace (test seam).
        let watcher_future = run_failure_watcher(inputs, Duration::from_millis(10));
        let watcher_handle = tokio::spawn(watcher_future);

        // Watcher should exit within grace + bound.
        let result = tokio::time::timeout(Duration::from_secs(1), watcher_handle).await;
        assert!(
            result.is_ok(),
            "watcher must exit within 1s on grace elapse"
        );

        // No FailRoute was issued.
        {
            let cmds = recorder.lock().expect("lock");
            assert!(
                cmds.is_empty(),
                "no FailRoute expected on grace path, got {cmds:?}"
            );
        }

        cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
    }

    #[tokio::test]
    async fn immediate_biased_error_wins_over_grace() {
        // A settled error latch must never be discarded by an
        // already-expired grace timer: biased ordering polls
        // err > ok > grace deterministically.
        let (senders, latches) = ImmediateLatches::pair();
        senders.returned_err("late boom".to_string());

        let result = latches.wait(Duration::from_millis(0)).await;
        let msg = result.expect_err("settled error must win over a zero grace");
        assert!(
            msg.contains("late boom"),
            "error text must carry through: {msg}"
        );
    }

    /// Runtime fake with a scripted accept/reject plan: rejects the first
    /// `reject_first` execute() attempts, accepts afterwards.
    struct ScriptedRuntime {
        recorder: Arc<std::sync::Mutex<Vec<RuntimeCommand>>>,
        reject_first: std::sync::atomic::AtomicUsize,
    }

    impl ScriptedRuntime {
        fn rejecting_first(
            reject_first: usize,
            recorder: Arc<std::sync::Mutex<Vec<RuntimeCommand>>>,
        ) -> Self {
            Self {
                recorder,
                reject_first: std::sync::atomic::AtomicUsize::new(reject_first),
            }
        }

        fn accepts_all(recorder: Arc<std::sync::Mutex<Vec<RuntimeCommand>>>) -> Self {
            Self::rejecting_first(0, recorder)
        }
    }

    #[async_trait::async_trait]
    impl camel_api::RuntimeCommandBus for ScriptedRuntime {
        async fn execute(&self, cmd: RuntimeCommand) -> Result<RuntimeCommandResult, CamelError> {
            self.recorder.lock().expect("lock").push(cmd.clone());
            if self.reject_first.load(std::sync::atomic::Ordering::SeqCst) > 0 {
                self.reject_first
                    .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                Err(CamelError::RouteError("rejected".into()))
            } else {
                Ok(RuntimeCommandResult::Accepted)
            }
        }
    }

    #[async_trait::async_trait]
    impl camel_api::RuntimeQueryBus for ScriptedRuntime {
        async fn ask(
            &self,
            _query: camel_api::RuntimeQuery,
        ) -> Result<camel_api::RuntimeQueryResult, CamelError> {
            Ok(camel_api::RuntimeQueryResult::RouteStatus {
                route_id: "test".into(),
                status: "Started".into(),
            })
        }
    }

    /// Shared-buffer MakeWriter so a fmt layer can capture subscriber
    /// output across awaits (set_default guard idiom — same mechanism as
    /// `test_crash_notification_warns_on_closed_channel` above; the
    /// current_thread test runtime polls every task on this thread).
    struct SharedBufWriter {
        buf: Arc<std::sync::Mutex<Vec<u8>>>,
    }

    impl Clone for SharedBufWriter {
        fn clone(&self) -> Self {
            Self {
                buf: Arc::clone(&self.buf),
            }
        }
    }

    impl std::io::Write for SharedBufWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.buf
                .lock()
                .expect("lock log buf")
                .extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedBufWriter {
        type Writer = SharedBufWriter;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn captured_logs() -> (Arc<std::sync::Mutex<Vec<u8>>>, SharedBufWriter) {
        let buf = Arc::new(std::sync::Mutex::new(Vec::new()));
        (Arc::clone(&buf), SharedBufWriter { buf })
    }

    fn captured_text(buf: &Arc<std::sync::Mutex<Vec<u8>>>) -> String {
        String::from_utf8(buf.lock().expect("lock log buf").clone())
            .expect("captured output must be UTF-8")
    }

    #[tokio::test]
    async fn watcher_retries_after_rejected_failroute() {
        // Spec: one defensive retry — the PRIMARY FailRoute attempt is
        // rejected, the retry (SAME command_id) is accepted; exactly two
        // recorded attempts, final success (no exhaustion log).
        use tracing_subscriber::prelude::*;

        let (log_buf, writer) = captured_logs();
        let _guard = tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(writer)
                    .with_ansi(false),
            )
            .set_default();

        let recorder: Arc<std::sync::Mutex<Vec<RuntimeCommand>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let runtime: Arc<dyn RuntimeHandle> =
            Arc::new(ScriptedRuntime::rejecting_first(1, Arc::clone(&recorder)));
        let runtime_weak = Arc::downgrade(&runtime);

        let (tx, _rx) = mpsc::channel(1);
        let ctx = ConsumerContext::new(tx, CancellationToken::new(), "retry-route".to_string());

        let (handle, _startup_rx, watcher_inputs, _outer_inputs) = spawn_consumer_task(
            "route-retry".to_string(),
            Box::new(FailingConsumer::new("boom")),
            ctx,
            None,
            Some(runtime_weak),
            false,
        );

        let inputs = watcher_inputs.expect("Immediate consumer must yield watcher inputs");
        let watcher = spawn_failure_watcher(inputs);

        handle.await.expect("consumer task must join");
        let _ = tokio::time::timeout(Duration::from_secs(2), watcher)
            .await
            .expect("watcher must terminate after the accepted retry");

        let cmds = recorder.lock().expect("lock");
        assert_eq!(cmds.len(), 2, "exactly primary + one retry, got {cmds:?}");
        let (id_a, id_b) = match (&cmds[0], &cmds[1]) {
            (
                RuntimeCommand::FailRoute { command_id: a, .. },
                RuntimeCommand::FailRoute { command_id: b, .. },
            ) => (a.clone(), b.clone()),
            other => panic!("expected two FailRoute commands, got {other:?}"),
        };
        assert_eq!(id_a, id_b, "retry must reuse the SAME command_id");

        let text = captured_text(&log_buf);
        assert!(
            !text.contains("exhausted retries"),
            "accepted retry must not log exhaustion, captured: {text}"
        );
    }

    #[tokio::test]
    async fn watcher_logs_after_exhaustion() {
        // Spec: bounded exhaustion — both attempts rejected; exactly two
        // attempts, the system-broken error! naming the route, watcher
        // terminates (handle joins).
        use tracing_subscriber::prelude::*;

        let (log_buf, writer) = captured_logs();
        let _guard = tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(writer)
                    .with_ansi(false),
            )
            .set_default();

        let recorder: Arc<std::sync::Mutex<Vec<RuntimeCommand>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(ScriptedRuntime::rejecting_first(
            usize::MAX,
            Arc::clone(&recorder),
        ));
        let runtime_weak = Arc::downgrade(&runtime);

        let (tx, _rx) = mpsc::channel(1);
        let ctx = ConsumerContext::new(tx, CancellationToken::new(), "exhaust-route".to_string());

        let (handle, _startup_rx, watcher_inputs, _outer_inputs) = spawn_consumer_task(
            "route-exhaust".to_string(),
            Box::new(FailingConsumer::new("boom")),
            ctx,
            None,
            Some(runtime_weak),
            false,
        );

        let inputs = watcher_inputs.expect("Immediate consumer must yield watcher inputs");
        let watcher = spawn_failure_watcher(inputs);

        handle.await.expect("consumer task must join");
        let _ = tokio::time::timeout(Duration::from_secs(2), watcher)
            .await
            .expect("watcher must terminate after exhausting its single retry");

        let cmds = recorder.lock().expect("lock");
        assert_eq!(
            cmds.len(),
            2,
            "exactly primary + single retry even under total rejection, got {cmds:?}"
        );

        let text = captured_text(&log_buf);
        assert!(
            text.contains("exhausted retries") && text.contains("route-exhaust"),
            "system-broken error! must name the route and the un-projected failure, captured: {text}"
        );
    }

    #[tokio::test]
    async fn watcher_no_runtime_terminates_bounded() {
        // Spec: degenerate runtime — inputs.runtime missing → no FailRoute
        // attempted, abort+cancel still run, system-broken error!, and the
        // watcher joins within the 1s outer bound.
        use tracing_subscriber::prelude::*;

        let (log_buf, writer) = captured_logs();
        let _guard = tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(writer)
                    .with_ansi(false),
            )
            .set_default();

        // The recorder exists (a live bus), but the watcher's inputs lose
        // the handle — "NO FailRoute attempted" is observable, not vacuous.
        let recorder: Arc<std::sync::Mutex<Vec<RuntimeCommand>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let runtime: Arc<dyn RuntimeHandle> =
            Arc::new(ScriptedRuntime::accepts_all(Arc::clone(&recorder)));
        let runtime_weak = Arc::downgrade(&runtime);

        let (tx, _rx) = mpsc::channel(1);
        let ctx = ConsumerContext::new(tx, CancellationToken::new(), "nort-route".to_string());

        let (handle, _startup_rx, watcher_inputs, _outer_inputs) = spawn_consumer_task(
            "route-no-runtime".to_string(),
            Box::new(FailingConsumer::new("boom")),
            ctx,
            None,
            Some(runtime_weak),
            false,
        );

        let mut inputs = watcher_inputs.expect("Immediate consumer must yield watcher inputs");
        inputs.runtime = None; // simulate the missing handle

        let cancel = inputs.consumer_cancel.clone();
        let watcher = spawn_failure_watcher(inputs);

        handle.await.expect("consumer task must join");
        let _ = tokio::time::timeout(Duration::from_secs(1), watcher)
            .await
            .expect("watcher must terminate within the 1s bound without a runtime handle");

        assert!(
            recorder.lock().expect("lock").is_empty(),
            "no FailRoute may be attempted without a runtime handle"
        );
        assert!(
            cancel.is_cancelled(),
            "the watcher's cancel must still fire (abort+cancel parity)"
        );

        let text = captured_text(&log_buf);
        assert!(
            text.contains("runtime handle is missing") && text.contains("route-no-runtime"),
            "system-broken error! must name the route and the missing handle, captured: {text}"
        );
    }
}

#[cfg(test)]
#[path = "handshake_tests.rs"]
mod handshake_tests;
