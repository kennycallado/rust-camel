use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{Semaphore, TryAcquireError};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tower::{Service, ServiceExt};

use camel_api::{CamelError, Exchange, StepLifecycle, StepShutdownReason};

/// Configuration for [`WireTapService`].
///
/// Default concurrency bound is 20 (Camel-faithful flat-semaphore).
/// `shutdown_grace` defaults to 5 seconds.
#[derive(Clone)]
pub struct WireTapConfig {
    /// Maximum number of concurrent tap tasks. `None` means unlimited.
    pub max_concurrent: Option<usize>,
    /// Grace period for in-flight tap tasks to complete on shutdown.
    /// A value of zero means "skip drain, cancel immediately".
    pub shutdown_grace: std::time::Duration,
}

impl Default for WireTapConfig {
    fn default() -> Self {
        Self {
            max_concurrent: Some(20),
            shutdown_grace: std::time::Duration::from_secs(5),
        }
    }
}

impl WireTapConfig {
    /// Validate the config, panicking on invalid states.
    ///
    /// `shutdown_grace` of zero is valid (means "skip drain, cancel immediately").
    pub fn validate(&self) {
        if self.max_concurrent == Some(0) {
            panic!("max_concurrent must be > 0 when set");
        }
    }

    /// Create a config with a bounded concurrency limit.
    pub fn bounded(max_concurrent: usize) -> Self {
        assert!(max_concurrent > 0, "max_concurrent must be > 0");
        Self {
            max_concurrent: Some(max_concurrent),
            shutdown_grace: std::time::Duration::from_secs(5),
        }
    }
}

/// Mutable admission-gate state guarded by [`WireTapShared::inner`].
///
/// The `Mutex` over this struct serializes the "check `open` → register task"
/// critical section so a `shutdown()` racing with a `call()` cannot orphan a
/// task: either `call()` registers under the lock (and `shutdown` drains it via
/// `tracker.wait`), or `shutdown` closes admission first (and `call()` returns
/// early without registering). There is no `await` point while the lock is held.
#[derive(Debug)]
struct WireTapSharedInner {
    open: bool,
    tracker: TaskTracker,
    cancel: CancellationToken,
    semaphore: Option<Arc<Semaphore>>,
    shutdown_grace: Duration,
}

/// Shared admission gate, liveness tracker, and cancellation token for a
/// [`WireTapService`] and its clones.
///
/// All clones of a `WireTapService` share the SAME `Arc<WireTapShared>`: per-
/// request clones drop an `Arc` ref but do NOT close admission or cancel taps.
/// Only the last-ref drop (canonical-service teardown) fires [`Drop`], which
/// cancels every in-flight tap. This is defense-in-depth alongside the runtime-
/// driven `StepLifecycle::shutdown` path (ADR-0022 mandates shutdown-before-drop,
/// but Drop guarantees cleanup if the runtime fails to call shutdown).
#[derive(Debug)]
struct WireTapShared {
    inner: Mutex<WireTapSharedInner>,
}

impl Drop for WireTapShared {
    fn drop(&mut self) {
        // Defense-in-depth for the cancel-and-drain contract: when the last
        // `Arc<WireTapShared>` ref drops (canonical-service teardown), cancel
        // every in-flight tap so spawned tasks unwind promptly via the
        // `cancel.cancelled()` select branch in `run_tap`. The runtime calls
        // `StepLifecycle::shutdown` before drop per ADR-0022, but Drop
        // guarantees cleanup if it does not.
        //
        // Cancel BEFORE the inner fields drop: once `cancel.cancel()` fires the
        // cancellation state is latched into the token (and all clones held by
        // in-flight tasks), so the subsequent `CancellationToken::drop` and
        // `TaskTracker::drop` (which detaches rather than aborts) do not race
        // the cancellation signal.
        self.inner
            .lock()
            .expect("WireTapShared mutex poisoned") // allow-unwrap
            .cancel
            .cancel();
    }
}

pub struct WireTapService {
    tap_endpoint: camel_api::BoxProcessor,
    shared: Arc<WireTapShared>,
}

// The shared admission gate, liveness tracker, and cancellation token live in
// `Arc<WireTapShared>`: each clone gets a new ref to the SAME shared state.
// This is required because the route pipeline clones the `BoxProcessor` per
// request (`BoxCloneService` contract) and drops the clone once `call()`'s
// immediate-return future resolves. Per-clone state would close admission on
// every request drop. Sharing keeps the gate open until canonical teardown.
impl Clone for WireTapService {
    fn clone(&self) -> Self {
        Self {
            tap_endpoint: self.tap_endpoint.clone(),
            shared: Arc::clone(&self.shared),
        }
    }
}

impl WireTapService {
    /// Create a new `WireTapService` with default (bounded-20) concurrency.
    pub fn new(tap_endpoint: camel_api::BoxProcessor) -> Self {
        Self::with_config(tap_endpoint, WireTapConfig::default())
    }

