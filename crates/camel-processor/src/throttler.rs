use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tower::Service;

use camel_api::CAMEL_STOP;
use camel_api::{
    BoxProcessor, CamelError, ConfigValidationError, Exchange, ThrottleStrategy, ThrottlerConfig,
    Value,
};

pub struct RateLimiter {
    tokens: f64,
    max_tokens: f64,
    refill_rate: f64,
    last_refill: Instant,
}

impl RateLimiter {
    fn new(max_requests: usize, period: Duration) -> Self {
        let refill_rate = max_requests as f64 / period.as_secs_f64();
        Self {
            tokens: max_requests as f64,
            max_tokens: max_requests as f64,
            refill_rate,
            last_refill: Instant::now(),
        }
    }

    fn try_acquire(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        if elapsed > 0.0 {
            self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.max_tokens);
            self.last_refill = now;
        }
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    fn time_until_next_token(&self) -> Duration {
        if self.tokens >= 1.0 {
            Duration::ZERO
        } else {
            let tokens_needed = 1.0 - self.tokens;
            Duration::from_secs_f64(tokens_needed / self.refill_rate)
        }
    }
}

#[derive(Clone)]
pub struct ThrottlerService {
    config: ThrottlerConfig,
    limiter: std::sync::Arc<Mutex<RateLimiter>>,
    next: BoxProcessor,
}

impl ThrottlerService {
    /// Construct a throttler. **Panics** if `period` is zero or
    /// `max_requests` is zero. Prefer `try_new` for fallible construction.
    pub fn new(config: ThrottlerConfig, next: BoxProcessor) -> Self {
        // Intentional panic-on-invariant contract for pre-validated
        // configs; try_new is the fallible path (D-M8).
        Self::try_new(config, next).expect("ThrottlerService::new invariants violated") // allow-unwrap
    }

    /// Fallible constructor. Returns `Err(CamelError::Config)` if
    /// `config.period` is `Duration::ZERO` or `config.max_requests == 0`.
    /// D-M8 fix: zero `max_requests` used to panic at first throttled
    /// message via `1.0/0.0 = inf` in `Duration::from_secs_f64`. Now the
    /// construction fails closed with a config error so the route never
    /// starts with an unsafe throttler.
    pub fn try_new(config: ThrottlerConfig, next: BoxProcessor) -> Result<Self, CamelError> {
        if config.period == Duration::ZERO {
            return Err(CamelError::Config(
                "ThrottlerConfig.period must be > 0".to_string(),
            ));
        }
        if config.max_requests == 0 {
            return Err(CamelError::from(
                ConfigValidationError::ThrottlerMaxRequestsZero,
            ));
        }
        let limiter = RateLimiter::new(config.max_requests, config.period);
        Ok(Self {
            config,
            limiter: std::sync::Arc::new(Mutex::new(limiter)),
            next,
        })
    }
}

impl Service<Exchange> for ThrottlerService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.next.poll_ready(cx)
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let config = self.config.clone();
        let limiter = self.limiter.clone();
        let mut next = self.next.clone();

        Box::pin(async move {
            let acquired = {
                let mut limiter = limiter.lock().unwrap(); // allow-unwrap
                limiter.try_acquire()
            };

            if acquired {
                next.call(exchange).await
            } else {
                match config.strategy {
                    ThrottleStrategy::Delay => {
                        loop {
                            let wait_time = {
                                let limiter = limiter.lock().unwrap(); // allow-unwrap
                                limiter.time_until_next_token()
                            };
                            if wait_time > Duration::ZERO {
                                tokio::time::sleep(wait_time).await;
                            }
                            let acquired = {
                                let mut limiter = limiter.lock().unwrap(); // allow-unwrap
                                limiter.try_acquire()
                            };
                            if acquired {
                                break;
                            }
                            // Yield to avoid tight spinning when concurrent tasks
                            // wake simultaneously and contend for the same token.
                            tokio::task::yield_now().await;
                        }
                        next.call(exchange).await
                    }
                    ThrottleStrategy::Reject => Err(CamelError::ProcessorError(
                        "Throttled: rate limit exceeded".to_string(),
                    )),
                    ThrottleStrategy::Drop => {
                        exchange.set_property(CAMEL_STOP, Value::Bool(true));
                        Ok(exchange)
                    }
                }
            }
        })
    }
}

