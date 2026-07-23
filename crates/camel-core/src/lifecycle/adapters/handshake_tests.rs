//! Tests for the rc-w1u9 consumer startup handshake (Option E).
//!
//! Covers `ConsumerStartupMode` integration with `spawn_consumer_task`:
//! - Immediate consumers get a pre-resolved receiver (fire-and-forget).
//! - Explicit consumers' receiver resolves only after `ctx.mark_ready()`.
//! - Explicit consumers whose `start()` returns `Err` surface the error.
//! - Explicit consumers that return `Ok` without signalling are rescued by
//!   the defensive fallback (no controller hang; warn! emitted).

use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Notify;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// `Immediate` consumer whose `start()` blocks on cancellation, mimicking
/// timer/file/direct consumers whose start IS the lifetime loop.
struct ImmediateLifetimeConsumer {
    started: Arc<AtomicBool>,
}

#[async_trait]
impl Consumer for ImmediateLifetimeConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        self.started.store(true, Ordering::SeqCst);
        ctx.cancelled().await;
        Ok(())
    }
    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
}

/// `Explicit` consumer that simulates bind+ready then waits on cancellation,
/// matching the HttpConsumer shape.
///
/// `entered` is a `Notify` signalled as soon as `start()` is entered, so the
/// test can deterministically observe "consumer is in `start()` but has not
/// yet bound" without relying on a fixed `sleep` (review M-4).
struct ExplicitReadyConsumer {
    ready_after: Duration,
    started: Arc<AtomicBool>,
    entered: Arc<Notify>,
    mark_ready_called: Arc<AtomicBool>,
}

#[async_trait]
impl Consumer for ExplicitReadyConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        self.started.store(true, Ordering::SeqCst);
        // Signal test: we have entered start() but not yet bound.
        self.entered.notify_one();
        // Simulate resource bind (e.g. TcpListener::bind) taking some time.
        tokio::time::sleep(self.ready_after).await;
        ctx.mark_ready();
        self.mark_ready_called.store(true, Ordering::SeqCst);
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

/// `Explicit` consumer whose simulated bind fails — its `start()` returns
/// Err WITHOUT calling `mark_ready`. The runtime MUST surface this as a
/// startup failure on the StartupReceiver.
struct ExplicitBindFailConsumer;

#[async_trait]
impl Consumer for ExplicitBindFailConsumer {
    async fn start(&mut self, _ctx: ConsumerContext) -> Result<(), CamelError> {
        // Simulate bind failure without ever calling mark_ready.
        Err(CamelError::RouteError(
            "simulated bind failure: Address already in use".to_string(),
        ))
    }
    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
    fn startup_mode(&self) -> ConsumerStartupMode {
        ConsumerStartupMode::Explicit
    }
}

/// `Explicit` consumer that returns Ok WITHOUT calling `mark_ready` —
/// a contract violation. The defensive fallback in `spawn_consumer_task`
/// must resolve the receiver so the controller does not hang (review M-2).
struct ExplicitOkNoMarkReadyConsumer;

#[async_trait]
impl Consumer for ExplicitOkNoMarkReadyConsumer {
    async fn start(&mut self, _ctx: ConsumerContext) -> Result<(), CamelError> {
        // Contract violation: return Ok without ever calling mark_ready.
        Ok(())
    }
    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
    fn startup_mode(&self) -> ConsumerStartupMode {
        ConsumerStartupMode::Explicit
    }
}

#[tokio::test]
async fn spawn_consumer_task_immediate_consumer_returns_resolved_receiver() {
    // Immediate consumers must NOT block the controller — the receiver
    // returned by spawn_consumer_task must be already resolved as Ok
    // even though the consumer's start() loop has not yet signalled
    // anything (and never will — Immediate consumers don't call
    // mark_ready).
    let (tx, _rx) = mpsc::channel(1);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), "immediate-route".to_string());

    let started = Arc::new(AtomicBool::new(false));
    let consumer = ImmediateLifetimeConsumer {
        started: Arc::clone(&started),
    };

    let (handle, startup_rx) = spawn_consumer_task(
        "immediate-route".to_string(),
        Box::new(consumer),
        ctx,
        None,
        None,
        false,
    );

    // The receiver MUST resolve Ok without waiting on the consumer's
    // lifetime loop. Bounded timeout proves non-blocking.
    let result = tokio::time::timeout(Duration::from_millis(200), startup_rx.await_ready())
        .await
        .expect("immediate startup receiver must not block");
    assert!(result.is_ok(), "immediate receiver resolves Ok");

    // Sanity: the consumer did run.
    for _ in 0..50 {
        if started.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        started.load(Ordering::SeqCst),
        "immediate consumer should have entered start()"
    );

    // Cleanup.
    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