    /// Create a new `WireTapService` from a [`WireTapConfig`].
    pub fn with_config(tap_endpoint: camel_api::BoxProcessor, config: WireTapConfig) -> Self {
        config.validate();
        let semaphore = config
            .max_concurrent
            .map(|limit| Arc::new(Semaphore::new(limit)));
        let shared = Arc::new(WireTapShared {
            inner: Mutex::new(WireTapSharedInner {
                open: true,
                tracker: TaskTracker::new(),
                cancel: CancellationToken::new(),
                semaphore,
                shutdown_grace: config.shutdown_grace,
            }),
        });
        Self {
            tap_endpoint,
            shared,
        }
    }

    /// Test-only accessor for the count of currently-tracked (detached) tap
    /// tasks. This counts ONLY detached tasks registered with the
    /// [`TaskTracker`]; the inline CallerRuns path is NOT counted. Therefore
    /// the bound invariant (`bound + 1` total concurrent execution, where the
    /// +1 is the inline tap) is not observable through this accessor.
    #[cfg(test)]
    pub(crate) fn in_flight_count(&self) -> usize {
        self.shared
            .inner
            .lock()
            .expect("WireTapShared mutex poisoned") // allow-unwrap
            .tracker
            .len()
    }
}

/// A lifecycle handle for a [`WireTapService`] implementing graceful-drain-
/// then-abort teardown via [`StepLifecycle::shutdown`].
///
/// Obtain via [`WireTapService::lifecycle`]. The handle shares the same
/// [`Arc<WireTapShared>`] as the service, so `shutdown` observes the live
/// admission gate and task tracker.
#[derive(Debug)]
pub struct WireTapLifecycle {
    shared: Arc<WireTapShared>,
    shutdown_called: AtomicBool,
}

#[async_trait]
impl StepLifecycle for WireTapLifecycle {
    fn name(&self) -> &'static str {
        "wiretap"
    }

    async fn shutdown(&self, _reason: StepShutdownReason) -> Result<(), CamelError> {
        // Idempotency gate.
        if self.shutdown_called.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        // (a) Close admission, close the tracker, clone handles out of the lock.
        let (tracker, cancel, grace) = {
            let mut guard = self
                .shared
                .inner
                .lock()
                .expect("WireTapShared mutex poisoned"); // allow-unwrap
            guard.open = false;
            guard.tracker.close();
            (
                guard.tracker.clone(),
                guard.cancel.clone(),
                guard.shutdown_grace,
            )
            // MutexGuard dropped here — NO .await held across the guard.
        };

        // (b) If zero grace, skip drain — go straight to cancel.
        // (c) DRAIN FIRST: await in-flight taps that complete naturally
        //     within the grace period.
        if !grace.is_zero() {
            let _ = tokio::time::timeout(grace, tracker.wait()).await;
        }

        // (d) CANCEL: abort any stragglers that did not drain within grace.
        cancel.cancel();

        // (e) Await cancel-completions (tasks that abort on token fire).
        let _ = tracker.wait().await;

        Ok(())
    }
}

impl WireTapService {
    /// Obtain a shared lifecycle handle for this service's admission gate and
    /// task tracker. Callers can invoke [`StepLifecycle::shutdown`] on the
    /// returned handle for graceful-drain-then-abort teardown.
    pub fn lifecycle(&self) -> Arc<dyn StepLifecycle> {
        Arc::new(WireTapLifecycle {
            shared: Arc::clone(&self.shared),
            shutdown_called: AtomicBool::new(false),
        })
    }
}

impl Service<Exchange> for WireTapService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    /// Always ready (ADR-0019): the main route never blocks on tap readiness.
    /// Tap endpoint readiness is driven inside [`run_tap`] on the fire-and-
    /// forgetget path; a tap readiness error is logged and suppressed, never
    /// propagated to the main exchange.
    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let tap_endpoint = self.tap_endpoint.clone();
        let tap_exchange = exchange.clone();

        // Admission critical section: hold the lock across "check open →
        // admit-or-inline decision → register tracked task" so a racing
        // `shutdown` cannot close the tracker between the open-check and the
        // task registration. There is NO await point while the lock is held;
        // `try_acquire_owned`, `tracker.spawn`, and the open-check are all sync.
        let inner = self
            .shared
            .inner
            .lock()
            .expect("WireTapShared mutex poisoned"); // allow-unwrap
        if !inner.open {
            tracing::warn!("WireTap admission closed, dropping tap");
            drop(inner);
            return Box::pin(async move { Ok(exchange) });
        }

        match &inner.semaphore {
            Some(sem) => match Arc::clone(sem).try_acquire_owned() {
                Ok(permit) => {
                    // Admit: register a detached tracked task holding the permit.
                    // The OwnedSemaphorePermit is MOVED into the task body and
                    // lives for the task's lifetime, releasing on completion.
                    let cancel = inner.cancel.clone();
                    inner.tracker.spawn(async move {
                        let _permit = permit;
                        run_tap(tap_endpoint, tap_exchange, cancel).await;
                    });
                    drop(inner);
                    Box::pin(async move { Ok(exchange) })
                }
                Err(TryAcquireError::NoPermits) => {
                    // Saturated: run the tap INLINE on the caller's future. No
                    // permit is acquired or held, so total concurrent execution
                    // transiently reaches `bound + 1` (this inline tap alongside
                    // the `bound` detached permit-holders). The caller is
                    // back-pressured until the inline tap finishes (CallerRuns).
                    let cancel = inner.cancel.clone();
                    drop(inner);
                    Box::pin(async move {
                        run_tap(tap_endpoint, tap_exchange, cancel).await;
                        Ok(exchange)
                    })
                }
                Err(TryAcquireError::Closed) => {
                    tracing::warn!("WireTap semaphore closed, dropping tap");
                    drop(inner);
                    Box::pin(async move { Ok(exchange) })
                }
            },
            None => {
                // Unbounded: register a detached tracked task with no permit.
                let cancel = inner.cancel.clone();
                inner.tracker.spawn(async move {
                    run_tap(tap_endpoint, tap_exchange, cancel).await;
                });
                drop(inner);
                Box::pin(async move { Ok(exchange) })
            }
        }
    }
}

