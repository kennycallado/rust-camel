use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tower::Service;

use camel_api::{CamelError, DelayConfig, Exchange, Value};

#[derive(Clone)]
pub struct DelayerService {
    config: DelayConfig,
}

impl DelayerService {
    pub fn new(config: DelayConfig) -> Self {
        Self { config }
    }

    fn effective_delay_ms(&self, exchange: &Exchange) -> u64 {
        if let Some(ref header) = self.config.dynamic_header
            && let Some(Value::Number(n)) = exchange.input.header(header)
            && let Some(n) = n.as_f64()
            && n >= 0.0
        {
            // H12 Batch 1: clamp the raw f64 to [0, max_delay_ms] BEFORE the
            // cast. Without this, `1e20 as u64` saturates to u64::MAX, then
            // `Duration::from_millis(u64::MAX)` + `Instant::now()` overflows
            // and panics. NaN / negative inputs are already filtered above.
            let cap = self.config.max_delay_ms as f64;
            let clamped = n.min(cap);
            return clamped as u64;
        }
        self.config.delay_ms
    }
}

impl Service<Exchange> for DelayerService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let delay_ms = self.effective_delay_ms(&exchange);
        Box::pin(async move {
            if delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            Ok(exchange)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use camel_api::DelayConfig;
    use camel_api::{Exchange, Message, Value};
    use tower::{Service, ServiceExt};

    use super::DelayerService;

    #[tokio::test]
    async fn test_delayer_fixed_delay() {
        let config = DelayConfig::new(100);
        let mut svc = DelayerService::new(config);

        let start = Instant::now();
        let ex = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(ex).await;
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        assert!(elapsed >= Duration::from_millis(90));
    }

    #[tokio::test]
    async fn test_delayer_dynamic_from_header() {
        let config = DelayConfig::new(5000).with_dynamic_header("CamelDelayMs");
        let mut svc = DelayerService::new(config);

        let mut ex = Exchange::new(Message::new("test"));
        ex.input
            .set_header("CamelDelayMs", Value::Number(50.into()));

        let start = Instant::now();
        let result = svc.ready().await.unwrap().call(ex).await;
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        assert!(elapsed >= Duration::from_millis(40));
        assert!(elapsed < Duration::from_millis(200));
    }

    #[tokio::test]
    async fn test_delayer_zero_delay() {
        let config = DelayConfig::new(0);
        let mut svc = DelayerService::new(config);

        let start = Instant::now();
        let ex = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(ex).await;
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        assert!(elapsed < Duration::from_millis(50));
    }

    #[tokio::test]
    async fn test_delayer_zero_from_header() {
        let config = DelayConfig::new(5000).with_dynamic_header("CamelDelayMs");
        let mut svc = DelayerService::new(config);

        let mut ex = Exchange::new(Message::new("test"));
        ex.input.set_header("CamelDelayMs", Value::Number(0.into()));

        let start = Instant::now();
        let result = svc.ready().await.unwrap().call(ex).await;
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        assert!(elapsed < Duration::from_millis(50));
    }

    #[tokio::test]
    async fn test_delayer_fallback_on_invalid_header() {
        let config = DelayConfig::new(80).with_dynamic_header("CamelDelayMs");
        let mut svc = DelayerService::new(config);

        let mut ex = Exchange::new(Message::new("test"));
        ex.input
            .set_header("CamelDelayMs", Value::String("not a number".into()));

        let start = Instant::now();
        let result = svc.ready().await.unwrap().call(ex).await;
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        assert!(elapsed >= Duration::from_millis(70));
    }

    #[tokio::test]
    async fn test_delayer_negative_header() {
        let config = DelayConfig::new(80).with_dynamic_header("CamelDelayMs");
        let mut svc = DelayerService::new(config);

        let mut ex = Exchange::new(Message::new("test"));
        ex.input
            .set_header("CamelDelayMs", Value::Number((-100).into()));

        let start = Instant::now();
        let result = svc.ready().await.unwrap().call(ex).await;
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        assert!(elapsed >= Duration::from_millis(70));
    }

    #[tokio::test]
    async fn test_delayer_missing_header() {
        let config = DelayConfig::new(80).with_dynamic_header("CamelDelayMs");
        let mut svc = DelayerService::new(config);

        let start = Instant::now();
        let ex = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(ex).await;
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        assert!(elapsed >= Duration::from_millis(70));
    }

    // ── H12 Batch 1: clamp header-derived delay to max_delay_ms ──────

    /// H12: a header value of `1e20` (which saturates to `u64::MAX` when
    /// cast) MUST be clamped to `max_delay_ms` before the sleep. The test
    /// uses a tiny cap so the test runs in milliseconds, not hours.
    /// Without the clamp, the call would panic with `Instant + Duration`
    /// overflow (or sleep for ~584M years).
    #[tokio::test]
    async fn test_huge_header_value_clamps_to_max_delay_ms() {
        let config = DelayConfig::new(80)
            .with_dynamic_header("CamelDelayMs")
            .with_max_delay_ms(50); // tiny cap so the test is fast
        let mut svc = DelayerService::new(config);

        let mut ex = Exchange::new(Message::new("test"));
        // 1e20 saturates to u64::MAX without the clamp.
        ex.input.set_header(
            "CamelDelayMs",
            Value::Number(serde_json::Number::from_f64(1e20).unwrap()),
        );

        let start = Instant::now();
        let result = svc.ready().await.unwrap().call(ex).await;
        let elapsed = start.elapsed();

        assert!(result.is_ok(), "huge header value must clamp, not panic");
        assert!(
            elapsed < Duration::from_millis(500),
            "should clamp to ~50ms, got {elapsed:?}"
        );
        assert!(
            elapsed >= Duration::from_millis(40),
            "should sleep at least ~50ms (the cap), got {elapsed:?}"
        );
    }

    /// When the header value is under the cap, the clamp is a no-op.
    #[tokio::test]
    async fn test_small_header_value_unaffected_by_clamp() {
        let config = DelayConfig::new(80)
            .with_dynamic_header("CamelDelayMs")
            .with_max_delay_ms(3_600_000); // 1 hour, default
        let mut svc = DelayerService::new(config);

        let mut ex = Exchange::new(Message::new("test"));
        ex.input
            .set_header("CamelDelayMs", Value::Number(20.into()));

        let start = Instant::now();
        let result = svc.ready().await.unwrap().call(ex).await;
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        assert!(
            elapsed < Duration::from_millis(200),
            "small value should be unaffected, got {elapsed:?}"
        );
    }
}
