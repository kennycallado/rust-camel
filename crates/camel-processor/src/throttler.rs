use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use camel_api::CAMEL_STOP;
use camel_api::{CamelError, ConfigValidationError, ThrottleStrategy, ThrottlerConfig};

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

/// Outcome-aware throttle segment (ADR-0025).
///
/// Wraps a `ThrottlerConfig` + shared `RateLimiter` + child sub-pipeline body.
/// The segment propagates `PipelineOutcome::Stopped` / `Failed` from the body.
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

    /// Fallible constructor. Validates `period` and `max_requests`.
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
                // Delay and any future variant wait for an available token.
                _ => {
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
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_processor_segment::processor_segment;
    use camel_api::{
        Body, BoxProcessor, BoxProcessorExt, Exchange, Message, OutcomePipeline, PipelineOutcome,
    };

    fn passthrough() -> BoxProcessor {
        BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }))
    }

    fn segment(config: ThrottlerConfig) -> ThrottleSegment {
        ThrottleSegment::new(config, processor_segment(passthrough()))
    }

    /// D-M8: zero `period` is rejected at construction with `Err`, not a
    /// panic. The audit reproduced the original panic; the fix returns
    /// `Err(CamelError::Config)` from `try_new` so the caller can refuse
    /// to start the route.
    #[test]
    fn throttle_segment_zero_period_returns_err() {
        let config = ThrottlerConfig::new(5, Duration::ZERO);
        let err = ThrottleSegment::try_new(config, processor_segment(passthrough()))
            .err()
            .expect("zero period must be Err from try_new, not Ok");
        let msg = err.to_string();
        assert!(
            msg.contains("period") && msg.contains("> 0"),
            "error must mention period > 0: {msg}"
        );
    }

    #[tokio::test]
    async fn throttle_segment_allows_under_limit() {
        let config = ThrottlerConfig::new(5, Duration::from_secs(1));
        let mut seg = segment(config);

        for _ in 0..5 {
            let ex = Exchange::new(Message::new("test"));
            let outcome = seg.run(ex).await;
            assert!(matches!(outcome, PipelineOutcome::Completed(_)));
        }
    }

    #[tokio::test]
    async fn throttle_segment_delay_strategy_queues_message() {
        let config = ThrottlerConfig::new(1, Duration::from_millis(100));
        let mut seg = segment(config);

        let first = seg.run(Exchange::new(Message::new("first"))).await;
        assert!(matches!(first, PipelineOutcome::Completed(_)));

        let start = Instant::now();
        let second = seg.run(Exchange::new(Message::new("second"))).await;
        assert!(matches!(second, PipelineOutcome::Completed(_)));
        assert!(start.elapsed() >= Duration::from_millis(50));
    }

    // ── D-M8 Batch 1: zero max_requests / zero period is Err, not panic ──

    /// D-M8 (reproduced): construction with `max_requests=0` used to panic
    /// via `time_until_next_token → 1.0/0.0 = inf →
    /// Duration::from_secs_f64(inf) panic`. The fix returns `Err`
    /// (CamelError::ConfigValidation(ThrottlerMaxRequestsZero)) instead.
    /// Caller can match the error and refuse to start the route.
    #[test]
    fn throttle_segment_zero_max_requests_returns_err() {
        let config = ThrottlerConfig::new(0, Duration::from_secs(1));
        let err = ThrottleSegment::try_new(config, processor_segment(passthrough()))
            .err()
            .expect("zero max_requests must be Err, not Ok");
        assert!(
            matches!(
                err,
                CamelError::ConfigValidation(
                    camel_api::ConfigValidationError::ThrottlerMaxRequestsZero,
                )
            ),
            "expected ConfigValidation(ThrottlerMaxRequestsZero), got: {err}"
        );
    }

    // ── ThrottleSegment tests (ADR-0025 OutcomePipeline parity) ────────────

    #[tokio::test]
    async fn throttle_segment_reject_strategy_returns_failed() {
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
        let err = match outcome {
            PipelineOutcome::Failed(err) => err,
            other => panic!("Reject strategy must return Failed when tokens exhausted: {other:?}"),
        };
        assert!(
            matches!(err, CamelError::ProcessorError(_)),
            "throttle rejection must be ProcessorError, got: {err:?}"
        );
        assert!(
            err.to_string().contains("Throttled"),
            "throttle rejection error must mention Throttled: {err}"
        );
    }

    #[tokio::test]
    async fn throttle_segment_drop_strategy_returns_stopped() {
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