/// Single private helper shared by the detached path and the inline CallerRuns
/// path. Drives the tap endpoint to readiness then calls it, racing against the
/// shared `cancel` token so shutdown/abort unwinds promptly. Tap readiness and
/// processing errors are logged at `warn!` (category handler-owned per
/// ADR-0012) and suppressed — the main exchange proceeds unchanged.
async fn run_tap(
    mut tap_endpoint: camel_api::BoxProcessor,
    tap_exchange: Exchange,
    cancel: CancellationToken,
) {
    // Readiness phase: cancel races against `tap_endpoint.ready()`.
    {
        let ready_fut = tap_endpoint.ready();
        tokio::pin!(ready_fut);
        let ready_result = tokio::select! {
            biased;
            _ = cancel.cancelled() => { return; }
            r = &mut ready_fut => r,
        };
        if let Err(e) = ready_result {
            // log-policy: handler-owned
            tracing::warn!("WireTap endpoint poll_ready failed: {}", e);
            return;
        }
    }
    // Call phase: tap_endpoint is now Ready; cancel races against the call.
    {
        let call_fut = tap_endpoint.call(tap_exchange);
        tokio::pin!(call_fut);
        let call_result = tokio::select! {
            biased;
            _ = cancel.cancelled() => { return; }
            r = &mut call_fut => r,
        };
        if let Err(e) = call_result {
            // log-policy: handler-owned
            tracing::warn!("WireTap processing error: {}", e);
        }
    }
}

/// A Tower layer that produces `WireTapService` instances.
pub struct WireTapLayer {
    tap_endpoint: camel_api::BoxProcessor,
    config: WireTapConfig,
}

impl WireTapLayer {
    /// Create a new WireTapLayer with the given tap endpoint processor (default bounded-20 concurrency).
    pub fn new(tap_endpoint: camel_api::BoxProcessor) -> Self {
        Self {
            tap_endpoint,
            config: WireTapConfig::default(),
        }
    }

    /// Create a new WireTapLayer with bounded concurrency.
    pub fn bounded(tap_endpoint: camel_api::BoxProcessor, max_concurrent: usize) -> Self {
        Self {
            tap_endpoint,
            config: WireTapConfig::bounded(max_concurrent),
        }
    }
}

impl<S> tower::Layer<S> for WireTapLayer {
    type Service = WireTapService;

    fn layer(&self, _inner: S) -> Self::Service {
        WireTapService::with_config(self.tap_endpoint.clone(), self.config.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{BoxProcessor, BoxProcessorExt, Message};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tower::ServiceExt;

    // --- Existing tests retained / adapted to the new shared-state model ---

    #[tokio::test]
    async fn test_wire_tap_returns_original_immediately() {
        let tap_processor = BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }));

        let mut wire_tap = WireTapService::new(tap_processor);
        let exchange = Exchange::new(Message::new("test message"));

        let result = wire_tap
            .ready()
            .await
            .unwrap()
            .call(exchange)
            .await
            .unwrap();

