//! Resequencer — continuation-boundary EIP.
//!
//! The resequencer is a `CompiledStep` + `StepLifecycle`. `call(input)` sends
//! the input into a bounded actor channel; an actor buffers + computes ready
//! outputs + sends them to a post-driver that drives the owned post-continuation;
//! `call()` returns a control ack. The main pipeline ends at the resequencer.
//!
//! Architecture: See ADR-0029 (resequencer continuation boundary).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::{Mutex as TokioMutex, mpsc};
use tokio::task::JoinHandle;
use tower::Service;
use tower::util::BoxCloneService;

pub mod batch;
pub mod stream;

use camel_api::{
    CamelError, MetricsCollector, StepLifecycle, StepShutdownReason, exchange::Exchange,
    message::Message, processor::SyncBoxProcessor,
};

/// Rate-limit window for InOut warning log emission.
const INOUT_WARN_INTERVAL: Duration = Duration::from_secs(30);

/// Configuration for the `ResequencerService`.
#[derive(Clone, Default)]
pub struct ResequencerConfig {
    /// Allow `InOut` exchanges to pass through the resequencer without
    /// emitting a warning. Defaults to `false`.
    pub allow_inout: bool,
    /// Optional metrics collector for incrementing operational counters.
    pub metrics: Option<Arc<dyn MetricsCollector>>,
    /// Optional route ID for metric labels.
    pub route_id: Option<String>,
}

impl std::fmt::Debug for ResequencerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResequencerConfig")
            .field("allow_inout", &self.allow_inout)
            .field("metrics", &self.metrics.as_ref().map(|_| "<metrics>"))
            .field("route_id", &self.route_id)
            .finish()
    }
}

// ── Property keys (CamelCase, matching CAMEL_AGGREGATED_COMPLETION_REASON) ──

/// Set on the ack exchange: `true` when the resequencer accepted the input.
pub const CAMEL_RESEQUENCER_ACCEPTED: &str = "CamelResequencerAccepted";

/// Set on the ack exchange: `true` when the input was dropped during shutdown.
pub const CAMEL_RESEQUENCER_DROPPED: &str = "CamelResequencerDropped";

/// Set on the ack exchange: `true` when an InOut exchange reaches the resequencer.
pub const CAMEL_RESEQUENCER_INOUT_WARN: &str = "CamelResequencerInoutWarn";

// ── Policy trait ──

/// Buffer / ordering policy for a resequencer.
///
/// Implementations (batch, stream) live in sibling modules.
#[async_trait]
pub trait ResequencePolicy: Send + Sync + 'static {
    /// Accept an input; return the list of now-ready exchanges (in emit order).
    async fn accept(&self, input: Exchange) -> Vec<Exchange>;

    /// Flush all buffered state (shutdown). Return any remaining, ordered.
    async fn flush(&self) -> Vec<Exchange>;

    /// Stable name for logging / diagnostics.
    fn name(&self) -> &'static str;

    /// Set the driver channel for timeout-triggered emissions.
    /// Default is a no-op. `BatchPolicy` overrides this to receive
    /// the channel that feeds the post-driver.
    fn set_timeout_tx(&self, _tx: tokio::sync::mpsc::Sender<Exchange>) {}
}

// ── Service ──