/// Outcome-aware throttle segment (ADR-0025).
///
/// Wraps a `ThrottlerConfig` + shared `RateLimiter` + child sub-pipeline body.
/// Unlike `ThrottlerService` (which operates at the Tower layer),
/// `ThrottleSegment` correctly propagates `PipelineOutcome::Stopped` / `Failed`
/// from the body.
pub struct ThrottleSegment {
    pub config: ThrottlerConfig,
    pub limiter: std::sync::Arc<std::sync::Mutex<RateLimiter>>,
    pub body: camel_api::OutcomeSegment,
}

impl ThrottleSegment {
    /// Construct a throttle segment. **Panics** if invariants are violated.
    /// Prefer `try_new`.
    pub fn new(config: ThrottlerConfig, body: camel_api::OutcomeSegment) -> Self {
        // Intentional panic-on-invariant contract for pre-validated
        // configs; try_new is the fallible path (D-M8).
        Self::try_new(config, body).expect("ThrottleSegment::new invariants violated") // allow-unwrap
    }

    /// Fallible constructor. Same validation as `ThrottlerService::try_new`.
    pub fn try_new(
        config: ThrottlerConfig,
        body: camel_api::OutcomeSegment,
    ) -> Result<Self, CamelError> {
        if config.period == Duration::ZERO {
            return Err(CamelError::Config(
                "ThrottlerConfig.period must be > 0".to_string(),
            ));
        }
        if config.max_requests == 0 {
            return Err(CamelError::from(
                ConfigValidationError::ThrottlerMaxRequestsZero,
            ));
        }
        Ok(Self {
            limiter: std::sync::Arc::new(std::sync::Mutex::new(RateLimiter::new(
                config.max_requests,
                config.period,
            ))),
            config,
            body,
        })
    }
}

impl Clone for ThrottleSegment {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            limiter: std::sync::Arc::clone(&self.limiter),
            body: self.body.clone(),
        }
    }
}