        assert_eq!(result.input.body.as_text(), Some("test message"));
    }

    #[tokio::test]
    async fn test_wire_tap_endpoint_receives_clone() {
        let received_count = Arc::new(AtomicUsize::new(0));
        let count_clone = received_count.clone();

        let tap_processor = BoxProcessor::from_fn(move |ex| {
            let count = count_clone.clone();
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(ex)
            })
        });

        let mut wire_tap = WireTapService::new(tap_processor);
        let exchange = Exchange::new(Message::new("test"));

        let _result = wire_tap
            .ready()
            .await
            .unwrap()
            .call(exchange)
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        assert_eq!(received_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_wire_tap_isolates_errors() {
        let tap_processor = BoxProcessor::from_fn(|_ex| {
            Box::pin(async move { Err(CamelError::ProcessorError("tap error".into())) })
        });

        let mut wire_tap = WireTapService::new(tap_processor);
        let exchange = Exchange::new(Message::new("test"));

        let result = wire_tap.ready().await.unwrap().call(exchange).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().input.body.as_text(), Some("test"));
    }

    #[tokio::test]
    async fn test_wire_tap_layer() {
        use tower::Layer;

        let tap_processor = BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }));

        let layer = super::WireTapLayer::new(tap_processor);
        let inner = camel_api::IdentityProcessor;
        let mut svc = layer.layer(inner);

        let exchange = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(exchange).await.unwrap();

        assert_eq!(result.input.body.as_text(), Some("test"));
    }

    #[tokio::test]
    async fn test_wiretap_bounded_concurrency() {
        // Under the new CallerRuns admission model, when the bound is saturated
        // the next call runs its tap INLINE on the caller's future (without
        // acquiring a permit). The transient peak concurrent execution is
        // therefore `bound + 1` (the inline tap alongside the `bound` detached
        // permit-holders), matching the spec invariant. The old `<= bound`
        // assertion reflected the leaky spawn-then-acquire model.
        let concurrent = Arc::new(AtomicUsize::new(0));
        let max_concurrent = Arc::new(AtomicUsize::new(0));

        let c = Arc::clone(&concurrent);
        let mc = Arc::clone(&max_concurrent);
        let tap_processor = BoxProcessor::from_fn(move |ex| {
            let c = Arc::clone(&c);
            let mc = Arc::clone(&mc);
            Box::pin(async move {
                let current = c.fetch_add(1, Ordering::SeqCst) + 1;
                mc.fetch_max(current, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                c.fetch_sub(1, Ordering::SeqCst);
                Ok(ex)
            })
        });

        let config = super::WireTapConfig::bounded(2);
        let mut svc = super::WireTapService::with_config(tap_processor, config);

        for _ in 0..3 {
            let ex = Exchange::new(Message::new("test"));
            let _ = svc.ready().await.unwrap().call(ex).await.unwrap();
        }

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let observed_max = max_concurrent.load(Ordering::SeqCst);
        // CallerRuns allows `bound + 1` (the inline tap).
        assert!(
            observed_max <= 3,
            "max concurrency was {observed_max}, expected <= bound+1 (=3) under CallerRuns"
        );
    }

    #[tokio::test]
    async fn test_wire_tap_survives_per_request_clone_drop() {
        // Regression for the clone-abort bug (rc-vq91): the route pipeline
        // clones the BoxProcessor per request and drops the clone once call()'s
        // immediate-return future resolves. With per-clone state, that drop
        // would close admission. Sharing `Arc<WireTapShared>` keeps the gate
        // open across clone drops; only the last-ref drop fires cancellation.
        let completed = Arc::new(AtomicUsize::new(0));
        let completed_clone = completed.clone();

        let tap_processor = BoxProcessor::from_fn(move |ex| {
            let c = completed_clone.clone();
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                c.fetch_add(1, Ordering::SeqCst);
                Ok(ex)
            })
        });

        let canonical = WireTapService::new(tap_processor);

        for _ in 0..3 {
            let mut clone = canonical.clone();
            let _ = clone
                .ready()
                .await
                .unwrap()
                .call(Exchange::new(Message::new("req")))
                .await
                .unwrap();
        }

        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while completed.load(Ordering::SeqCst) < 3 {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await;
        assert_eq!(
            completed.load(Ordering::SeqCst),
            3,
            "all tap tasks must complete despite per-request clone drops"
        );
    }

    #[test]
    fn test_wiretap_config_default_is_bounded_20() {
        let cfg = WireTapConfig::default();
        assert_eq!(cfg.max_concurrent, Some(20));
        assert_eq!(cfg.shutdown_grace, std::time::Duration::from_secs(5));
    }

    #[test]
    fn test_wiretap_config_bounded_zero_panics() {
        let result = std::panic::catch_unwind(|| WireTapConfig::bounded(0));
        assert!(result.is_err());
        if let Err(payload) = result {
            let msg = payload
                .downcast_ref::<&str>()
                .expect("panic payload should be &str");
            assert!(
                msg.contains("max_concurrent"),
                "panic message should contain 'max_concurrent', got: {msg}"
            );
        }
    }

    #[test]
    fn test_wiretap_config_validate_rejects_zero_bound() {
        let cfg = WireTapConfig {
            max_concurrent: Some(0),
            shutdown_grace: std::time::Duration::from_secs(5),
        };
        let result = std::panic::catch_unwind(|| cfg.validate());
        assert!(result.is_err());
        let payload = result.unwrap_err();
        let msg = payload
            .downcast_ref::<&str>()
            .expect("panic payload should be &str");
        assert!(
            msg.contains("max_concurrent"),
            "panic message should contain 'max_concurrent', got: {msg}"
        );
    }

    #[tokio::test]
    async fn test_wire_tap_drop_aborts_spawned_tasks() {
        // Under the new shared-state model, dropping the canonical service
        // drops the last `Arc<WireTapShared>` ref, firing `WireTapShared::drop`
        // which cancels the token. The spawned tap's `run_tap` selects on
        // `cancel.cancelled()` and returns promptly, so the 10s sleep is
        // aborted and `task_completed` stays false.
        let task_started = Arc::new(AtomicBool::new(false));
        let task_completed = Arc::new(AtomicBool::new(false));
        let started_clone = task_started.clone();
        let completed_clone = task_completed.clone();

        let tap_processor = BoxProcessor::from_fn(move |_ex| {
            let started = started_clone.clone();
            let completed = completed_clone.clone();
            Box::pin(async move {
                started.store(true, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                completed.store(true, Ordering::SeqCst);
                Ok(Exchange::default())
            })
        });

        let mut service = WireTapService::new(tap_processor);
        let _ = service
            .ready()
            .await
            .unwrap()
            .call(Exchange::default())
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            task_started.load(Ordering::SeqCst),
            "tap task should be running"
        );
        assert!(
            !task_completed.load(Ordering::SeqCst),
            "task should not have completed yet"
        );

        drop(service);

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert!(
            !task_completed.load(Ordering::SeqCst),
            "task should have been aborted, not completed"
        );
    }

    // --- New tests for the bounded-admission + TaskTracker + cancellation model ---

    #[tokio::test]
    async fn test_wiretap_bounded_detached_count_never_exceeds_bound() {
        // Detached tracked count must stay `<= bound`. The inline CallerRuns tap
        // is NOT tracked so it cannot be observed here (transient total
        // execution may briefly reach `bound + 1` — that invariant is exercised
        // by `test_wiretap_bounded_concurrency` above).
        let tap_processor = BoxProcessor::from_fn(|_ex| {
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                Ok(Exchange::default())
            })
        });

        let canonical = WireTapService::with_config(tap_processor, WireTapConfig::bounded(2));
        let max_seen = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));

        // Background sampler: continuously polls in_flight_count() via a shared
        // clone and tracks the peak observed detached task count.
        let sampler_svc = canonical.clone();
        let sampler_max = Arc::clone(&max_seen);
        let sampler_stop = Arc::clone(&stop);
        let sampler = tokio::spawn(async move {
            while !sampler_stop.load(Ordering::SeqCst) {
                let n = sampler_svc.in_flight_count();
                sampler_max.fetch_max(n, Ordering::SeqCst);
                tokio::task::yield_now().await;
            }
        });

        // Fire 5 call() futures from spawned callers under contention.
        let mut callers = Vec::new();
        for _ in 0..5 {
            let mut caller_svc = canonical.clone();
            callers.push(tokio::spawn(async move {
                let _ = caller_svc
                    .ready()
                    .await
                    .unwrap()
                    .call(Exchange::new(Message::new("x")))
                    .await;
            }));
        }
        for h in callers {
            let _ = h.await;
        }

        stop.store(true, Ordering::SeqCst);
        let _ = sampler.await;

        let observed = max_seen.load(Ordering::SeqCst);
        assert!(
            observed <= 2,
            "detached tracked task count peaked at {observed}, expected <= bound (=2)"
        );
    }

    #[tokio::test]
    async fn test_wiretap_caller_backpressured_when_saturated() {
        // CallerRuns: when bound is saturated, the next call's tap runs INLINE
        // on the caller's future. The caller is back-pressured until the inline
        // tap finishes. The leaky spawn-then-acquire version would resolve the
        // call immediately regardless of the tap's progress.
        use tokio::sync::Notify;

        let notify = Arc::new(Notify::new());
        let tap_notify = Arc::clone(&notify);
        let tap_processor = BoxProcessor::from_fn(move |_ex| {
            let n = Arc::clone(&tap_notify);
            Box::pin(async move {
                n.notified().await;
                Ok(Exchange::default())
            })
        });

        let mut svc = WireTapService::with_config(tap_processor, WireTapConfig::bounded(1));

        // First call: acquires the sole permit, spawns detached tap awaiting Notify.
        let _ = svc
            .ready()
            .await
            .unwrap()
            .call(Exchange::default())
            .await
            .unwrap();
        // Yield to let the spawned tap actually register its `notified()` waiter.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // Second call: try_acquire fails (NoPermits). CallerRuns path runs the
        // tap inline on this future, which awaits Notify.
        let mut svc2 = svc.clone();
        let mut fut2 = Box::pin(
            svc2.ready()
                .await
                .unwrap()
                .call(Exchange::new(Message::new("inline"))),
        );

        // Race fut2 against a 50ms sleep: fut2 should still be Pending (it is
        // running the tap inline, awaiting Notify).
        let pending_after_50ms = tokio::select! {
            r = &mut fut2 => {
                panic!(
                    "fut2 should be Pending after 50ms under CallerRuns back-pressure; resolved early: {:?}",
                    r.is_ok()
                );
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => true,
        };
        assert!(
            pending_after_50ms,
            "fut2 should be Pending (inline tap awaiting Notify) after 50ms under CallerRuns back-pressure"
        );

        // Release all waiters: notify_waiters wakes both the detached tap 1 and
        // the inline tap (fut2). fut2 resolves Ok.
        notify.notify_waiters();
        let result = fut2.await;
        assert!(
            result.is_ok(),
            "fut2 should resolve Ok after notify_waiters"
        );
    }

    #[tokio::test]
    async fn test_wiretap_unbounded_none_path_detaches_without_permit() {
        // Explicit unbounded (max_concurrent = None) path: tasks detach into the
        // tracker without acquiring a permit. The tracker count rises above 0
        // then drains to 0 as tasks complete.
        let tap_processor = BoxProcessor::from_fn(|_ex| {
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                Ok(Exchange::default())
            })
        });

        let mut svc = WireTapService::with_config(
            tap_processor,
            WireTapConfig {
                max_concurrent: None,
                shutdown_grace: std::time::Duration::from_secs(5),
            },
        );

        let sampler_svc = svc.clone();
        let peak = Arc::new(AtomicUsize::new(0));
        let peak_clone = Arc::clone(&peak);
        let done = Arc::new(AtomicBool::new(false));
        let done_clone = Arc::clone(&done);
        let sampler = tokio::spawn(async move {
            while !done_clone.load(Ordering::SeqCst) {
                let n = sampler_svc.in_flight_count();
                peak_clone.fetch_max(n, Ordering::SeqCst);
                tokio::task::yield_now().await;
            }
        });

        for _ in 0..50 {
            let ex = Exchange::new(Message::new("x"));
            let _ = svc.ready().await.unwrap().call(ex).await.unwrap();
        }

        // Poll until the tracker drains to 0 within 2s.
        let drained = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if svc.in_flight_count() == 0 {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .is_ok();
        done.store(true, Ordering::SeqCst);
        let _ = sampler.await;

        assert!(drained, "unbounded path tasks should drain to 0 within 2s");
        assert!(
            peak.load(Ordering::SeqCst) > 0,
            "unbounded path should have observed tracked tasks (peak > 0)"
        );
    }

    #[tokio::test]
    async fn test_wiretap_no_unbounded_task_growth_across_bursts() {
        // Regression for the leaky spawn-then-acquire model: completed tasks
        // MUST decrement the tracker's len() so subsequent bursts do not
        // accumulate.
        let tap_processor =
            BoxProcessor::from_fn(|_ex| Box::pin(async move { Ok(Exchange::default()) }));

        let svc = WireTapService::with_config(tap_processor, WireTapConfig::default());

        let drain_to_zero = |svc: &WireTapService| {
            let s = svc.clone();
            async move {
                tokio::time::timeout(std::time::Duration::from_secs(2), async {
                    loop {
                        if s.in_flight_count() == 0 {
                            return;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                    }
                })
                .await
                .is_ok()
            }
        };

        // Burst 1.
        let mut callers = Vec::new();
        for _ in 0..1000 {
            let mut s = svc.clone();
            callers.push(tokio::spawn(async move {
                let _ = s.ready().await.unwrap().call(Exchange::default()).await;
            }));
        }
        for h in callers {
            let _ = h.await;
        }
        assert!(
            drain_to_zero(&svc).await,
            "burst 1 must drain to in_flight_count == 0 within 2s"
        );

        // Burst 2.
        let mut callers = Vec::new();
        for _ in 0..1000 {
            let mut s = svc.clone();
            callers.push(tokio::spawn(async move {
                let _ = s.ready().await.unwrap().call(Exchange::default()).await;
            }));
        }
        for h in callers {
            let _ = h.await;
        }
        assert!(
            drain_to_zero(&svc).await,
            "burst 2 must drain to in_flight_count == 0 within 2s (no accumulation across bursts)"
        );
    }

    // --- Tracing capture helper for warn-log assertions ---

    /// `MakeWriter` that appends formatted events to a shared `Vec<u8>` sink.
    /// Used by the warn-suppression tests to assert that a `warn!` record was
    /// emitted. The sink collects the ANSI-stripped fmt layer output.
    #[derive(Clone)]
    struct CapturingWriter {
        sink: Arc<Mutex<Vec<u8>>>,
    }

    impl std::io::Write for CapturingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.sink.lock().unwrap().extend_from_slice(buf); // allow-unwrap: test-only
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturingWriter {
        type Writer = CapturingWriter;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn capture_sink() -> (Arc<Mutex<Vec<u8>>>, impl tracing::Subscriber) {
        let sink: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let writer = CapturingWriter {
            sink: Arc::clone(&sink),
        };
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer)
            .with_ansi(false)
            .finish();
        (sink, subscriber)
    }

    /// Custom Service whose `poll_ready` always returns `Err`. Used to exercise
    /// the tap-readiness-error suppression path.
    #[derive(Clone)]
    struct ReadyFailingSvc {
        err_msg: &'static str,
    }

    impl Service<Exchange> for ReadyFailingSvc {
        type Response = Exchange;
        type Error = CamelError;
        type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;
        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Err(CamelError::ProcessorError(self.err_msg.into())))
        }
        fn call(&mut self, ex: Exchange) -> Self::Future {
            Box::pin(async move { Ok(ex) })
        }
    }

    #[tokio::test]
    async fn test_wiretap_tap_readiness_error_suppressed_with_log() {
        let tap: camel_api::BoxProcessor = tower::util::BoxCloneService::new(ReadyFailingSvc {
            err_msg: "ready-boom",
        });
        let mut svc = WireTapService::new(tap);

        let (sink, subscriber) = capture_sink();
        let exchange = Exchange::new(Message::new("main"));

        // set_default propagates to tasks spawned via tokio within this scope.
        let _guard = tracing::subscriber::set_default(subscriber);
        let result = svc.ready().await.unwrap().call(exchange).await;

        assert!(result.is_ok(), "tap readiness error must be suppressed");
        assert_eq!(result.unwrap().input.body.as_text(), Some("main"));

        // Give the spawned tap task time to run ready() and log warn.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        drop(_guard);

        let captured = String::from_utf8(sink.lock().unwrap().clone()).unwrap(); // allow-unwrap: test-only
        assert!(
            captured.contains("ready-boom"),
            "a warn! record mentioning the readiness error should have been emitted; got: {captured}"
        );
    }

    #[tokio::test]
    async fn test_wiretap_tap_processing_error_suppressed_with_log() {
        let tap_processor = BoxProcessor::from_fn(|_ex| {
            Box::pin(async move { Err(CamelError::ProcessorError("call-boom".into())) })
        });
        let mut svc = WireTapService::new(tap_processor);

        let (sink, subscriber) = capture_sink();
        let exchange = Exchange::new(Message::new("main"));

        let _guard = tracing::subscriber::set_default(subscriber);
        let result = svc.ready().await.unwrap().call(exchange).await;

        assert!(result.is_ok(), "tap processing error must be suppressed");
        assert_eq!(result.unwrap().input.body.as_text(), Some("main"));

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        drop(_guard);

        let captured = String::from_utf8(sink.lock().unwrap().clone()).unwrap(); // allow-unwrap: test-only
        assert!(
            captured.contains("call-boom"),
            "a warn! record mentioning the processing error should have been emitted; got: {captured}"
        );
    }

    #[tokio::test]
    async fn test_wiretap_poll_ready_always_ready() {
        // poll_ready returns Ready(Ok(())) unconditionally (ADR-0019), even
        // when the tap endpoint's own readiness would fail.
        let tap: camel_api::BoxProcessor = tower::util::BoxCloneService::new(ReadyFailingSvc {
            err_msg: "would-fail",
        });
        let mut svc = WireTapService::new(tap);

        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let poll = svc.poll_ready(&mut cx);
        assert!(
            matches!(poll, Poll::Ready(Ok(()))),
            "poll_ready must be Ready(Ok(())) unconditionally (ADR-0019), got opposite"
        );
    }

    // --- WireTapLifecycle + StepLifecycle shutdown tests (Task 4) ---

    #[tokio::test]
    async fn test_wiretap_shutdown_drains_fast_aborts_slow() {
        let fast_done = Arc::new(AtomicBool::new(false));
        let slow_done = Arc::new(AtomicBool::new(false));
        let call_idx = Arc::new(AtomicUsize::new(0));

        let fd = fast_done.clone();
        let sd = slow_done.clone();
        let ci = call_idx.clone();
        let tap_processor = BoxProcessor::from_fn(move |ex| {
            let fd = fd.clone();
            let sd = sd.clone();
            let ci = ci.clone();
            Box::pin(async move {
                let n = ci.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    fd.store(true, Ordering::SeqCst);
                } else {
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                    sd.store(true, Ordering::SeqCst);
                }
                Ok(ex)
            })
        });

        let config = WireTapConfig {
            max_concurrent: Some(20),
            shutdown_grace: Duration::from_millis(200),
        };
        let mut svc = WireTapService::with_config(tap_processor, config);

        let _ = svc
            .ready()
            .await
            .unwrap()
            .call(Exchange::new(Message::new("fast")))
            .await
            .unwrap();
        let _ = svc
            .ready()
            .await
            .unwrap()
            .call(Exchange::new(Message::new("slow")))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(20)).await;

        let lifecycle = svc.lifecycle();
        let start = tokio::time::Instant::now();
        lifecycle
            .shutdown(StepShutdownReason::RouteStop)
            .await
            .unwrap();
        let elapsed = start.elapsed();

        assert!(
            fast_done.load(Ordering::SeqCst),
            "fast tap should drain before grace expires"
        );
        assert!(
            !slow_done.load(Ordering::SeqCst),
            "slow tap should be aborted after grace, not complete"
        );
        assert!(
            elapsed < Duration::from_millis(500),
            "shutdown took {:?}, expected < 500ms",
            elapsed
        );
    }

    #[tokio::test]
    async fn test_wiretap_shutdown_idempotent() {
        let slow_done = Arc::new(AtomicBool::new(false));
        let sd = slow_done.clone();
        let tap_processor = BoxProcessor::from_fn(move |ex| {
            let sd = sd.clone();
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                sd.store(true, Ordering::SeqCst);
                Ok(ex)
            })
        });

        let config = WireTapConfig {
            max_concurrent: Some(20),
            shutdown_grace: Duration::from_millis(50),
        };
        let mut svc = WireTapService::with_config(tap_processor, config);

        let _ = svc
            .ready()
            .await
            .unwrap()
            .call(Exchange::new(Message::new("slow")))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(20)).await;

        let lifecycle = svc.lifecycle();
        lifecycle
            .shutdown(StepShutdownReason::RouteStop)
            .await
            .unwrap();

        let start = tokio::time::Instant::now();
        let result = lifecycle.shutdown(StepShutdownReason::HotSwap).await;
        let elapsed = start.elapsed();

        assert!(result.is_ok(), "second shutdown must return Ok");
        assert!(
            elapsed < Duration::from_millis(100),
            "second shutdown must return promptly, took {:?}",
            elapsed
        );
        assert!(
            !slow_done.load(Ordering::SeqCst),
            "slow tap must be aborted, not completed"
        );
    }

    #[tokio::test]
    async fn test_wiretap_calls_after_close_rejected() {
        let tap_invoked = Arc::new(AtomicBool::new(false));
        let ti = tap_invoked.clone();
        let tap_processor = BoxProcessor::from_fn(move |ex| {
            let ti = ti.clone();
            Box::pin(async move {
                ti.store(true, Ordering::SeqCst);
                Ok(ex)
            })
        });

        let mut svc = WireTapService::new(tap_processor);
        let lifecycle = svc.lifecycle();
        lifecycle
            .shutdown(StepShutdownReason::RouteStop)
            .await
            .unwrap();

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(Exchange::new(Message::new("post-close")))
            .await;

        assert!(
            result.is_ok(),
            "call after close must return Ok(original exchange)"
        );
        assert!(
            !tap_invoked.load(Ordering::SeqCst),
            "tap must not be invoked after admission closed"
        );
    }

    #[tokio::test]
    async fn test_wiretap_cancellation_while_pending_readiness() {
        // Service whose poll_ready returns Pending indefinitely, so the
        // spawned task blocks in run_tap's ready() phase. Shutdown cancels
        // the token, the biased select! picks it up, and the task exits
        // cleanly without reaching call().
        #[derive(Clone)]
        struct ForeverPendingSvc {
            called: Arc<AtomicBool>,
        }

        impl Service<Exchange> for ForeverPendingSvc {
            type Response = Exchange;
            type Error = CamelError;
            type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

            fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
                Poll::Pending
            }

            fn call(&mut self, ex: Exchange) -> Self::Future {
                self.called.store(true, Ordering::SeqCst);
                Box::pin(async move { Ok(ex) })
            }
        }

        let called = Arc::new(AtomicBool::new(false));
        let tap: camel_api::BoxProcessor = tower::util::BoxCloneService::new(ForeverPendingSvc {
            called: called.clone(),
        });
        let mut svc = WireTapService::new(tap);

        let _ = svc
            .ready()
            .await
            .unwrap()
            .call(Exchange::new(Message::new("hanging")))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(20)).await;

        let lifecycle = svc.lifecycle();
        let result = lifecycle.shutdown(StepShutdownReason::RouteStop).await;

        assert!(
            result.is_ok(),
            "shutdown must succeed even with pending readiness: {:?}",
            result
        );
        assert!(
            !called.load(Ordering::SeqCst),
            "tap call() must never be reached — cancelled during readiness phase"
        );
    }

    #[tokio::test]
    async fn test_wiretap_zero_grace_immediate_cancel() {
        let slow_done = Arc::new(AtomicBool::new(false));
        let sd = slow_done.clone();
        let tap_processor = BoxProcessor::from_fn(move |ex| {
            let sd = sd.clone();
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                sd.store(true, Ordering::SeqCst);
                Ok(ex)
            })
        });

        let config = WireTapConfig {
            max_concurrent: Some(20),
            shutdown_grace: Duration::ZERO,
        };
        let mut svc = WireTapService::with_config(tap_processor, config);

        let _ = svc
            .ready()
            .await
            .unwrap()
            .call(Exchange::new(Message::new("slow")))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(20)).await;

        let lifecycle = svc.lifecycle();
        let start = tokio::time::Instant::now();
        lifecycle
            .shutdown(StepShutdownReason::RouteStop)
            .await
            .unwrap();
        let elapsed = start.elapsed();

        assert!(
            !slow_done.load(Ordering::SeqCst),
            "slow tap must be aborted immediately (zero grace)"
        );
        assert!(
            elapsed < Duration::from_millis(200),
            "zero-grace shutdown must return quickly, took {:?}",
            elapsed
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_wiretap_admission_shutdown_no_orphan_task() {
        // Stress test: fire concurrent call()s and shutdown() across many
        // randomized iterations. Verify in_flight_count() == 0 after shutdown
        // completes — no orphan task escaped the tracker.
        const ITERATIONS: usize = 200;

        for _ in 0..ITERATIONS {
            let tap_processor = BoxProcessor::from_fn(|_ex| {
                Box::pin(async move {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                    Ok(Exchange::default())
                })
            });

            let svc = WireTapService::new(tap_processor);
            let lifecycle = svc.lifecycle();

            // Fire several concurrent callers.
            let mut handles = Vec::new();
            for _ in 0..4 {
                let mut c = svc.clone();
                handles.push(tokio::spawn(async move {
                    let _ = c.ready().await.unwrap().call(Exchange::default()).await;
                }));
            }

            // Yield to let spawns register in the tracker.
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(1)).await;

            // Shutdown concurrently with callers still in-flight.
            lifecycle
                .shutdown(StepShutdownReason::RouteStop)
                .await
                .unwrap();

            for h in handles {
                let _ = h.await;
            }

            // Poll until tracker drains (already waited in shutdown, but
            // defensive check).
            let drained = tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if svc.in_flight_count() == 0 {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .is_ok();

            assert!(
                drained,
                "iteration: in_flight_count must drain to 0 after shutdown"
            );
        }
    }
}
