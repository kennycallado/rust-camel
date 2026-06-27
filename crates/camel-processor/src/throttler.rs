use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tower::Service;

use camel_api::CAMEL_STOP;
use camel_api::{BoxProcessor, CamelError, Exchange, ThrottleStrategy, ThrottlerConfig, Value};

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
    pub fn new(config: ThrottlerConfig, next: BoxProcessor) -> Self {
        assert!(
            config.period > Duration::ZERO,
            "throttler period must be > 0"
        );
        let limiter = RateLimiter::new(config.max_requests, config.period);
        Self {
            config,
            limiter: std::sync::Arc::new(Mutex::new(limiter)),
            next,
        }
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
    pub fn new(config: ThrottlerConfig, body: camel_api::OutcomeSegment) -> Self {
        assert!(
            config.period > Duration::ZERO,
            "throttler period must be > 0"
        );
        Self {
            limiter: std::sync::Arc::new(std::sync::Mutex::new(RateLimiter::new(
                config.max_requests,
                config.period,
            ))),
            config,
            body,
        }
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

    #[test]
    fn test_throttler_zero_period_rejected() {
        let config = ThrottlerConfig::new(5, Duration::ZERO);
        let result = std::panic::catch_unwind(|| {
            ThrottlerService::new(config, passthrough());
        });
        assert!(result.is_err(), "zero period should panic");
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

        let config = ThrottlerConfig {
            max_requests: 0, // 0 tokens immediately exhausted
            period: Duration::from_secs(1),
            strategy: ThrottleStrategy::Reject,
        };
        let body = camel_api::OutcomeSegment::new(Box::new(NoopSeg));
        let mut seg = ThrottleSegment::new(config, body);
        let ex = Exchange::new(Message::new("test"));
        let outcome = seg.run(ex).await;
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

        let config = ThrottlerConfig {
            max_requests: 0,
            period: Duration::from_secs(1),
            strategy: ThrottleStrategy::Drop,
        };
        let body = camel_api::OutcomeSegment::new(Box::new(NoopSeg));
        let mut seg = ThrottleSegment::new(config, body);
        let ex = Exchange::new(Message::new("test"));
        let outcome = seg.run(ex).await;
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
