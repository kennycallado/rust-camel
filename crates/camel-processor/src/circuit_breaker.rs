use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tower::{Layer, Service};

use camel_api::metrics::MetricsCollector;
use camel_api::{BoxProcessor, CamelError, CircuitBreakerConfig, Exchange};

// ── State ──────────────────────────────────────────────────────────────

/// Circuit is closed — all calls pass through.
const STATE_CLOSED: u8 = 0;
/// Circuit is open — calls are rejected or fall back until expiry.
const STATE_OPEN: u8 = 1;
/// Half-open with the single probe slot taken (a probe is in flight, or the
/// caller that observed expiry claims the slot).
const STATE_HALF_OPEN_PROBE_HELD: u8 = 2;
/// Half-open with the probe slot free — released when `poll_ready` returned
/// `Pending` while a probe was claimed, so a re-poll can re-claim it (D-M1).
const STATE_HALF_OPEN_PROBE_FREE: u8 = 3;

/// Shared breaker state. The closed/success path touches only `state` and
/// `consecutive_failures` with plain atomic loads; `opened_at` is the single
/// serializer for slow-path transitions (opens, recovery close, expiry).
struct SharedBreakerState {
    state: AtomicU8,
    consecutive_failures: AtomicU32,
    opened_at: Mutex<Option<Instant>>,
}

/// Outcome of an attempt to claim the half-open probe after expiry.
enum HalfOpenTransition {
    /// The caller won the Open → HalfOpen transition and becomes the probe.
    Transitioned,
    /// The circuit is still within `open_duration`.
    NotExpired,
    /// Expiry passed but another caller won the CAS; re-load and retry.
    LostRace,
}

impl SharedBreakerState {
    fn new() -> Self {
        Self {
            state: AtomicU8::new(STATE_CLOSED),
            consecutive_failures: AtomicU32::new(0),
            opened_at: Mutex::new(None),
        }
    }

    /// Run a slow-path state transition while holding the `opened_at` lock.
    ///
    /// The closure re-loads `state` under the lock and acts only if it still
    /// shows the expected state, so qualification, timestamp publication, and
    /// the state CAS are atomic against recovery.
    fn with_transition(&self, f: impl FnOnce(&mut Option<Instant>) -> bool) -> bool {
        let mut guard = self.opened_at.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }

    /// Attempt the Open → HalfOpen(probe claimed) transition. Returns
    /// [`HalfOpenTransition::Transitioned`] when the caller won the transition
    /// (it becomes the probe). The `opened_at` lock is held across the expiry
    /// check and the CAS so a freshly re-opened circuit cannot be half-opened
    /// off a stale timestamp.
    fn try_transition_to_half_open(&self, open_duration: Duration) -> HalfOpenTransition {
        let opened_at = self.opened_at.lock().unwrap_or_else(|e| e.into_inner());
        let expired = opened_at
            .map(|at| at.elapsed() >= open_duration)
            .unwrap_or(false);
        if !expired {
            return HalfOpenTransition::NotExpired;
        }
        match self.state.compare_exchange(
            STATE_OPEN,
            STATE_HALF_OPEN_PROBE_HELD,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => HalfOpenTransition::Transitioned,
            Err(_) => HalfOpenTransition::LostRace,
        }
    }
}

// ── Layer ──────────────────────────────────────────────────────────────

/// Tower Layer that wraps an inner service with circuit-breaker logic.
#[derive(Clone)]
pub struct CircuitBreakerLayer {
    config: CircuitBreakerConfig,
    state: Arc<SharedBreakerState>,
    route_id: Arc<str>,
    metrics: Option<Arc<dyn MetricsCollector>>,
}

impl CircuitBreakerLayer {
    /// `route_id` + `metrics` thread the rejection counter
    /// (`camel_circuit_breaker_rejections_total{route}`) into the wrapped
    /// service: every open-breaker fast-fail counts as a rejection, never
    /// as an error (dashboard-observability D2).
    pub fn new(
        config: CircuitBreakerConfig,
        route_id: Arc<str>,
        metrics: Option<Arc<dyn MetricsCollector>>,
    ) -> Self {
        Self {
            config,
            state: Arc::new(SharedBreakerState::new()),
            route_id,
            metrics,
        }
    }
}

impl<S> Layer<S> for CircuitBreakerLayer {
    type Service = CircuitBreakerService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CircuitBreakerService {
            inner,
            config: self.config.clone(),
            state: Arc::clone(&self.state),
            route_id: Arc::clone(&self.route_id),
            metrics: self.metrics.clone(),
        }
    }
}

// ── Service ────────────────────────────────────────────────────────────

/// Tower Service implementing the circuit-breaker pattern.
pub struct CircuitBreakerService<S> {
    inner: S,
    config: CircuitBreakerConfig,
    state: Arc<SharedBreakerState>,
    route_id: Arc<str>,
    metrics: Option<Arc<dyn MetricsCollector>>,
}

impl<S: Clone> Clone for CircuitBreakerService<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            config: self.config.clone(),
            state: Arc::clone(&self.state),
            route_id: Arc::clone(&self.route_id),
            metrics: self.metrics.clone(),
        }
    }
}

impl<S> CircuitBreakerService<S> {
    /// Count one open-breaker fast-fail as a circuit-breaker rejection.
    fn record_rejection(&self) {
        if let Some(ref metrics) = self.metrics {
            metrics.increment_circuit_breaker_rejection(&self.route_id);
        }
    }
}

