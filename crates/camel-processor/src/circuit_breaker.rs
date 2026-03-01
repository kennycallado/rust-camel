use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Instant;

use tower::{Layer, Service};

use camel_api::{CamelError, CircuitBreakerConfig, Exchange};

// ── State ──────────────────────────────────────────────────────────────

enum CircuitState {
    Closed { consecutive_failures: u32 },
    Open { opened_at: Instant },
    HalfOpen,
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
                    *state = CircuitState::HalfOpen;
                    drop(state);
                    self.inner.poll_ready(cx)
                } else {
                    Poll::Ready(Err(CamelError::CircuitOpen(
                        "circuit breaker is open".into(),
                    )))
                }
            }
            CircuitState::HalfOpen => {
                drop(state);
                self.inner.poll_ready(cx)
            }
        }
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        // Clone inner service (Tower pattern) and state handle.
        let mut inner = self.inner.clone();
        let state = Arc::clone(&self.state);
        let config = self.config.clone();

        // Snapshot the current state before calling (briefly lock).
        let current_is_half_open = matches!(
            *state.lock().unwrap_or_else(|e| e.into_inner()),
            CircuitState::HalfOpen
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
}