impl camel_api::OutcomePipeline for ThrottleSegment {
    fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: camel_api::Exchange,
    ) -> Pin<Box<dyn Future<Output = camel_api::PipelineOutcome> + Send + 'a>> {
        Box::pin(async move {
            let acquired = {
                let mut limiter = self.limiter.lock().unwrap(); // allow-unwrap
                limiter.try_acquire()
            };
            if acquired {
                return self.body.run(exchange).await;
            }
            match self.config.strategy {
                ThrottleStrategy::Delay => {
                    loop {
                        let wait_time = {
                            let limiter = self.limiter.lock().unwrap(); // allow-unwrap
                            limiter.time_until_next_token()
                        };
                        if wait_time > Duration::ZERO {
                            tokio::time::sleep(wait_time).await;
                        }
                        let acquired = {
                            let mut limiter = self.limiter.lock().unwrap(); // allow-unwrap
                            limiter.try_acquire()
                        };
                        if acquired {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                    self.body.run(exchange).await
                }
                ThrottleStrategy::Reject => {
                    camel_api::PipelineOutcome::Failed(camel_api::CamelError::ProcessorError(
                        "Throttled: rate limit exceeded".to_string(),
                    ))
                }
                ThrottleStrategy::Drop => {
                    let mut ex = exchange;
                    ex.set_property(CAMEL_STOP, camel_api::Value::Bool(true));
                    camel_api::PipelineOutcome::Stopped(ex)
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{BoxProcessorExt, Message};
    use tower::ServiceExt;

    fn passthrough() -> BoxProcessor {
        BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }))
    }

    /// D-M8: zero `period` is rejected at construction with `Err`, not a
    /// panic. The audit reproduced the original panic; the fix returns
    /// `Err(CamelError::Config)` from `try_new` so the caller can refuse
    /// to start the route. This test was previously called
    /// `test_throttler_zero_period_rejected` and asserted the panic on
    /// `new`; after the fix it asserts the Err from `try_new` (the path
    /// operators / route constructors should use). The `new` path still
    /// panics via `expect(..)` — that's a separate, intentional contract
    /// for callers that have already validated the config.
    #[test]
    fn test_throttler_zero_period_returns_err() {
        let config = ThrottlerConfig::new(5, Duration::ZERO);
        let err = ThrottlerService::try_new(config, passthrough())
            .err()
            .expect("zero period must be Err from try_new, not Ok");
        let msg = err.to_string();
        assert!(
            msg.contains("period") && msg.contains("> 0"),
            "error must mention period > 0: {msg}"
        );
    }

    #[tokio::test]
    async fn test_throttler_allows_under_limit() {
        let config = ThrottlerConfig::new(5, Duration::from_secs(1));
        let mut svc = ThrottlerService::new(config, passthrough());

        for _ in 0..5 {
            let ex = Exchange::new(Message::new("test"));
            let result = svc.ready().await.unwrap().call(ex).await;
            assert!(result.is_ok());
        }
    }

    #[tokio::test]
    async fn test_throttler_delay_strategy_queues_message() {
        let config = ThrottlerConfig::new(1, Duration::from_millis(100));
        let mut svc = ThrottlerService::new(config, passthrough());

        let ex1 = Exchange::new(Message::new("first"));
        let result1 = svc.ready().await.unwrap().call(ex1).await;
        assert!(result1.is_ok());

        let start = Instant::now();
        let ex2 = Exchange::new(Message::new("second"));
        let result2 = svc.ready().await.unwrap().call(ex2).await;
        let elapsed = start.elapsed();
        assert!(result2.is_ok());
        assert!(elapsed >= Duration::from_millis(50));
    }

    #[tokio::test]
    async fn test_throttler_reject_strategy_returns_error() {
        let config =
            ThrottlerConfig::new(1, Duration::from_secs(10)).strategy(ThrottleStrategy::Reject);
        let mut svc = ThrottlerService::new(config, passthrough());

        let ex1 = Exchange::new(Message::new("first"));
        let _ = svc.ready().await.unwrap().call(ex1).await;

        let ex2 = Exchange::new(Message::new("second"));
        let result = svc.ready().await.unwrap().call(ex2).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Throttled"));
    }

    #[tokio::test]
    async fn test_throttler_drop_strategy_sets_camel_stop() {
        let config =
            ThrottlerConfig::new(1, Duration::from_secs(10)).strategy(ThrottleStrategy::Drop);
        let mut svc = ThrottlerService::new(config, passthrough());

        let ex1 = Exchange::new(Message::new("first"));
        let _ = svc.ready().await.unwrap().call(ex1).await;

        let ex2 = Exchange::new(Message::new("second"));
        let result = svc.ready().await.unwrap().call(ex2).await.unwrap();
        assert_eq!(result.property(CAMEL_STOP), Some(&Value::Bool(true)));
    }

    #[tokio::test]
    async fn test_throttler_token_replenishment() {
        let config = ThrottlerConfig::new(1, Duration::from_millis(50));
        let mut svc = ThrottlerService::new(config, passthrough());

        let ex1 = Exchange::new(Message::new("first"));
        let _ = svc.ready().await.unwrap().call(ex1).await;

        tokio::time::sleep(Duration::from_millis(100)).await;

        let ex2 = Exchange::new(Message::new("second"));
        let result = svc.ready().await.unwrap().call(ex2).await;
        assert!(result.is_ok());
    }

    // ── D-M8 Batch 1: zero max_requests / zero period is Err, not panic ──

    /// D-M8 (reproduced): `ThrottlerService::new` with `max_requests=0`
    /// used to panic via `time_until_next_token → 1.0/0.0 = inf →
    /// Duration::from_secs_f64(inf) panic`. The fix returns `Err`
    /// (CamelError::ConfigValidation(ThrottlerMaxRequestsZero)) instead.
    /// Caller can match the error and refuse to start the route.
    #[test]
    fn test_throttler_zero_max_requests_returns_err() {
        // We need a constructor that returns Result. The current API is
        // `new(config, next)` which asserts. Wrap it: this test asserts
        // that calling the (post-fix) `try_new` with max_requests=0
        // returns Err. If the constructor still asserts, the test will
        // panic (caught by `catch_unwind`) and we will see a failure
        // distinct from "Err returned" — but the test asserts is_err.
        let config = ThrottlerConfig::new(0, Duration::from_secs(1));
        // Try both names: post-fix API is `try_new`; pre-fix is `new`.
        // The post-fix `try_new` is what we want; the pre-fix `new`
        // panics, which is the bug.
        let result = std::panic::catch_unwind(|| ThrottlerService::try_new(config, passthrough()));
        match result {
            Ok(Ok(_)) => panic!("zero max_requests must be Err, not Ok"),
            Ok(Err(e)) => {
                assert!(
                    matches!(
                        e,
                        CamelError::ConfigValidation(
                            camel_api::ConfigValidationError::ThrottlerMaxRequestsZero,
                        )
                    ),
                    "expected ConfigValidation(ThrottlerMaxRequestsZero), got: {e}"
                );
            }
            Err(_) => panic!("zero max_requests must return Err, not panic"),
        }
    }

    // ── ThrottleSegment tests (ADR-0025 OutcomePipeline parity) ────────────

    #[tokio::test]
    async fn throttle_segment_reject_strategy_returns_failed() {
        use camel_api::{Exchange, Message, OutcomePipeline, PipelineOutcome};

        #[derive(Clone)]
        struct NoopSeg;
        impl OutcomePipeline for NoopSeg {
            fn clone_box(&self) -> Box<dyn OutcomePipeline> {
                Box::new(NoopSeg)
            }
            fn run<'a>(
                &'a mut self,
                ex: Exchange,
            ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
                Box::pin(async move { PipelineOutcome::Completed(ex) })
            }
        }

        // Single-token throttler. First call exhausts the token; second
        // call triggers the Reject strategy. D-M8 rejects max_requests=0,
        // so we use 1 and fire one request to consume the token.
        let config = ThrottlerConfig {
            max_requests: 1,
            period: Duration::from_secs(1),
            strategy: ThrottleStrategy::Reject,
        };
        let body = camel_api::OutcomeSegment::new(Box::new(NoopSeg));
        let mut seg = ThrottleSegment::new(config, body);
        let ex = Exchange::new(Message::new("test"));
        // First call exhausts the single token
        let _first = seg.run(ex).await;
        // Second call is rejected
        let ex2 = Exchange::new(Message::new("test2"));
        let outcome = seg.run(ex2).await;
        assert!(
            matches!(outcome, PipelineOutcome::Failed(_)),
            "Reject strategy must return Failed when tokens exhausted"
        );
    }

    #[tokio::test]
    async fn throttle_segment_drop_strategy_returns_stopped() {
        use camel_api::{Exchange, Message, OutcomePipeline, PipelineOutcome};

        #[derive(Clone)]
        struct NoopSeg;
        impl OutcomePipeline for NoopSeg {
            fn clone_box(&self) -> Box<dyn OutcomePipeline> {
                Box::new(NoopSeg)
            }
            fn run<'a>(
                &'a mut self,
                ex: Exchange,
            ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
                Box::pin(async move { PipelineOutcome::Completed(ex) })
            }
        }

        // Single-token throttler. First call exhausts the token; second
        // call triggers the Drop strategy. D-M8 rejects max_requests=0.
        let config = ThrottlerConfig {
            max_requests: 1,
            period: Duration::from_secs(1),
            strategy: ThrottleStrategy::Drop,
        };
        let body = camel_api::OutcomeSegment::new(Box::new(NoopSeg));
        let mut seg = ThrottleSegment::new(config, body);
        let ex = Exchange::new(Message::new("test"));
        // First call exhausts the single token
        let _first = seg.run(ex).await;
        // Second call is dropped
        let ex2 = Exchange::new(Message::new("test2"));
        let outcome = seg.run(ex2).await;
        match outcome {
            PipelineOutcome::Stopped(returned_ex) => {
                let stopped_flag = returned_ex.property(CAMEL_STOP).and_then(|v| v.as_bool());
                assert_eq!(
                    stopped_flag,
                    Some(true),
                    "Drop strategy must set CamelStop=true property"
                );
            }
            other => panic!("Drop must return Stopped, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn throttle_segment_delay_strategy_propagates_stopped_body() {
        use camel_api::{Body, Exchange, Message, OutcomePipeline, PipelineOutcome};

        #[derive(Clone)]
        struct StoppingSeg;
        impl OutcomePipeline for StoppingSeg {
            fn clone_box(&self) -> Box<dyn OutcomePipeline> {
                Box::new(StoppingSeg)
            }
            fn run<'a>(
                &'a mut self,
                mut ex: Exchange,
            ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
                Box::pin(async move {
                    ex.input.body = Body::Bytes(b"stopped-mut".to_vec().into());
                    PipelineOutcome::Stopped(ex)
                })
            }
        }

        let config = ThrottlerConfig {
            max_requests: 1, // 1 token available immediately
            period: Duration::from_secs(1),
            strategy: ThrottleStrategy::Delay,
        };
        let body = camel_api::OutcomeSegment::new(Box::new(StoppingSeg));
        let mut seg = ThrottleSegment::new(config, body);
        let ex = Exchange::new(Message::new("test"));
        let outcome = seg.run(ex).await;
        match outcome {
            PipelineOutcome::Stopped(returned_ex) => {
                if let Body::Bytes(b) = &returned_ex.input.body {
                    assert_eq!(
                        b.as_ref(),
                        b"stopped-mut",
                        "BUG: throttle body Stop must preserve mutations"
                    );
                } else {
                    panic!("expected Body::Bytes");
                }
            }
            other => panic!("expected Stopped propagation, got {:?}", other),
        }
    }
}