/// Continuation-boundary resequencer.
///
/// Owns an actor task (consumes from input channel, calls `policy.accept(input)`)
/// and a post-driver task (consumes ready exchanges, drives `post_continuation`).
/// `Service::call(input)` sends into the bounded input channel and returns an ack.
#[derive(Clone)]
pub struct ResequencerService {
    policy: Arc<dyn ResequencePolicy>,
    config: ResequencerConfig,
    /// Bounded input channel sender. Wrapped in `Option` so `shutdown` can
    /// `take()` it to signal EOF to the actor (all sender clones must drop).
    input_tx: Arc<Mutex<Option<mpsc::Sender<Exchange>>>>,
    /// Post-driver channel sender. The actor task holds a clone; we hold one
    /// here for shutdown flush. Wrapped in `Option` so `shutdown` can take it
    /// to close the post-driver channel after flush.
    driver_tx: Arc<Mutex<Option<mpsc::Sender<Exchange>>>>,
    actor_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    driver_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    shutdown_started: Arc<Mutex<bool>>,
    /// Post-step lifecycles to drain after post-driver quiesces (oracle Fix 2).
    /// Empty for Task 1a (no post-steps yet); filled by Tasks 1b/2/3.
    post_lifecycles: Arc<Mutex<Vec<Arc<dyn StepLifecycle>>>>,
    /// Metric counter for InOut exchanges that reach the resequencer.
    inout_counter: Arc<AtomicU64>,
    /// Rate-limit last-warn timestamp (TokioMutex for async safety).
    last_inout_warn: Arc<TokioMutex<Option<Instant>>>,
    /// Optional metrics collector for operational counters.
    metrics: Option<Arc<dyn MetricsCollector>>,
    /// Route ID for metric labels.
    route_id: Option<String>,
}

impl std::fmt::Debug for ResequencerService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResequencerService")
            .field("policy", &self.policy.name())
            .finish_non_exhaustive()
    }
}

impl ResequencerService {
    /// Create a new resequencer with the given policy, continuation, and post-step lifecycles.
    ///
    /// * `input_capacity` — bounded channel capacity (default 1024). Backpressure
    ///   propagates to the caller via `send().await`.
    /// * `post_lifecycles` — lifecycle handles for steps AFTER the resequencer
    ///   (drained in `shutdown` after post-driver quiesces; empty for Task 1a).
    ///
    /// # Panics
    ///
    /// Panics if called outside a Tokio runtime context: `new()` spawns the actor and
    /// post-driver tasks via `tokio::spawn`.
    pub fn new(
        policy: Arc<dyn ResequencePolicy>,
        post_continuation: BoxCloneService<Exchange, Exchange, CamelError>,
        input_capacity: usize,
        post_lifecycles: Vec<Arc<dyn StepLifecycle>>,
    ) -> Self {
        Self::with_config(
            policy,
            post_continuation,
            input_capacity,
            post_lifecycles,
            ResequencerConfig::default(),
        )
    }

