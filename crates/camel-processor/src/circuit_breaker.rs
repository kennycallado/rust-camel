use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Instant;

use tower::{Layer, Service};

use camel_api::{BoxProcessor, CamelError, CircuitBreakerConfig, Exchange};

// ── State ──────────────────────────────────────────────────────────────

enum CircuitState {
    Closed {
        consecutive_failures: u32,
    },
    Open {
        opened_at: Instant,
    },
    /// `probe_admitted == true` means a probe request is in flight; subsequent
    /// concurrent callers must be rejected until the probe completes.
    /// `probe_admitted == false` means no probe is in flight and the next
    /// caller is admitted as the probe.
    HalfOpen {
        probe_admitted: bool,
    },
}

// ── Layer ──────────────────────────────────────────────────────────────

/// Tower Layer that wraps an inner service with circuit-breaker logic.
#[derive(Clone)]
pub struct CircuitBreakerLayer {
    config: CircuitBreakerConfig,
    state: Arc<Mutex<CircuitState>>,
}

impl CircuitBreakerLayer {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: Arc::new(Mutex::new(CircuitState::Closed {
                consecutive_failures: 0,
            })),
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
        }
    }
}

// ── Service ────────────────────────────────────────────────────────────

/// Tower Service implementing the circuit-breaker pattern.
pub struct CircuitBreakerService<S> {
    inner: S,
    config: CircuitBreakerConfig,
    state: Arc<Mutex<CircuitState>>,
}

impl<S: Clone> Clone for CircuitBreakerService<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            config: self.config.clone(),
            state: Arc::clone(&self.state),
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
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match *state {
            CircuitState::Closed { .. } => {
                drop(state);
                self.inner.poll_ready(cx)
            }
            CircuitState::Open { opened_at } => {
                if opened_at.elapsed() >= self.config.open_duration {
                    tracing::info!("Circuit breaker transitioning from Open to HalfOpen");
                    *state = CircuitState::HalfOpen {
                        probe_admitted: true,
                    };
                    drop(state);
                    // If the inner service returns Pending we MUST release the
                    // probe claim so a re-poll can re-claim it; otherwise the
                    // breaker wedges until the half-open probe completes.
                    match self.inner.poll_ready(cx) {
                        Poll::Ready(result) => Poll::Ready(result),
                        Poll::Pending => {
                            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                            if matches!(*st, CircuitState::HalfOpen { .. }) {
                                *st = CircuitState::HalfOpen {
                                    probe_admitted: false,
                                };
                            }
                            Poll::Pending
                        }
                    }
                } else if self.config.fallback.is_some() {
                    Poll::Ready(Ok(()))
                } else {
                    Poll::Ready(Err(CamelError::CircuitOpen(
                        "circuit breaker is open".into(),
                    )))
                }
            }
            CircuitState::HalfOpen { probe_admitted } => {
                if probe_admitted {
                    // A probe is already in flight — reject this concurrent
                    // caller. Always Err (even with fallback): returning Ok
                    // here would let a 2nd caller reach call() → inner,
                    // bypassing the single-probe gate. The caller retries
                    // when after_result resolves the probe state.
                    drop(state);
                    Poll::Ready(Err(CamelError::CircuitOpen(
                        "circuit breaker is half-open (probe in flight)".into(),
                    )))
                } else {
                    // Claim the probe slot and forward to inner. If inner
                    // returns Pending, release the claim so re-poll works.
                    *state = CircuitState::HalfOpen {
                        probe_admitted: true,
                    };
                    drop(state);
                    match self.inner.poll_ready(cx) {
                        Poll::Ready(result) => Poll::Ready(result),
                        Poll::Pending => {
                            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                            if matches!(*st, CircuitState::HalfOpen { .. }) {
                                *st = CircuitState::HalfOpen {
                                    probe_admitted: false,
                                };
                            }
                            Poll::Pending
                        }
                    }
                }
            }
        }
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let CircuitState::Open { opened_at } = *st {
                if opened_at.elapsed() < self.config.open_duration {
                    if let Some(mut fallback) = self.config.fallback.clone() {
                        return Box::pin(async move { fallback.call(exchange).await });
                    }
                    return Box::pin(async {
                        Err(CamelError::CircuitOpen("circuit breaker is open".into()))
                    });
                }

                tracing::info!("Circuit breaker transitioning from Open to HalfOpen");
                *st = CircuitState::HalfOpen {
                    probe_admitted: true,
                };
            }
            // D-M1: the single-probe gate lives in poll_ready — 2nd callers
            // get Err there and never reach call(). The probe caller (whose
            // poll_ready set probe_admitted: true) proceeds here to inner.
        }

        // Clone inner service (Tower pattern) and state handle.
        let mut inner = self.inner.clone();
        let state = Arc::clone(&self.state);
        let config = self.config.clone();

        // Snapshot the current state before calling (briefly lock).
        let current_is_half_open = matches!(
            *state.lock().unwrap_or_else(|e| e.into_inner()),
            CircuitState::HalfOpen { .. }
        );

        Box::pin(async move {
            let result = inner.call(exchange).await;

            // Update state based on result (briefly lock).
            let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
            match &result {
                Ok(_) => {
                    // Success → reset to Closed.
                    if current_is_half_open {
                        tracing::info!("Circuit breaker transitioning from HalfOpen to Closed");
                    }
                    *st = CircuitState::Closed {
                        consecutive_failures: 0,
                    };
                }
                Err(_) => {
                    if current_is_half_open {
                        // Half-open failure → reopen circuit.
                        tracing::warn!(
                            "Circuit breaker transitioning from HalfOpen to Open (probe failed)"
                        );
                        *st = CircuitState::Open {
                            opened_at: Instant::now(),
                        };
                    } else if let CircuitState::Closed {
                        consecutive_failures,
                    } = &mut *st
                    {
                        *consecutive_failures += 1;
                        if *consecutive_failures >= config.failure_threshold {
                            tracing::warn!(
                                threshold = config.failure_threshold,
                                "Circuit breaker transitioning from Closed to Open (failure threshold reached)"
                            );
                            *st = CircuitState::Open {
                                opened_at: Instant::now(),
                            };
                        }
                    }
                }
            }

            result
        })
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
    state: Arc<Mutex<CircuitState>>,
}

