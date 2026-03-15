use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tower::Service;

use camel_api::{BoxProcessor, CamelError, Exchange, ThrottleStrategy, ThrottlerConfig, Value};

const CAMEL_STOP: &str = "CamelStop";

struct RateLimiter {
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
                let mut limiter = limiter.lock().unwrap();
                limiter.try_acquire()
            };

            if acquired {
                next.call(exchange).await
            } else {
                match config.strategy {
                    ThrottleStrategy::Delay => {
                        let wait_time = {
                            let limiter = limiter.lock().unwrap();
                            limiter.time_until_next_token()
                        };
                        if wait_time > Duration::ZERO {
                            tokio::time::sleep(wait_time).await;
                        }
                        {
                            let mut limiter = limiter.lock().unwrap();
                            limiter.try_acquire();
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

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{BoxProcessorExt, Message};
    use tower::ServiceExt;

    fn passthrough() -> BoxProcessor {
        BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }))
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
}