#[tokio::test]
async fn spawn_consumer_task_explicit_consumer_waits_for_mark_ready() {
    // Explicit consumer's receiver must NOT resolve until the consumer
    // calls ctx.mark_ready(). This proves the controller's await will
    // observe "after-bind" state, not "spawn-returned" state.
    let (tx, _rx) = mpsc::channel(1);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), "explicit-route".to_string());

    let started = Arc::new(AtomicBool::new(false));
    let mark_ready_called = Arc::new(AtomicBool::new(false));
    let entered = Arc::new(Notify::new());
    let consumer = ExplicitReadyConsumer {
        ready_after: Duration::from_millis(40),
        started: Arc::clone(&started),
        entered: Arc::clone(&entered),
        mark_ready_called: Arc::clone(&mark_ready_called),
    };

    let (handle, startup_rx) = spawn_consumer_task(
        "explicit-route".to_string(),
        Box::new(consumer),
        ctx,
        None,
        None,
        false,
    );

    // Wait deterministically for the consumer to ENTER start() (review M-4:
    // previously this was a fixed 10ms sleep that could race the scheduler
    // under loaded CI). At this point mark_ready has NOT yet been called
    // (it fires `ready_after` later).
    entered.notified().await;
    assert!(
        started.load(Ordering::SeqCst),
        "consumer should have entered start() before ready signal"
    );
    assert!(
        !mark_ready_called.load(Ordering::SeqCst),
        "mark_ready must not have fired yet — receiver should still be pending"
    );

    // Now await — must resolve Ok AND mark_ready_called must be true.
    let result = tokio::time::timeout(Duration::from_secs(2), startup_rx.await_ready())
        .await
        .expect("explicit receiver must resolve after mark_ready");
    assert!(
        result.is_ok(),
        "explicit receiver resolves Ok after mark_ready"
    );
    assert!(
        mark_ready_called.load(Ordering::SeqCst),
        "mark_ready must have been called before receiver resolved"
    );

    // Cleanup.
    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

#[tokio::test]
async fn spawn_consumer_task_explicit_consumer_start_error_propagates() {
    // Explicit consumer whose bind fails: start() returns Err without
    // calling mark_ready. The StartupReceiver must surface the error
    // (previously this was a silent background log — rc-w1u9 fix).
    let (tx, _rx) = mpsc::channel(1);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel, "explicit-fail-route".to_string());

    let (handle, startup_rx) = spawn_consumer_task(
        "explicit-fail-route".to_string(),
        Box::new(ExplicitBindFailConsumer),
        ctx,
        None,
        None,
        false,
    );

    let result = tokio::time::timeout(Duration::from_secs(2), startup_rx.await_ready())
        .await
        .expect("explicit receiver must resolve (with Err) on start failure");
    let err = result.expect_err("start() Err must propagate to receiver");
    match err {
        CamelError::RouteError(msg) => assert!(
            msg.contains("simulated bind failure"),
            "error message must carry the consumer's failure: got {msg}"
        ),
        other => panic!("expected RouteError, got {other:?}"),
    }

    // Consumer task must complete.
    handle
        .await
        .expect("consumer task must join after start failure");
}

#[tokio::test]
async fn spawn_consumer_task_explicit_consumer_ok_without_mark_ready_does_not_hang_controller() {
    // rc-w1u9 review M-2: An Explicit consumer that returns Ok without
    // calling ctx.mark_ready() is a contract violation. The defensive
    // fallback in spawn_consumer_task MUST resolve the receiver so the
    // controller does not hang forever. The fallback also emits a warn!
    // (verified separately by code review of consumer_management.rs).
    let (tx, _rx) = mpsc::channel(1);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), "explicit-no-mark-ready".to_string());

    let (handle, startup_rx) = spawn_consumer_task(
        "explicit-no-mark-ready".to_string(),
        Box::new(ExplicitOkNoMarkReadyConsumer),
        ctx,
        None,
        None,
        false,
    );

    // Bounded wait: if the defensive fallback regresses, this hangs and
    // the test times out (instead of the controller hanging in prod).
    let result = tokio::time::timeout(Duration::from_secs(2), startup_rx.await_ready())
        .await
        .expect("defensive fallback must resolve receiver (no controller hang)");
    assert!(
        result.is_ok(),
        "defensive fallback must resolve receiver as Ok"
    );

    // Cleanup.
    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

/// Explicit consumer that defers `mark_ready` to a background task.
struct ExplicitDeferredMarkReadyConsumer {
    bg_handle: Option<tokio::task::JoinHandle<Result<(), CamelError>>>,
}

#[async_trait]
impl Consumer for ExplicitDeferredMarkReadyConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        let signal = ctx.startup_signal();
        let cancel = ctx.cancel_token();
        self.bg_handle = Some(tokio::spawn(async move {
            tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
            signal.mark_ready();
            cancel.cancelled().await;
            Ok(())
        }));
        Ok(())
    }
    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
    fn startup_mode(&self) -> ConsumerStartupMode {
        ConsumerStartupMode::Explicit
    }
    fn background_task_handle(
        &mut self,
    ) -> Option<tokio::task::JoinHandle<Result<(), CamelError>>> {
        self.bg_handle.take()
    }
}

#[tokio::test]
async fn spawn_consumer_task_explicit_deferred_mark_ready_not_defeated_by_fallback() {
    // rc-gu5n: Explicit consumer with a background task defers mark_ready
    // to the spawned task. The defensive fallback MUST be suppressed.
    let (tx, _rx) = mpsc::channel(1);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), "deferred-ready".to_string());

    let consumer = ExplicitDeferredMarkReadyConsumer { bg_handle: None };

    let (handle, startup_rx) = spawn_consumer_task(
        "deferred-ready".to_string(),
        Box::new(consumer),
        ctx,
        None,
        None,
        false,
    );

    // Receiver must NOT resolve before bg task calls mark_ready.
    // Measure elapsed time: fallback would resolve in <1ms; deferred
    // mark_ready takes ~50ms (bg task sleep). Assert >20ms.
    let start = std::time::Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(2), startup_rx.await_ready())
        .await
        .expect("bg task must call mark_ready");
    let elapsed = start.elapsed();
    assert!(result.is_ok(), "deferred mark_ready must resolve Ok");
    assert!(
        elapsed >= Duration::from_millis(20),
        "receiver resolved in {elapsed:?} — defensive fallback likely fired prematurely"
    );

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}