impl<S> Service<Exchange> for CircuitBreakerService<S>
where
    S: Service<Exchange, Response = Exchange, Error = CamelError> + Clone + Send + 'static,
    S::Future: Send,
{
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        loop {
            match self.state.state.load(Ordering::Acquire) {
                STATE_CLOSED => return self.inner.poll_ready(cx),
                STATE_OPEN => match self
                    .state
                    .try_transition_to_half_open(self.config.open_duration)
                {
                    HalfOpenTransition::Transitioned => {
                        tracing::info!("Circuit breaker transitioning from Open to HalfOpen");
                        return self.poll_ready_as_probe(cx);
                    }
                    HalfOpenTransition::NotExpired => {
                        if self.config.fallback.is_some() {
                            return Poll::Ready(Ok(()));
                        }
                        self.record_rejection();
                        return Poll::Ready(Err(CamelError::CircuitOpen(
                            "circuit breaker is open".into(),
                        )));
                    }
                    HalfOpenTransition::LostRace => continue,
                },
                STATE_HALF_OPEN_PROBE_HELD => {
                    // A probe is already in flight — reject this concurrent
                    // caller. Always Err (even with fallback): returning Ok
                    // here would let a 2nd caller reach call() → inner,
                    // bypassing the single-probe gate. The caller retries
                    // when after_result resolves the probe state.
                    self.record_rejection();
                    return Poll::Ready(Err(CamelError::CircuitOpen(
                        "circuit breaker is half-open (probe in flight)".into(),
                    )));
                }
                STATE_HALF_OPEN_PROBE_FREE => {
                    // Claim the probe slot and forward to inner. If inner
                    // returns Pending, release the claim so re-poll works.
                    if self
                        .state
                        .state
                        .compare_exchange(
                            STATE_HALF_OPEN_PROBE_FREE,
                            STATE_HALF_OPEN_PROBE_HELD,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        return self.poll_ready_as_probe(cx);
                    }
                    // Lost the claim race — re-load and decide again.
                }
                _ => continue,
            }
        }
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let mut decided_half_open = false;
        loop {
            match self.state.state.load(Ordering::Acquire) {
                STATE_CLOSED => break,
                STATE_OPEN => match self
                    .state
                    .try_transition_to_half_open(self.config.open_duration)
                {
                    HalfOpenTransition::Transitioned => {
                        tracing::info!("Circuit breaker transitioning from Open to HalfOpen");
                        decided_half_open = true;
                        break;
                    }
                    HalfOpenTransition::NotExpired => {
                        if let Some(mut fallback) = self.config.fallback.clone() {
                            return Box::pin(async move { fallback.call(exchange).await });
                        }
                        self.record_rejection();
                        return Box::pin(async {
                            Err(CamelError::CircuitOpen("circuit breaker is open".into()))
                        });
                    }
                    HalfOpenTransition::LostRace => continue,
                },
                STATE_HALF_OPEN_PROBE_HELD | STATE_HALF_OPEN_PROBE_FREE => {
                    // D-M1: the single-probe gate lives in poll_ready — 2nd callers
                    // get Err there and never reach call(). The probe caller (whose
                    // poll_ready claimed the probe) proceeds here to inner.
                    decided_half_open = true;
                    break;
                }
                _ => continue,
            }
        }

        // One load replaces the old second lock snapshot.
        let current_is_half_open = decided_half_open
            || matches!(
                self.state.state.load(Ordering::Acquire),
                STATE_HALF_OPEN_PROBE_HELD | STATE_HALF_OPEN_PROBE_FREE
            );

        // Clone inner service (Tower pattern) and state handle.
        let mut inner = self.inner.clone();
        let state = Arc::clone(&self.state);
        let config = self.config.clone();

        Box::pin(async move {
            let result = inner.call(exchange).await;

            match &result {
                Ok(_) => {
                    let s = state.state.load(Ordering::Acquire);
                    if s == STATE_CLOSED {
                        // Step 1 (bd rc-bnsxr) fast path: no RMW when already clean.
                        if state.consecutive_failures.load(Ordering::Acquire) == 0 {
                            return result;
                        }
                        state.consecutive_failures.swap(0, Ordering::Release);
                        return result;
                    }
                    if matches!(s, STATE_HALF_OPEN_PROBE_HELD | STATE_HALF_OPEN_PROBE_FREE) {
                        // Recovery runs under the transition lock: the counter
                        // reset and the Close CAS are serialized against every
                        // other slow-path transition, so an observer of CLOSED
                        // never sees a stale pre-open failure count.
                        state.with_transition(|_opened_at| {
                            // The CAS can only lose to the lockless probe admission
                            // (FREE -> HELD): every other HALF-code mutation holds
                            // this same lock. A lost recovery is retried so a
                            // concurrent admission cannot silently drop a completed
                            // probe's recovery; the log fires only when the
                            // transition happened.
                            let mut recovered = false;
                            for _ in 0..3 {
                                let s = state.state.load(Ordering::Acquire);
                                if s != STATE_HALF_OPEN_PROBE_HELD
                                    && s != STATE_HALF_OPEN_PROBE_FREE
                                {
                                    break; // state moved to OPEN/CLOSED — nothing to recover
                                }
                                if state
                                    .state
                                    .compare_exchange(
                                        s,
                                        STATE_CLOSED,
                                        Ordering::AcqRel,
                                        Ordering::Acquire,
                                    )
                                    .is_ok()
                                {
                                    state.consecutive_failures.swap(0, Ordering::Release);
                                    recovered = true;
                                    break;
                                }
                                // CAS lost only to the lockless probe admission
                                // (FREE -> HELD); reload and retry.
                            }
                            if recovered {
                                tracing::info!(
                                    "Circuit breaker transitioning from HalfOpen to Closed"
                                );
                            }
                            recovered
                        });
                    }
                    // s == STATE_OPEN: leave untouched — success does not close
                    // an open circuit.
                }
                Err(_) => {
                    if current_is_half_open {
                        // Half-open failure → reopen circuit.
                        state.with_transition(|opened_at| {
                            let s = state.state.load(Ordering::Acquire);
                            if !matches!(s, STATE_HALF_OPEN_PROBE_HELD | STATE_HALF_OPEN_PROBE_FREE)
                            {
                                return false;
                            }
                            tracing::warn!(
                                "Circuit breaker transitioning from HalfOpen to Open (probe failed)"
                            );
                            *opened_at = Some(Instant::now());
                            let _ = state.state.compare_exchange(
                                s,
                                STATE_OPEN,
                                Ordering::AcqRel,
                                Ordering::Acquire,
                            );
                            true
                        });
                    } else {
                        let s = state.state.load(Ordering::Acquire);
                        if s == STATE_CLOSED {
                            // Slow-path transitions serialize on the opened_at
                            // lock: qualification, timestamp publication, and
                            // the state CAS hold the lock together, and recovery
                            // (probe success) takes the same lock, so a threshold
                            // qualification can never be split across an
                            // open/expiry/recovery cycle — the invariant the old
                            // mutex gave per after_result call.
                            state.with_transition(|opened_at| {
                                let s = state.state.load(Ordering::Acquire);
                                if s != STATE_CLOSED {
                                    return false;
                                }
                                let prev =
                                    state.consecutive_failures.fetch_add(1, Ordering::AcqRel);
                                if prev + 1 >= config.failure_threshold {
                                    *opened_at = Some(Instant::now());
                                    if state
                                        .state
                                        .compare_exchange(
                                            STATE_CLOSED,
                                            STATE_OPEN,
                                            Ordering::AcqRel,
                                            Ordering::Acquire,
                                        )
                                        .is_ok()
                                    {
                                        tracing::warn!(
                                            threshold = config.failure_threshold,
                                            "Circuit breaker transitioning from Closed to Open (failure threshold reached)"
                                        );
                                    }
                                }
                                true
                            });
                        }
                    }
                }
            }

            result
        })
    }
}

