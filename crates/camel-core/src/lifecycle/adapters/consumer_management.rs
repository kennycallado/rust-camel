use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use camel_api::{CamelError, RuntimeHandle, StepLifecycle, StepShutdownReason};
use camel_component_api::{
    ComponentContext, ConcurrencyModel, Consumer, ConsumerContext, ConsumerStartupMode,
    RuntimeObservability, StartupReceiver, StartupSignal,
};
use camel_endpoint::parse_uri;

use crate::lifecycle::adapters::route_helpers::{
    CrashNotification, ManagedRoute, handle_is_running, publish_runtime_failure,
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

pub(crate) fn spawn_consumer_task(
    route_id: String,
    mut consumer: Box<dyn Consumer>,
    consumer_ctx: ConsumerContext,
    crash_notifier: Option<mpsc::Sender<CrashNotification>>,
    runtime_for_consumer: Option<Weak<dyn RuntimeHandle>>,
    is_resume: bool,
) -> (JoinHandle<()>, StartupReceiver) {
    // rc-w1u9: Explicit consumers (HTTP, WebSocket, …) signal readiness via
    // ctx.mark_ready() AFTER binding their resource. Immediate consumers
    // keep the original fire-and-forget semantics via a pre-resolved
    // receiver, so `start_route` can uniformly await the receiver without
    // changing behaviour for timer/file/direct/… consumers.
    let (startup_signal, startup_receiver) = match consumer.startup_mode() {
        ConsumerStartupMode::Immediate => {
            let immediate = StartupReceiver::immediate();
            // The signal below is never used by the consumer (Immediate
            // consumers don't call mark_ready), but we still install a
            // fresh pair so the context has something valid to carry.
            let (signal, _drop_rx) = StartupSignal::pair();
            let _ = _drop_rx;
            (signal, immediate)
        }
        ConsumerStartupMode::Explicit => {
            let (signal, receiver) = StartupSignal::pair();
            (signal, receiver)
        }
    };
    let startup_for_task = startup_signal.clone();
    let consumer_ctx = consumer_ctx.with_startup(startup_signal);

    let handle = tokio::spawn(async move {
        let result = consumer.start(consumer_ctx.clone()).await;
        if let Err(e) = &result {
            // Propagate the failure to any controller awaiting startup.
            // For Immediate consumers this is a no-op (their signal is
            // never awaited); for Explicit consumers it surfaces the bind
            // failure as a proper startup error instead of a silent
            // background log (rc-w1u9).
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

            // H9: clean up any resources the consumer created during the
            // failed start before dropping it.
            let _ = consumer.stop().await;
            return;
        }

        // Consumer started successfully. If it detached a background task,
        // monitor the handle for unexpected exits (crash propagation per
        // ADR-0007).
        let bg_handle = consumer.background_task_handle();

        // rc-w1u9: Defensive fallback. An Explicit consumer whose `start()`
        // returned Ok without ever calling `mark_ready` would otherwise hang
        // the route controller on the startup receiver. If the consumer is
        // done (Ok returned), it is definitionally ready — surface that to
        // any pending awaiter. No-op for Immediate consumers (already Ready)
        // and for Explicit consumers that correctly called mark_ready.
        //
        // mark_ready returns true ONLY when the state was still Pending —
        // i.e. the consumer violated the Explicit contract. Warn loudly so
        // the regression is not silently papered over (review I-1).
        //
        // rc-gu5n: Skip the fallback when the consumer has a background task
        // handle — these consumers (CXF, Redis, Kafka) intentionally defer
        // mark_ready to the spawned task where the resource bind happens.
        // The fallback would fire before the task runs, defeating the
        // handshake and logging a false "contract violation" warning.
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
                            publish_runtime_failure(runtime_for_consumer, &route_id, &error_msg).await;
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
                            publish_runtime_failure(runtime_for_consumer, &route_id, &error_msg).await;
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
        } else {
            // Inline consumer: start() returned, meaning the consumer's run
            // loop completed naturally or the context was cancelled mid-run.
            // No need to wait — just proceed to stop().
        }

        // "finally" — always call stop() after start() succeeds
        let _ = consumer.stop().await;
    });

    (handle, startup_receiver)
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
    use camel_api::SyncBoxProcessor;
    use camel_api::{BoxProcessor, IdentityProcessor};
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

        let (handle, _startup_rx) = spawn_consumer_task(
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

        let (handle, _startup_rx) = spawn_consumer_task(
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

        let (handle, _startup_rx) = spawn_consumer_task(
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

        let (handle, _startup_rx) = spawn_consumer_task(
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

        let (handle, _startup_rx) = spawn_consumer_task(
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

        let (handle, _startup_rx) = spawn_consumer_task(
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
}

#[cfg(test)]
#[path = "handshake_tests.rs"]
mod handshake_tests;