    /// Full constructor with explicit `ResequencerConfig`.
    ///
    /// # Panics
    ///
    /// Panics if called outside a Tokio runtime context.
    pub fn with_config(
        policy: Arc<dyn ResequencePolicy>,
        post_continuation: BoxCloneService<Exchange, Exchange, CamelError>,
        input_capacity: usize,
        post_lifecycles: Vec<Arc<dyn StepLifecycle>>,
        config: ResequencerConfig,
    ) -> Self {
        // Bounded channel for input exchanges → actor
        let (input_tx, mut input_rx) = mpsc::channel::<Exchange>(input_capacity);

        // Post-driver channel: actor → post-driver (bounded, single consumer)
        let (driver_tx, mut driver_rx) = mpsc::channel::<Exchange>(input_capacity);

        // Shared (Arc<Mutex<Option<T>>>) wrappers
        let input_tx_shared: Arc<Mutex<Option<mpsc::Sender<Exchange>>>> =
            Arc::new(Mutex::new(Some(input_tx)));
        let driver_tx_shared: Arc<Mutex<Option<mpsc::Sender<Exchange>>>> =
            Arc::new(Mutex::new(Some(driver_tx.clone()))); // we keep one for flush; actor gets clone
        let actor_handle: Arc<Mutex<Option<JoinHandle<()>>>> = Arc::new(Mutex::new(None));
        let driver_handle: Arc<Mutex<Option<JoinHandle<()>>>> = Arc::new(Mutex::new(None));
        let shutdown_started = Arc::new(Mutex::new(false));

        // Wire the driver channel into the policy for timeout-triggered emissions
        policy.set_timeout_tx(driver_tx.clone());

        // Thread-safe wrapper for the post-continuation
        let sync_post = SyncBoxProcessor::new(post_continuation);

        // ── Spawn actor task ──
        {
            let policy = Arc::clone(&policy);
            let actor_h = Arc::clone(&actor_handle);
            let actor_driver_tx = driver_tx; // move the original sender into the actor
            let handle = tokio::spawn(async move {
                while let Some(input) = input_rx.recv().await {
                    let ready = policy.accept(input).await;
                    for ex in ready {
                        // If the post-driver channel is closed, stop
                        if actor_driver_tx.send(ex).await.is_err() {
                            // post-driver dropped → exit
                            return;
                        }
                    }
                }
                // input channel closed (EOF from shutdown) → exit naturally
            });
            // allow-unwrap: actor handle slot is always None at construction time
            *actor_h.lock().expect("actor_handle lock poisoned") = Some(handle);
        }

        // ── Spawn post-driver task ──
        {
            let post = sync_post.clone();
            let driver_h = Arc::clone(&driver_handle);
            let metrics = config.metrics.clone();
            let route_id = config.route_id.clone();
            let handle = tokio::spawn(async move {
                while let Some(ex) = driver_rx.recv().await {
                    // CamelStop interaction: skip continuation for stop-signaled exchanges
                    if camel_api::is_camel_stop(&ex) {
                        tracing::debug!(
                            "resequencer post-driver: skipping continuation for CamelStop exchange"
                        );
                        continue;
                    }
                    // Clone inner processor (cheap: Arc bump + Mutex briefly held)
                    let mut proc = post.clone_inner();
                    match proc.call(ex).await {
                        Ok(_) => {}
                        Err(e) => {
                            // log-policy: post-ack failure (ADR-0012 best-effort, ADR-0029 I7)
                            tracing::warn!(
                                error = %e,
                                "resequencer post-driver: continuation call failed after ack (best-effort)"
                            );
                            if let Some(ref m) = metrics {
                                m.increment_errors(
                                    route_id.as_deref().unwrap_or("unknown"),
                                    "resequencer:post_ack_failure",
                                );
                            }
                        }
                    }
                }
            });
            // allow-unwrap: driver handle slot is always None at construction time
            *driver_h.lock().expect("driver_handle lock poisoned") = Some(handle);
        }

        let metrics = config.metrics.clone();
        let route_id = config.route_id.clone();
        Self {
            policy,
            config,
            input_tx: input_tx_shared,
            driver_tx: driver_tx_shared,
            actor_handle,
            driver_handle,
            shutdown_started,
            post_lifecycles: Arc::new(Mutex::new(post_lifecycles)),
            inout_counter: Arc::new(AtomicU64::new(0)),
            last_inout_warn: Arc::new(TokioMutex::new(None)),
            metrics,
            route_id,
        }
    }
}

// ── Tower Service impl ──

impl Service<Exchange> for ResequencerService {
    type Response = Exchange;
    type Error = CamelError;
    type Future =
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), CamelError>> {
        // ADR-0019: always ready; backpressure via bounded send().await in call()
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, input: Exchange) -> Self::Future {
        let config = self.config.clone();
        let inout_counter = Arc::clone(&self.inout_counter);
        let last_inout_warn = Arc::clone(&self.last_inout_warn);
        let tx_opt = Arc::clone(&self.input_tx);
        let metrics = self.metrics.clone();
        let route_id = self.route_id.clone();

        Box::pin(async move {
            // Build ack exchange
            let mut ack = Exchange::new(Message::default());

            // InOut guard (I6): rate-limited with metric counter and property flag
            if input.pattern == camel_api::exchange::ExchangePattern::InOut && !config.allow_inout {
                inout_counter.fetch_add(1, Ordering::Relaxed);
                ack.set_property(CAMEL_RESEQUENCER_INOUT_WARN, true);
                if let Some(ref m) = metrics {
                    m.increment_errors(
                        route_id.as_deref().unwrap_or("unknown"),
                        "resequencer:inout_warning",
                    );
                }
                let now = Instant::now();
                let mut last_guard = last_inout_warn.lock().await;
                let should_warn = last_guard
                    .map(|t| now.duration_since(t) >= INOUT_WARN_INTERVAL)
                    .unwrap_or(true);
                if should_warn {
                    let count = inout_counter.load(Ordering::Relaxed);
                    tracing::warn!(
                        inout_count = count,
                        "InOut exchange reached resequencer ({count} total); \
                         consider using InOnly pattern. \
                         Set allow_inout=true to suppress this warning."
                    );
                    *last_guard = Some(now);
                }
            }

            // Snapshot the sender (if still active). If shutdown already took it,
            // the exchange is dropped — best-effort (intake already cancelled).
            let tx = {
                let guard = tx_opt.lock().unwrap_or_else(|e| e.into_inner());
                guard.clone()
            };
            if let Some(tx) = tx {
                // Backpressure: blocks if channel is full
                match tx.send(input).await {
                    Ok(()) => {
                        ack.set_property(CAMEL_RESEQUENCER_ACCEPTED, true);
                    }
                    Err(tokio::sync::mpsc::error::SendError(input)) => {
                        tracing::warn!(
                            correlation_id = %input.correlation_id,
                            "resequencer input dropped during shutdown"
                        );
                        ack.set_property(CAMEL_RESEQUENCER_ACCEPTED, false);
                        ack.set_property(CAMEL_RESEQUENCER_DROPPED, true);
                    }
                }
            }

            Ok(ack)
        })
    }
}

