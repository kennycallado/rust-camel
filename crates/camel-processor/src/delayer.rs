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
            return n as u64;
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
        ex.input.set_header("CamelDelayMs", Value::Number(50.into()));

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
        ex.input.set_header("CamelDelayMs", Value::String("not a number".into()));

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
        ex.input.set_header("CamelDelayMs", Value::Number((-100).into()));

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
}