impl<S> CircuitBreakerService<S>
where
    S: Service<Exchange, Response = Exchange, Error = CamelError> + Clone + Send + 'static,
    S::Future: Send,
{
    /// Forward to the inner service while holding the claimed half-open probe.
    /// If the inner service returns `Pending` we MUST release the probe claim
    /// so a re-poll can re-claim it; otherwise the breaker wedges until the
    /// half-open probe completes.
    fn poll_ready_as_probe(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), CamelError>> {
        match self.inner.poll_ready(cx) {
            Poll::Ready(result) => Poll::Ready(result),
            Poll::Pending => {
                // Release the probe claim so a re-poll can re-claim it; otherwise
                // the breaker wedges until the half-open probe completes.
                // (Holds the transition lock: recovery serializes against this
                // release — a recovery reload+CAS can no longer be invalidated by
                // it.)
                let _ = self.state.with_transition(|_| {
                    let s = self.state.state.load(Ordering::Acquire);
                    if s == STATE_HALF_OPEN_PROBE_HELD {
                        self.state
                            .state
                            .compare_exchange(
                                STATE_HALF_OPEN_PROBE_HELD,
                                STATE_HALF_OPEN_PROBE_FREE,
                                Ordering::AcqRel,
                                Ordering::Acquire,
                            )
                            .is_ok()
                    } else {
                        false
                    }
                });
                Poll::Pending
            }
        }
    }
}

// ── Gate ──────────────────────────────────────────────────────────────

/// Decision returned by [`CircuitBreakerGate::before_call`].
pub enum CircuitBreakerDecision {
    /// Circuit is closed or half-open — proceed with the pipeline call.
    Allow,
    /// Circuit is open but a fallback processor is configured.
    /// Call this processor instead of the main pipeline.
    Fallback(BoxProcessor),
    /// Circuit is open with no fallback — reject the call.
    Reject(CamelError),
}

/// Reusable circuit-breaker gate with explicit `before_call`/`after_result` API.
#[derive(Clone)]
pub struct CircuitBreakerGate {
    config: CircuitBreakerConfig,
    state: Arc<SharedBreakerState>,
    route_id: Arc<str>,
    metrics: Option<Arc<dyn MetricsCollector>>,
}

impl CircuitBreakerGate {
    /// `route_id` + `metrics` mirror [`CircuitBreakerLayer::new`]: the
    /// RouteChannelService breaker path counts its own fast-fail Reject
    /// arms on `camel_circuit_breaker_rejections_total{route}`.
    pub fn new(
        config: CircuitBreakerConfig,
        route_id: Arc<str>,
        metrics: Option<Arc<dyn MetricsCollector>>,
    ) -> Self {
        Self {
            config,
            state: Arc::new(SharedBreakerState::new()),
            route_id,
            metrics,
        }
    }

    /// Count one open-breaker fast-fail as a circuit-breaker rejection.
    fn record_rejection(&self) {
        if let Some(ref metrics) = self.metrics {
            metrics.increment_circuit_breaker_rejection(&self.route_id);
        }
    }

    pub fn before_call(&self) -> CircuitBreakerDecision {
        loop {
            match self.state.state.load(Ordering::Acquire) {
                STATE_CLOSED => return CircuitBreakerDecision::Allow,
                STATE_OPEN => match self
                    .state
                    .try_transition_to_half_open(self.config.open_duration)
                {
                    HalfOpenTransition::Transitioned => {
                        tracing::info!("Circuit breaker gate: Open → HalfOpen");
                        return CircuitBreakerDecision::Allow;
                    }
                    HalfOpenTransition::NotExpired => {
                        if let Some(ref fallback) = self.config.fallback {
                            return CircuitBreakerDecision::Fallback(fallback.clone());
                        }
                        self.record_rejection();
                        return CircuitBreakerDecision::Reject(CamelError::CircuitOpen(
                            "circuit breaker is open".into(),
                        ));
                    }
                    HalfOpenTransition::LostRace => continue,
                },
                STATE_HALF_OPEN_PROBE_HELD => {
                    if let Some(ref fallback) = self.config.fallback {
                        return CircuitBreakerDecision::Fallback(fallback.clone());
                    }
                    self.record_rejection();
                    return CircuitBreakerDecision::Reject(CamelError::CircuitOpen(
                        "circuit breaker is half-open (probe in flight)".into(),
                    ));
                }
                STATE_HALF_OPEN_PROBE_FREE => {
                    // Admit ourselves as the probe.
                    if self
                        .state
                        .state
                        .compare_exchange(
                            STATE_HALF_OPEN_PROBE_FREE,
                            STATE_HALF_OPEN_PROBE_HELD,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        return CircuitBreakerDecision::Allow;
                    }
                    // Lost the claim race — re-load and decide again.
                }
                _ => continue,
            }
        }
    }