// ── StepLifecycle impl ──

#[async_trait]
impl StepLifecycle for ResequencerService {
    fn name(&self) -> &'static str {
        self.policy.name()
    }

    /// Idempotent shutdown with this ordering:
    /// 1. Set shutdown flag; close/drop input_tx so actor sees EOF.
    /// 2. Await actor JoinHandle (bounded deadline).
    /// 3. policy.flush() → emit remaining in order via post-driver.
    /// 4. Close post-driver channel sender so its loop sees EOF.
    /// 5. Await post-driver JoinHandle with 5s deadline.
    /// 6. Drain post-step lifecycles (oracle Fix 2).
    async fn shutdown(&self, reason: StepShutdownReason) -> Result<(), CamelError> {
        // TODO(Task 1b): differentiate HotSwap (complete in-flight through old continuation,
        // ADR-0004) from RouteStop (flush + drain). Currently both paths run the same
        // flush-then-close sequence.
        tracing::debug!(
            reason = ?reason,
            policy = self.policy.name(),
            "ResequencerService shutdown via StepLifecycle"
        );

        // Idempotent guard (I1)
        {
            let mut started = self
                .shutdown_started
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if *started {
                tracing::debug!(
                    "ResequencerService shutdown already started (idempotent); skipping"
                );
                return Ok(());
            }
            *started = true;
        }

        // Step 1: Close/drop input_tx so actor's input_rx sees EOF.
        // Taking the sender from the Option drops it. The shared Arc<Mutex<...>>
        // means all clones see None after this.
        {
            let mut guard = self.input_tx.lock().unwrap_or_else(|e| e.into_inner());
            *guard = None; // drop the sender
        }

        // Step 2: Await actor JoinHandle (bounded deadline).
        // Extract handle first, then await outside the lock to avoid Send issue.
        let actor_handle_to_await = {
            let mut guard = self.actor_handle.lock().unwrap_or_else(|e| e.into_inner());
            guard.take()
        };
        if let Some(handle) = actor_handle_to_await {
            // 5s deadline for actor to finish processing remaining input
            let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
        }

        // Step 3: policy.flush() → emit remaining in order via post-driver.
        let flushed = self.policy.flush().await;
        if !flushed.is_empty() {
            let dt = {
                let guard = self.driver_tx.lock().unwrap_or_else(|e| e.into_inner());
                guard.clone()
            };
            if let Some(driver_tx) = dt {
                for ex in flushed {
                    if driver_tx.send(ex).await.is_err() {
                        tracing::warn!(
                            "resequencer shutdown flush: post-driver channel closed early"
                        );
                        break;
                    }
                }
            }
        }

        // Step 4: Close the post-driver channel sender so its loop sees EOF.
        {
            let mut guard = self.driver_tx.lock().unwrap_or_else(|e| e.into_inner());
            *guard = None; // drop the sender → driver_rx.recv() returns None
        }

        // Step 5: Await the post-driver JoinHandle with a 5s deadline.
        let driver_handle_to_await = {
            let mut guard = self.driver_handle.lock().unwrap_or_else(|e| e.into_inner());
            guard.take()
        };
        if let Some(handle) = driver_handle_to_await {
            let result = tokio::time::timeout(Duration::from_secs(5), handle).await;
            if result.is_err() {
                tracing::warn!(
                    "resequencer post-driver task did not finish within 5s deadline; \
                     leaking handle (best-effort)"
                );
            }
        }

        // Step 6: Drain post-step lifecycles (oracle Fix 2).
        // Must happen AFTER post-driver drains (flush emits through continuation first).
        {
            let post_lcs: Vec<Arc<dyn StepLifecycle>> = {
                let mut guard = self
                    .post_lifecycles
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                std::mem::take(&mut *guard)
            };
            for lc in &post_lcs {
                if let Err(e) = lc.shutdown(reason).await {
                    tracing::warn!(
                        step = lc.name(),
                        error = %e,
                        "resequencer post-step lifecycle shutdown failed (best-effort)"
                    );
                }
            }
        }

        Ok(())
    }
}