impl CircuitBreakerGate {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: Arc::new(Mutex::new(CircuitState::Closed {
                consecutive_failures: 0,
            })),
        }
    }

    pub fn before_call(&self) -> CircuitBreakerDecision {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match *state {
            CircuitState::Closed { .. } => CircuitBreakerDecision::Allow,
            CircuitState::Open { opened_at } => {
                if opened_at.elapsed() >= self.config.open_duration {
                    tracing::info!("Circuit breaker gate: Open → HalfOpen");
                    *state = CircuitState::HalfOpen {
                        probe_admitted: true,
                    };
                    CircuitBreakerDecision::Allow
                } else if let Some(ref fallback) = self.config.fallback {
                    CircuitBreakerDecision::Fallback(fallback.clone())
                } else {
                    CircuitBreakerDecision::Reject(CamelError::CircuitOpen(
                        "circuit breaker is open".into(),
                    ))
                }
            }
            CircuitState::HalfOpen { probe_admitted } => {
                if probe_admitted {
                    if let Some(ref fallback) = self.config.fallback {
                        CircuitBreakerDecision::Fallback(fallback.clone())
                    } else {
                        CircuitBreakerDecision::Reject(CamelError::CircuitOpen(
                            "circuit breaker is half-open (probe in flight)".into(),
                        ))
                    }
                } else {
                    *state = CircuitState::HalfOpen {
                        probe_admitted: true,
                    };
                    CircuitBreakerDecision::Allow
                }
            }
        }
    }

    pub fn after_result(&self, result: &Result<Exchange, CamelError>) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let current_is_half_open = matches!(*st, CircuitState::HalfOpen { .. });
        match result {
            Ok(_) => {
                if current_is_half_open {
                    tracing::info!("Circuit breaker gate: HalfOpen → Closed");
                }
                *st = CircuitState::Closed {
                    consecutive_failures: 0,
                };
            }
            Err(_) => {
                if current_is_half_open {
                    tracing::warn!("Circuit breaker gate: HalfOpen → Open (probe failed)");
                    *st = CircuitState::Open {
                        opened_at: Instant::now(),
                    };
                } else if let CircuitState::Closed {
                    consecutive_failures,
                } = &mut *st
                {
                    *consecutive_failures += 1;
                    if *consecutive_failures >= self.config.failure_threshold {
                        tracing::warn!(
                            threshold = self.config.failure_threshold,
                            "Circuit breaker gate: Closed → Open (failure threshold reached)"
                        );
                        *st = CircuitState::Open {
                            opened_at: Instant::now(),
                        };
                    }
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
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;
    use tower::ServiceExt;

    fn make_exchange() -> Exchange {
        Exchange::new(Message::new("test"))
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
        let layer = CircuitBreakerLayer::new(config);
        let mut svc = layer.layer(ok_processor());

        for _ in 0..5 {
            let result = svc.ready().await.unwrap().call(make_exchange()).await;
            assert!(result.is_ok());
        }

        // State should still be closed with 0 failures.
        let state = svc.state.lock().unwrap();
        match *state {
            CircuitState::Closed {
                consecutive_failures,
            } => assert_eq!(consecutive_failures, 0),
            _ => panic!("expected Closed state"),
        }
    }

    /// 2. Circuit opens after failure_threshold consecutive failures.
    #[tokio::test]
    async fn test_opens_after_failure_threshold() {
        let config = CircuitBreakerConfig::new().failure_threshold(3);
        let layer = CircuitBreakerLayer::new(config);
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
        let layer = CircuitBreakerLayer::new(config);
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
        let state = svc.state.lock().unwrap();
        match *state {
            CircuitState::Closed {
                consecutive_failures,
            } => assert_eq!(consecutive_failures, 0),
            _ => panic!("expected Closed state after successful half-open probe"),
        }
    }

    /// 4. Half-open failure reopens circuit.
    #[tokio::test]
    async fn test_half_open_failure_reopens() {
        let config = CircuitBreakerConfig::new()
            .failure_threshold(2)
            .open_duration(Duration::from_millis(50));
        let layer = CircuitBreakerLayer::new(config);
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
        let state = svc.state.lock().unwrap();
        match *state {
            CircuitState::Open { .. } => {} // expected
            _ => panic!("expected Open state after half-open failure"),
        }
    }

    /// 5. Intermittent failures below threshold don't open circuit.
    #[tokio::test]
    async fn test_intermittent_failures_dont_open() {
        let config = CircuitBreakerConfig::new().failure_threshold(3);
        let layer = CircuitBreakerLayer::new(config);

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
        let state = svc.state.lock().unwrap();
        match *state {
            CircuitState::Closed { .. } => {} // expected
            _ => panic!("expected circuit to remain Closed"),
        }
    }

    #[tokio::test]
    async fn test_open_uses_fallback_when_configured() {
        let fallback = tag_processor("fallback");
        let config = CircuitBreakerConfig::new()
            .failure_threshold(1)
            .open_duration(Duration::from_secs(60))
            .fallback(fallback);
        let layer = CircuitBreakerLayer::new(config);
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
        let layer = CircuitBreakerLayer::new(config);
        let mut svc = layer.layer(failing_processor());

        let _ = svc.ready().await.unwrap().call(make_exchange()).await;
        let result = svc.ready().await;
        assert!(matches!(result, Err(CamelError::CircuitOpen(_))));
    }

    // ── CircuitBreakerGate tests ──────────────────────────────────────────

    #[test]
    fn test_cb_gate_before_call_closed_allows() {
        let gate = CircuitBreakerGate::new(CircuitBreakerConfig {
            failure_threshold: 3,
            open_duration: Duration::from_secs(60),
            success_threshold: 1,
            fallback: None,
        });
        assert!(matches!(gate.before_call(), CircuitBreakerDecision::Allow));
    }

    #[test]
    fn test_cb_gate_records_failures_and_opens() {
        let gate = CircuitBreakerGate::new(CircuitBreakerConfig {
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
        let gate = CircuitBreakerGate::new(CircuitBreakerConfig {
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
        let gate = CircuitBreakerGate::new(CircuitBreakerConfig {
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
        let gate = CircuitBreakerGate::new(CircuitBreakerConfig {
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
        let gate = CircuitBreakerGate::new(CircuitBreakerConfig {
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
        let gate = CircuitBreakerGate::new(config);
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
        let layer = CircuitBreakerLayer::new(config);
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
}