    pub fn after_result(&self, result: &Result<Exchange, CamelError>) {
        match result {
            Ok(_) => {
                let s = self.state.state.load(Ordering::Acquire);
                if s == STATE_CLOSED {
                    // Step 1 (bd rc-bnsxr) fast path: no RMW when already clean.
                    if self.state.consecutive_failures.load(Ordering::Acquire) == 0 {
                        return;
                    }
                    self.state.consecutive_failures.swap(0, Ordering::Release);
                    return;
                }
                if matches!(s, STATE_HALF_OPEN_PROBE_HELD | STATE_HALF_OPEN_PROBE_FREE) {
                    // Recovery runs under the transition lock: the counter reset
                    // and the Close CAS are serialized against every other
                    // slow-path transition, so an observer of CLOSED never sees
                    // a stale pre-open failure count.
                    self.state.with_transition(|_opened_at| {
                        // The CAS can only lose to the lockless probe admission
                        // (FREE -> HELD): every other HALF-code mutation holds
                        // this same lock. A lost recovery is retried so a
                        // concurrent admission cannot silently drop a completed
                        // probe's recovery; the log fires only when the
                        // transition happened.
                        let mut recovered = false;
                        for _ in 0..3 {
                            let s = self.state.state.load(Ordering::Acquire);
                            if s != STATE_HALF_OPEN_PROBE_HELD && s != STATE_HALF_OPEN_PROBE_FREE {
                                break; // state moved to OPEN/CLOSED — nothing to recover
                            }
                            if self
                                .state
                                .state
                                .compare_exchange(
                                    s,
                                    STATE_CLOSED,
                                    Ordering::AcqRel,
                                    Ordering::Acquire,
                                )
                                .is_ok()
                            {
                                self.state.consecutive_failures.swap(0, Ordering::Release);
                                recovered = true;
                                break;
                            }
                            // CAS lost only to the lockless probe admission
                            // (FREE -> HELD); reload and retry.
                        }
                        if recovered {
                            tracing::info!("Circuit breaker gate: HalfOpen → Closed");
                        }
                        recovered
                    });
                }
                // s == STATE_OPEN: leave untouched — success does not close an
                // open circuit.
            }
            Err(_) => {
                let s = self.state.state.load(Ordering::Acquire);
                if matches!(s, STATE_HALF_OPEN_PROBE_HELD | STATE_HALF_OPEN_PROBE_FREE) {
                    self.state.with_transition(|opened_at| {
                        let s = self.state.state.load(Ordering::Acquire);
                        if !matches!(s, STATE_HALF_OPEN_PROBE_HELD | STATE_HALF_OPEN_PROBE_FREE) {
                            return false;
                        }
                        tracing::warn!("Circuit breaker gate: HalfOpen → Open (probe failed)");
                        *opened_at = Some(Instant::now());
                        let _ = self.state.state.compare_exchange(
                            s,
                            STATE_OPEN,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        );
                        true
                    });
                } else if s == STATE_CLOSED {
                    // Slow-path transitions serialize on the opened_at lock:
                    // qualification, timestamp publication, and the state CAS
                    // hold the lock together, and recovery (probe success) takes
                    // the same lock, so a threshold qualification can never be
                    // split across an open/expiry/recovery cycle — the invariant
                    // the old mutex gave per after_result call.
                    self.state.with_transition(|opened_at| {
                        let s = self.state.state.load(Ordering::Acquire);
                        if s != STATE_CLOSED {
                            return false;
                        }
                        let prev = self
                            .state
                            .consecutive_failures
                            .fetch_add(1, Ordering::AcqRel);
                        if prev + 1 >= self.config.failure_threshold {
                            *opened_at = Some(Instant::now());
                            if self
                                .state
                                .state
                                .compare_exchange(
                                    STATE_CLOSED,
                                    STATE_OPEN,
                                    Ordering::AcqRel,
                                    Ordering::Acquire,
                                )
                                .is_ok()
                            {
                                tracing::warn!(
                                    threshold = self.config.failure_threshold,
                                    "Circuit breaker gate: Closed → Open (failure threshold reached)"
                                );
                            }
                        }
                        true
                    });
                }
            }
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{BoxProcessor, BoxProcessorExt, Message};
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;
    use tokio::sync::Barrier;
    use tower::ServiceExt;

    #[derive(Debug, PartialEq, Eq)]
    enum ObservedState {
        Closed { consecutive_failures: u32 },
        Open,
        HalfOpen { probe_admitted: bool },
    }

    fn observe(shared: &SharedBreakerState) -> ObservedState {
        match shared.state.load(Ordering::Acquire) {
            STATE_CLOSED => ObservedState::Closed {
                consecutive_failures: shared.consecutive_failures.load(Ordering::Acquire),
            },
            STATE_OPEN => ObservedState::Open,
            STATE_HALF_OPEN_PROBE_HELD => ObservedState::HalfOpen {
                probe_admitted: true,
            },
            STATE_HALF_OPEN_PROBE_FREE => ObservedState::HalfOpen {
                probe_admitted: false,
            },
            _ => unreachable!("unknown breaker state"),
        }
    }

    fn make_exchange() -> Exchange {
        Exchange::new(Message::new("test"))
    }

    fn gate_with(config: CircuitBreakerConfig) -> CircuitBreakerGate {
        CircuitBreakerGate::new(config, Arc::from("test"), None)
    }

    fn ok_processor() -> BoxProcessor {
        BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }))
    }

    fn failing_processor() -> BoxProcessor {
        BoxProcessor::from_fn(|_ex| {
            Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
        })
    }

    fn fail_n_times(n: u32) -> BoxProcessor {
        let count = Arc::new(AtomicU32::new(0));
        BoxProcessor::from_fn(move |ex| {
            let count = Arc::clone(&count);
            Box::pin(async move {
                let c = count.fetch_add(1, Ordering::SeqCst);
                if c < n {
                    Err(CamelError::ProcessorError(format!("attempt {c}")))
                } else {
                    Ok(ex)
                }
            })
        })
    }

    fn tag_processor(tag: &'static str) -> BoxProcessor {
        BoxProcessor::from_fn(move |_ex| {
            Box::pin(async move {
                let mut out = make_exchange();
                out.input.body = tag.to_string().into();
                Ok(out)
            })
        })
    }

    /// 1. Circuit stays closed on success.
    #[tokio::test]
    async fn test_stays_closed_on_success() {
        let config = CircuitBreakerConfig::new().failure_threshold(3);
        let layer = CircuitBreakerLayer::new(config, Arc::from("test"), None);
        let mut svc = layer.layer(ok_processor());

        for _ in 0..5 {
            let result = svc.ready().await.unwrap().call(make_exchange()).await;
            assert!(result.is_ok());
        }

        // State should still be closed with 0 failures.
        assert_eq!(
            observe(&svc.state),
            ObservedState::Closed {
                consecutive_failures: 0
            }
        );
    }

    /// 2. Circuit opens after failure_threshold consecutive failures.
    #[tokio::test]
    async fn test_opens_after_failure_threshold() {
        let config = CircuitBreakerConfig::new().failure_threshold(3);
        let layer = CircuitBreakerLayer::new(config, Arc::from("test"), None);
        let mut svc = layer.layer(failing_processor());

        // Three consecutive failures should open the circuit.
        for _ in 0..3 {
            let result = svc.ready().await.unwrap().call(make_exchange()).await;
            assert!(result.is_err());
        }

        // The next poll_ready should return CircuitOpen error.
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let poll = Pin::new(&mut svc).poll_ready(&mut cx);
        match poll {
            Poll::Ready(Err(CamelError::CircuitOpen(_))) => {} // expected
            other => panic!("expected CircuitOpen error, got {other:?}"),
        }
    }

    /// 3. Circuit transitions to half-open after open_duration.
    #[tokio::test]
    async fn test_transitions_to_half_open_after_duration() {
        let config = CircuitBreakerConfig::new()
            .failure_threshold(2)
            .open_duration(Duration::from_millis(50));
        let layer = CircuitBreakerLayer::new(config, Arc::from("test"), None);
        // Use fail_n_times(2) so the first 2 calls fail (opening the circuit),
        // then the third (half-open probe) succeeds.
        let mut svc = layer.layer(fail_n_times(2));

        // Trigger 2 failures to open the circuit.
        for _ in 0..2 {
            let _ = svc.ready().await.unwrap().call(make_exchange()).await;
        }

        // Circuit is now open. Wait for open_duration to elapse.
        tokio::time::sleep(Duration::from_millis(60)).await;

        // poll_ready should transition to HalfOpen and succeed.
        let result = svc.ready().await.unwrap().call(make_exchange()).await;
        assert!(result.is_ok(), "half-open probe should succeed");

        // After successful probe, circuit should be back to Closed.
        assert_eq!(
            observe(&svc.state),
            ObservedState::Closed {
                consecutive_failures: 0
            }
        );
    }

    /// 4. Half-open failure reopens circuit.
    #[tokio::test]
    async fn test_half_open_failure_reopens() {
        let config = CircuitBreakerConfig::new()
            .failure_threshold(2)
            .open_duration(Duration::from_millis(50));
        let layer = CircuitBreakerLayer::new(config, Arc::from("test"), None);
        let mut svc = layer.layer(failing_processor());

        // Trigger 2 failures to open the circuit.
        for _ in 0..2 {
            let _ = svc.ready().await.unwrap().call(make_exchange()).await;
        }

        // Wait for open_duration to elapse, transitioning to HalfOpen.
        tokio::time::sleep(Duration::from_millis(60)).await;

        // Half-open probe fails → circuit reopens.
        let result = svc.ready().await.unwrap().call(make_exchange()).await;
        assert!(result.is_err());

        // Circuit should be open again.
        assert_eq!(observe(&svc.state), ObservedState::Open);
    }

    /// 5. Intermittent failures below threshold don't open circuit.
    #[tokio::test]
    async fn test_intermittent_failures_dont_open() {
        let config = CircuitBreakerConfig::new().failure_threshold(3);
        let layer = CircuitBreakerLayer::new(config, Arc::from("test"), None);

        // Alternate: fail, fail, success, fail, fail, success
        // The counter should reset on success, so threshold of 3 is never reached.
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = Arc::clone(&call_count);
        let inner = BoxProcessor::from_fn(move |ex| {
            let cc = Arc::clone(&cc);
            Box::pin(async move {
                let c = cc.fetch_add(1, Ordering::SeqCst);
                // Pattern: fail, fail, success, fail, fail, success
                if c % 3 == 2 {
                    Ok(ex)
                } else {
                    Err(CamelError::ProcessorError("intermittent".into()))
                }
            })
        });

        let mut svc = layer.layer(inner);

        for _ in 0..6 {
            let _ = svc.ready().await.unwrap().call(make_exchange()).await;
        }

        // Circuit should still be closed because successes reset the counter.
        assert!(matches!(observe(&svc.state), ObservedState::Closed { .. }));
    }

    #[tokio::test]
    async fn test_open_uses_fallback_when_configured() {
        let fallback = tag_processor("fallback");
        let config = CircuitBreakerConfig::new()
            .failure_threshold(1)
            .open_duration(Duration::from_secs(60))
            .fallback(fallback);
        let layer = CircuitBreakerLayer::new(config, Arc::from("test"), None);
        let mut svc = layer.layer(failing_processor());

        let _ = svc.ready().await.unwrap().call(make_exchange()).await;
        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange())
            .await
            .unwrap();
        assert_eq!(result.input.body.as_text(), Some("fallback"));
    }

    #[tokio::test]
    async fn test_open_without_fallback_returns_err() {
        let config = CircuitBreakerConfig::new()
            .failure_threshold(1)
            .open_duration(Duration::from_secs(60));
        let layer = CircuitBreakerLayer::new(config, Arc::from("test"), None);
        let mut svc = layer.layer(failing_processor());

        let _ = svc.ready().await.unwrap().call(make_exchange()).await;
        let result = svc.ready().await;
        assert!(matches!(result, Err(CamelError::CircuitOpen(_))));
    }

    // ── CircuitBreakerGate tests ──────────────────────────────────────────

    #[test]
    fn test_cb_gate_before_call_closed_allows() {
        let gate = gate_with(CircuitBreakerConfig {
            failure_threshold: 3,
            open_duration: Duration::from_secs(60),
            success_threshold: 1,
            fallback: None,
        });
        assert!(matches!(gate.before_call(), CircuitBreakerDecision::Allow));
    }

    #[test]
    fn test_cb_gate_records_failures_and_opens() {
        let gate = gate_with(CircuitBreakerConfig {
            failure_threshold: 2,
            open_duration: Duration::from_secs(60),
            success_threshold: 1,
            fallback: None,
        });
        gate.after_result(&Err(CamelError::ProcessorError("fail".into())));
        assert!(
            matches!(gate.before_call(), CircuitBreakerDecision::Allow),
            "still closed after 1 failure"
        );
        gate.after_result(&Err(CamelError::ProcessorError("fail".into())));
        assert!(
            matches!(gate.before_call(), CircuitBreakerDecision::Reject(_)),
            "should be open after 2 failures"
        );
    }

    #[tokio::test]
    async fn test_cb_gate_closes_on_success() {
        let gate = gate_with(CircuitBreakerConfig {
            failure_threshold: 1,
            open_duration: Duration::from_millis(1),
            success_threshold: 1,
            fallback: None,
        });
        gate.after_result(&Err(CamelError::ProcessorError("fail".into())));
        assert!(
            matches!(gate.before_call(), CircuitBreakerDecision::Reject(_)),
            "should be open"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(
            matches!(gate.before_call(), CircuitBreakerDecision::Allow),
            "should transition to half-open"
        );
        let ex = Exchange::new(Message::new("test"));
        gate.after_result(&Ok(ex));
        assert!(
            matches!(gate.before_call(), CircuitBreakerDecision::Allow),
            "should be closed again"
        );
    }

    #[tokio::test]
    async fn test_cb_gate_half_open_failure_reopens() {
        let gate = gate_with(CircuitBreakerConfig {
            failure_threshold: 1,
            open_duration: Duration::from_millis(1),
            success_threshold: 1,
            fallback: None,
        });
        // Open the circuit
        gate.after_result(&Err(CamelError::ProcessorError("fail".into())));
        assert!(
            matches!(gate.before_call(), CircuitBreakerDecision::Reject(_)),
            "should be open"
        );

        // Wait for open_duration to elapse → transitions to HalfOpen
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(
            matches!(gate.before_call(), CircuitBreakerDecision::Allow),
            "should be half-open now"
        );

        // Probe fails in HalfOpen → should reopen
        gate.after_result(&Err(CamelError::ProcessorError("probe fail".into())));
        assert!(
            matches!(gate.before_call(), CircuitBreakerDecision::Reject(_)),
            "should be open again after probe failure"
        );
    }

    #[test]
    fn test_cb_gate_open_with_fallback_returns_fallback() {
        let fallback = BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }));
        let gate = gate_with(CircuitBreakerConfig {
            failure_threshold: 1,
            open_duration: Duration::from_secs(60),
            success_threshold: 1,
            fallback: Some(fallback),
        });
        gate.after_result(&Err(CamelError::ProcessorError("fail".into())));
        assert!(
            matches!(gate.before_call(), CircuitBreakerDecision::Fallback(_)),
            "should return fallback when open"
        );
    }

    #[test]
    fn test_cb_gate_handled_error_counts_as_success() {
        let gate = gate_with(CircuitBreakerConfig {
            failure_threshold: 1,
            open_duration: Duration::from_secs(60),
            success_threshold: 1,
            fallback: None,
        });
        let ex = Exchange::new(Message::new("test"));
        gate.after_result(&Ok(ex));
        assert!(
            matches!(gate.before_call(), CircuitBreakerDecision::Allow),
            "handled error should not trip CB"
        );
    }

    // ── D-M1: half-open admits a single probe ─────────────────────────────

    /// Gate path: only the first caller in HalfOpen is admitted as the probe;
    /// every subsequent caller is rejected.
    #[test]
    fn gate_half_open_admits_only_one_probe() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            open_duration: Duration::from_millis(1),
            success_threshold: 1,
            fallback: None,
        };
        let gate = gate_with(config);
        // Trip: one failure → Open
        gate.after_result(&Err::<Exchange, CamelError>(CamelError::CircuitOpen(
            "boom".into(),
        )));
        std::thread::sleep(Duration::from_millis(10)); // past open_duration
        let d1 = gate.before_call();
        assert!(
            matches!(d1, CircuitBreakerDecision::Allow),
            "first probe must be admitted"
        );
        let d2 = gate.before_call();
        assert!(
            matches!(d2, CircuitBreakerDecision::Reject(_)),
            "2nd concurrent caller must be rejected"
        );
    }

    /// Service path: only the first `poll_ready` in HalfOpen is admitted as the
    /// probe; every subsequent `poll_ready` from a cloned service must be
    /// rejected with `CircuitOpen`.
    #[tokio::test]
    async fn service_half_open_admits_only_one_probe() {
        let config = CircuitBreakerConfig::new()
            .failure_threshold(1)
            .open_duration(Duration::from_millis(1));
        let layer = CircuitBreakerLayer::new(config, Arc::from("test"), None);
        let mut svc1 = layer.layer(failing_processor());
        let mut svc2 = svc1.clone();

        // Trip to Open
        let _ = svc1.ready().await.unwrap().call(make_exchange()).await;

        // Wait past open_duration
        tokio::time::sleep(Duration::from_millis(10)).await;

        // svc1 poll_ready: admitted as probe → Ready(Ok(()))
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let p1 = std::pin::Pin::new(&mut svc1).poll_ready(&mut cx);
        assert!(
            matches!(p1, Poll::Ready(Ok(()))),
            "first probe admitted, got {p1:?}"
        );

        // svc2 poll_ready: must be rejected
        let p2 = std::pin::Pin::new(&mut svc2).poll_ready(&mut cx);
        match p2 {
            Poll::Ready(Err(CamelError::CircuitOpen(_))) => {} // expected
            other => panic!("expected CircuitOpen error on 2nd probe, got {other:?}"),
        }
    }

    // ── bd rc-bnsxr item 3: deterministic concurrency coverage ────────────

    /// Inner service for the D-M1 release dance: its `poll_ready` is gated on
    /// a flag the test flips, and its first `call` fails so the breaker can be
    /// tripped through the normal failure path.
    #[derive(Clone)]
    struct ProbeReadyService {
        ready: Arc<AtomicBool>,
        calls: Arc<AtomicU32>,
    }

    impl Service<Exchange> for ProbeReadyService {
        type Response = Exchange;
        type Error = CamelError;
        type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), CamelError>> {
            if self.ready.load(Ordering::Acquire) {
                Poll::Ready(Ok(()))
            } else {
                Poll::Pending
            }
        }

        fn call(&mut self, _exchange: Exchange) -> Self::Future {
            let is_trip = self.calls.fetch_add(1, Ordering::AcqRel) == 0;
            Box::pin(async move {
                if is_trip {
                    Err(CamelError::ProcessorError("trip".into()))
                } else {
                    Ok(make_exchange())
                }
            })
        }
    }

    /// 1. A concurrent success storm never observes a failure and leaves the
    /// circuit Closed with a clean counter.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn concurrent_successes_stay_closed_and_clean() {
        const TASKS: usize = 8;
        const ITERS: usize = 5_000;

        let gate = gate_with(
            CircuitBreakerConfig::new()
                .failure_threshold(8)
                .open_duration(Duration::from_secs(3_600)),
        );
        let barrier = Arc::new(Barrier::new(TASKS));
        let mut handles = Vec::with_capacity(TASKS);

        for _ in 0..TASKS {
            let gate = gate.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(tokio::spawn(async move {
                tokio::time::timeout(Duration::from_secs(5), barrier.wait())
                    .await
                    .expect("barrier wait must not deadlock");
                for i in 0..ITERS {
                    match gate.before_call() {
                        CircuitBreakerDecision::Allow => {}
                        CircuitBreakerDecision::Fallback(_) => panic!(
                            "no fallback configured, yet iteration {i} got Fallback: state={:?}",
                            observe(&gate.state)
                        ),
                        CircuitBreakerDecision::Reject(err) => panic!(
                            "success storm iteration {i} was rejected: {err:?}, state={:?}",
                            observe(&gate.state)
                        ),
                    }
                    gate.after_result(&Ok(make_exchange()));
                }
            }));
        }
        for handle in handles {
            handle
                .await
                .expect("success storm task panicked before joining");
        }

        assert_eq!(
            observe(&gate.state),
            ObservedState::Closed {
                consecutive_failures: 0
            },
            "40k concurrent successes must never trip the breaker or dirty the counter"
        );
    }

    /// 2. Under a barrier-start race, exactly one caller claims the half-open
    /// probe; every other concurrent caller is rejected. Repeats 25 recovery
    /// cycles to shake scheduling skew.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn concurrent_before_call_admits_exactly_one_probe() {
        const TASKS: usize = 8;
        const ROUNDS: usize = 25;

        let open_duration = Duration::from_millis(10);
        let gate = gate_with(
            CircuitBreakerConfig::new()
                .failure_threshold(1)
                .open_duration(open_duration),
        );

        for round in 0..ROUNDS {
            gate.after_result(&Err::<Exchange, CamelError>(CamelError::ProcessorError(
                format!("trip {round}"),
            )));
            assert_eq!(
                observe(&gate.state),
                ObservedState::Open,
                "round {round}: a single failure must trip the threshold(1) breaker"
            );

            tokio::time::sleep(open_duration + Duration::from_millis(10)).await; // allow-test-sleep: expiry timer is the behavior under test

            let barrier = Arc::new(Barrier::new(TASKS));
            let mut handles = Vec::with_capacity(TASKS);
            for _ in 0..TASKS {
                let gate = gate.clone();
                let barrier = Arc::clone(&barrier);
                handles.push(tokio::spawn(async move {
                    tokio::time::timeout(Duration::from_secs(5), barrier.wait())
                        .await
                        .expect("barrier wait must not deadlock");
                    gate.before_call()
                }));
            }

            let mut allows = 0usize;
            let mut rejects = 0usize;
            for handle in handles {
                match handle.await.expect("probe race task panicked") {
                    CircuitBreakerDecision::Allow => allows += 1,
                    CircuitBreakerDecision::Reject(_) => rejects += 1,
                    CircuitBreakerDecision::Fallback(_) => {
                        panic!("round {round}: no fallback is configured")
                    }
                }
            }
            assert_eq!(
                allows, 1,
                "round {round}: exactly one concurrent caller may be admitted as the probe"
            );
            assert_eq!(
                rejects, 7,
                "round {round}: every non-probe caller must be rejected"
            );
            assert_eq!(
                observe(&gate.state),
                ObservedState::HalfOpen {
                    probe_admitted: true
                },
                "round {round}: the admitted probe must hold the single probe slot"
            );

            // Resolve the probe so the next round starts from Closed.
            gate.after_result(&Ok(make_exchange()));
            assert_eq!(
                observe(&gate.state),
                ObservedState::Closed {
                    consecutive_failures: 0
                },
                "round {round}: the successful probe must close the circuit"
            );
        }
    }

    /// 3. Exactly-threshold concurrent failures must all be counted: no lost
    /// increment may leave the circuit Closed, and no extra transition may
    /// double-open it. A failing half-open probe then reopens it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn concurrent_failures_open_exactly_at_threshold() {
        // Phase A pins threshold atomicity (exactly-threshold concurrent failures
        // must open; a lost increment would leave it Closed) behind a 1-hour open
        // window so the post-join assertions carry no timing deadline. Phase B
        // covers probe-fail reopen on a fresh short-expiry gate, single-tasked.

        // Phase A — threshold qualification, deterministic.
        const TASKS: usize = 8;
        const PER_TASK: usize = 8;
        const THRESHOLD: u32 = 64;

        let gate_a = gate_with(
            CircuitBreakerConfig::new()
                .failure_threshold(THRESHOLD)
                .open_duration(Duration::from_secs(60 * 60)),
        );
        let barrier = Arc::new(Barrier::new(TASKS));
        let mut handles = Vec::with_capacity(TASKS);

        for _ in 0..TASKS {
            let gate = gate_a.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(tokio::spawn(async move {
                tokio::time::timeout(Duration::from_secs(5), barrier.wait())
                    .await
                    .expect("barrier wait must not deadlock");
                for _ in 0..PER_TASK {
                    gate.after_result(&Err::<Exchange, CamelError>(CamelError::ProcessorError(
                        "boom".into(),
                    )));
                }
            }));
        }
        for handle in handles {
            handle.await.expect("threshold storm task panicked");
        }

        assert_eq!(
            observe(&gate_a.state),
            ObservedState::Open,
            "exactly {THRESHOLD} concurrent failures must open the circuit (no lost increment)"
        );
        assert!(
            matches!(gate_a.before_call(), CircuitBreakerDecision::Reject(_)),
            "a freshly opened circuit must reject before expiry"
        );

        // Phase B — probe-fail reopen on a fresh short-expiry gate, no concurrency.
        let open_duration = Duration::from_millis(10);
        let gate_b = gate_with(
            CircuitBreakerConfig::new()
                .failure_threshold(1)
                .open_duration(open_duration),
        );
        gate_b.after_result(&Err::<Exchange, CamelError>(CamelError::ProcessorError(
            "boom".into(),
        )));
        assert_eq!(
            observe(&gate_b.state),
            ObservedState::Open,
            "a single failure must trip a threshold-1 circuit"
        );

        tokio::time::sleep(open_duration + Duration::from_millis(10)).await; // allow-test-sleep: expiry timer is the behavior under test
        assert!(
            matches!(gate_b.before_call(), CircuitBreakerDecision::Allow),
            "an expired circuit must admit the probe"
        );
        gate_b.after_result(&Err::<Exchange, CamelError>(CamelError::ProcessorError(
            "probe".into(),
        )));
        assert_eq!(
            observe(&gate_b.state),
            ObservedState::Open,
            "a failed probe must reopen the circuit"
        );
    }

    /// 4. Expiry reopens the probe path exactly once, and expiry must never
    /// re-open a circuit that a successful probe already closed.
    #[tokio::test]
    async fn expiry_reopens_the_probe_path() {
        let open_duration = Duration::from_millis(20);
        let gate = gate_with(
            CircuitBreakerConfig::new()
                .failure_threshold(1)
                .open_duration(open_duration),
        );

        gate.after_result(&Err::<Exchange, CamelError>(CamelError::ProcessorError(
            "boom".into(),
        )));
        assert!(
            matches!(gate.before_call(), CircuitBreakerDecision::Reject(_)),
            "an open circuit must reject before expiry"
        );

        tokio::time::sleep(Duration::from_millis(50)).await; // allow-test-sleep: expiry timer is the behavior under test
        assert!(
            matches!(gate.before_call(), CircuitBreakerDecision::Allow),
            "an expired circuit must admit the first caller as the probe"
        );
        assert_eq!(
            observe(&gate.state),
            ObservedState::HalfOpen {
                probe_admitted: true
            },
            "the expiry winner must hold the probe slot"
        );
        assert!(
            matches!(gate.before_call(), CircuitBreakerDecision::Reject(_)),
            "the single-probe gate must reject an immediate second caller"
        );

        gate.after_result(&Ok(make_exchange()));
        assert_eq!(
            observe(&gate.state),
            ObservedState::Closed {
                consecutive_failures: 0
            },
            "a successful probe must close the circuit"
        );

        // No failures: waiting past open_duration again must not re-open it.
        tokio::time::sleep(Duration::from_millis(50)).await; // allow-test-sleep: expiry timer is the behavior under test
        assert!(
            matches!(gate.before_call(), CircuitBreakerDecision::Allow),
            "expiry must never re-open a Closed circuit"
        );
        assert_eq!(
            observe(&gate.state),
            ObservedState::Closed {
                consecutive_failures: 0
            },
            "the circuit must stay Closed after the probe recovery"
        );
    }

    /// 5. A half-open success closes the circuit and resets the counter, and
    /// further successes keep it Closed.
    #[tokio::test]
    async fn success_after_half_open_closes_and_resets_counter() {
        let open_duration = Duration::from_millis(10);
        let gate = gate_with(
            CircuitBreakerConfig::new()
                .failure_threshold(2)
                .open_duration(open_duration),
        );

        for _ in 0..2 {
            gate.after_result(&Err::<Exchange, CamelError>(CamelError::ProcessorError(
                "boom".into(),
            )));
        }
        assert_eq!(
            observe(&gate.state),
            ObservedState::Open,
            "threshold(2) must open after two failures"
        );

        tokio::time::sleep(Duration::from_millis(20)).await; // allow-test-sleep: expiry timer is the behavior under test
        assert!(
            matches!(gate.before_call(), CircuitBreakerDecision::Allow),
            "an expired circuit must admit the probe"
        );
        gate.after_result(&Ok(make_exchange()));
        assert_eq!(
            observe(&gate.state),
            ObservedState::Closed {
                consecutive_failures: 0
            },
            "a successful probe must close the circuit and reset the counter"
        );

        for _ in 0..100 {
            gate.after_result(&Ok(make_exchange()));
        }
        assert_eq!(
            observe(&gate.state),
            ObservedState::Closed {
                consecutive_failures: 0
            },
            "further successes must keep the recovered circuit Closed with a zeroed counter"
        );
    }

    /// 6. D-M1 release dance: a `poll_ready` Pending from the inner service
    /// releases the probe claim so a re-poll can re-claim it.
    #[tokio::test]
    async fn poll_ready_pending_releases_the_probe_claim() {
        let open_duration = Duration::from_millis(10);
        let ready = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicU32::new(0));
        let layer = CircuitBreakerLayer::new(
            CircuitBreakerConfig::new()
                .failure_threshold(1)
                .open_duration(open_duration),
            Arc::from("test"),
            None,
        );
        let mut svc = layer.layer(ProbeReadyService {
            ready: Arc::clone(&ready),
            calls: Arc::clone(&calls),
        });

        // Trip the breaker through the ordinary failure path. The inner
        // service's first call fails; readiness is irrelevant to `call`.
        let trip = svc.call(make_exchange()).await;
        assert!(trip.is_err(), "the first inner call must fail to trip");
        assert_eq!(
            observe(&svc.state),
            ObservedState::Open,
            "one failed call must trip the threshold(1) breaker"
        );
        tokio::time::sleep(Duration::from_millis(20)).await; // allow-test-sleep: expiry timer is the behavior under test

        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        // First poll: expiry claims the probe, inner says Pending, so the
        // claim must be released back to FREE.
        let first = Pin::new(&mut svc).poll_ready(&mut cx);
        assert!(
            matches!(first, Poll::Pending),
            "not-ready inner service must surface Pending, got {first:?}"
        );
        assert_eq!(
            observe(&svc.state),
            ObservedState::HalfOpen {
                probe_admitted: false
            },
            "Pending must release the probe claim (HELD -> FREE)"
        );

        // Re-poll once the inner service is ready: the claim is re-taken and
        // the breaker admits the probe.
        ready.store(true, Ordering::Release);
        let second = Pin::new(&mut svc).poll_ready(&mut cx);
        assert!(
            matches!(second, Poll::Ready(Ok(()))),
            "a ready inner service must let the probe proceed, got {second:?}"
        );
        assert_eq!(
            observe(&svc.state),
            ObservedState::HalfOpen {
                probe_admitted: true
            },
            "the re-poll must re-claim the probe slot (FREE -> HELD)"
        );

        // Complete the probe: the second inner call succeeds and closes.
        let result = svc.call(make_exchange()).await;
        assert!(result.is_ok(), "the half-open probe must succeed");
        assert_eq!(
            observe(&svc.state),
            ObservedState::Closed {
                consecutive_failures: 0
            },
            "a successful probe must close the circuit and reset the counter"
        );
    }
}