// ── Passthrough policy (for testing) ──

/// Emits each input unchanged — used as a baseline policy for testing
/// the continuation-boundary mechanics.
#[derive(Debug)]
pub struct PassthroughPolicy;

#[async_trait]
impl ResequencePolicy for PassthroughPolicy {
    async fn accept(&self, input: Exchange) -> Vec<Exchange> {
        vec![input]
    }

    async fn flush(&self) -> Vec<Exchange> {
        vec![]
    }

    fn name(&self) -> &'static str {
        "passthrough"
    }
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    /// A test continuation that sends received exchanges through an mpsc channel.
    #[derive(Clone)]
    struct CapturePost {
        tx: mpsc::UnboundedSender<Exchange>,
    }

    impl Service<Exchange> for CapturePost {
        type Response = Exchange;
        type Error = CamelError;
        type Future = std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Exchange, CamelError>> + Send>,
        >;

        fn poll_ready(
            &mut self,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), CamelError>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn call(&mut self, exchange: Exchange) -> Self::Future {
            let tx = self.tx.clone();
            Box::pin(async move {
                // Fire-and-forget: drop send errors (receiver gone = test teardown)
                let _ = tx.send(exchange.clone());
                Ok(exchange)
            })
        }
    }

    #[tokio::test]
    async fn resequencer_boundary_passthrough_ack_and_continuation() {
        use camel_api::body::Body;

        // Build a resequencer with passthrough policy + channel capture continuation
        let policy: Arc<dyn ResequencePolicy> = Arc::new(PassthroughPolicy);
        let (capture_tx, mut capture_rx) = mpsc::unbounded_channel::<Exchange>();
        let capture = CapturePost { tx: capture_tx };
        let post_continuation: BoxCloneService<Exchange, Exchange, CamelError> =
            BoxCloneService::new(capture);

        let service = ResequencerService::new(policy, post_continuation, 1024, vec![]);

        // Send an exchange
        let mut input = Exchange::new(Message::new(Body::Text("hello".into())));
        input.set_property("seq", 1);

        let ack = service.clone().oneshot(input).await.unwrap();

        // Assert (a): ack has Body::Empty + CAMEL_RESEQUENCER_ACCEPTED=true
        assert!(
            matches!(ack.input.body, Body::Empty),
            "ack body should be Empty, got {:?}",
            ack.input.body
        );
        assert_eq!(
            ack.property(CAMEL_RESEQUENCER_ACCEPTED)
                .and_then(|v| v.as_bool()),
            Some(true),
            "CAMEL_RESEQUENCER_ACCEPTED should be true"
        );

        // Assert (b): the post-continuation receives the input payload
        let captured = tokio::time::timeout(Duration::from_millis(500), capture_rx.recv())
            .await
            .expect("post-continuation did not receive exchange within 500ms timeout")
            .expect("capture channel closed without receiving exchange");
        let body = captured.input.body.as_text();
        assert_eq!(
            body,
            Some("hello"),
            "post-continuation received body should match"
        );

        // Assert (c): shutdown is idempotent (calling twice returns Ok)
        service
            .shutdown(StepShutdownReason::RouteStop)
            .await
            .expect("first shutdown should succeed");

        service
            .shutdown(StepShutdownReason::RouteStop)
            .await
            .expect("second shutdown should succeed (idempotent)");
    }

    #[tokio::test]
    async fn resequencer_boundary_camel_stop_skipped() {
        use camel_api::body::Body;

        let policy: Arc<dyn ResequencePolicy> = Arc::new(PassthroughPolicy);
        let (capture_tx, mut capture_rx) = mpsc::unbounded_channel::<Exchange>();
        let capture = CapturePost { tx: capture_tx };
        let post_continuation: BoxCloneService<Exchange, Exchange, CamelError> =
            BoxCloneService::new(capture);

        let service = ResequencerService::new(policy, post_continuation, 1024, vec![]);

        // Send an exchange flagged with CamelStop
        let mut input = Exchange::new(Message::new(Body::Text(
            "should-not-reach-continuation".into(),
        )));
        input.set_property(camel_api::exchange::CAMEL_STOP, true);

        let ack = service.clone().oneshot(input).await.unwrap();

        // Assert: actor accepted the exchange (ack is returned)
        assert_eq!(
            ack.property(CAMEL_RESEQUENCER_ACCEPTED)
                .and_then(|v| v.as_bool()),
            Some(true),
            "CamelStop exchange should still be accepted by resequencer actor"
        );

        // Assert: the post-continuation does NOT receive the CamelStop exchange
        let did_receive = tokio::time::timeout(Duration::from_millis(500), capture_rx.recv()).await;
        match did_receive {
            Ok(Some(_)) => panic!("CamelStop exchange should NOT reach post-continuation"),
            Ok(None) => {}      // channel closed (expected in some teardown scenarios)
            Err(_elapsed) => {} // timeout is the expected path: nothing arrived
        }

        // Cleanup
        service
            .shutdown(StepShutdownReason::RouteStop)
            .await
            .expect("shutdown should succeed");
    }

    #[tokio::test]
    async fn inout_guard_increments_counter() {
        let policy: Arc<dyn ResequencePolicy> = Arc::new(PassthroughPolicy);
        let (tx, _rx) = mpsc::unbounded_channel::<Exchange>();
        let post: BoxCloneService<Exchange, Exchange, CamelError> =
            BoxCloneService::new(CapturePost { tx });
        let config = ResequencerConfig::default();
        let service = ResequencerService::with_config(policy, post, 16, vec![], config);

        // InOnly should NOT increment counter
        let ex_inonly = Exchange::new(Message::new("inonly"));
        let _ = service.clone().oneshot(ex_inonly).await.unwrap();

        // InOut SHOULD increment counter
        let ex_inout = Exchange::new_in_out(Message::new("inout"));
        let _ = service.clone().oneshot(ex_inout).await.unwrap();
        assert!(
            service.inout_counter.load(Ordering::Relaxed) > 0,
            "InOut counter should be > 0 after InOut exchange"
        );

        service
            .shutdown(StepShutdownReason::RouteStop)
            .await
            .expect("shutdown");
    }

    #[tokio::test]
    async fn inout_guard_allow_inout_suppresses() {
        let policy: Arc<dyn ResequencePolicy> = Arc::new(PassthroughPolicy);
        let (tx, _rx) = mpsc::unbounded_channel::<Exchange>();
        let post: BoxCloneService<Exchange, Exchange, CamelError> =
            BoxCloneService::new(CapturePost { tx });
        let config = ResequencerConfig {
            allow_inout: true,
            ..Default::default()
        };
        let service = ResequencerService::with_config(policy, post, 16, vec![], config);

        let ex_inout = Exchange::new_in_out(Message::new("inout-allowed"));
        let _ = service.clone().oneshot(ex_inout).await.unwrap();
        assert_eq!(
            service.inout_counter.load(Ordering::Relaxed),
            0,
            "InOut counter should be 0 when allow_inout=true"
        );

        service
            .shutdown(StepShutdownReason::RouteStop)
            .await
            .expect("shutdown");
    }
}
