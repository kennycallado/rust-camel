use std::time::Duration;

use async_trait::async_trait;
use tokio::time;
use tracing::debug;

use camel_component_api::UriConfig;
use camel_component_api::{BoxProcessor, CamelError, Exchange, Message};
use camel_component_api::{Component, Consumer, ConsumerContext, Endpoint, ProducerContext};

// ---------------------------------------------------------------------------
// TimerConfig
// ---------------------------------------------------------------------------

/// Configuration parsed from a timer URI.
///
/// Format: `timer:name?period=1000&delay=0&repeatCount=0`
#[derive(Debug, Clone, UriConfig)]
#[uri_scheme = "timer"]
#[uri_config(crate = "camel_component_api")]
pub struct TimerConfig {
    /// Timer name (the path portion of the URI).
    pub name: String,

    /// Interval between ticks (milliseconds). Default: 1000.
    #[allow(dead_code)] // Used by macro-generated Duration conversion
    #[uri_param(name = "period", default = "1000")]
    period_ms: u64,

    /// Converted Duration for period.
    pub period: Duration,

    /// Initial delay before the first tick (milliseconds). Default: 0.
    #[allow(dead_code)] // Used by macro-generated Duration conversion
    #[uri_param(name = "delay", default = "0")]
    delay_ms: u64,

    /// Converted Duration for delay.
    pub delay: Duration,

    /// Maximum number of ticks. `None` means infinite.
    #[uri_param(name = "repeatCount")]
    pub repeat_count: Option<u32>,
}

// ---------------------------------------------------------------------------
// TimerComponent
// ---------------------------------------------------------------------------

/// The Timer component produces exchanges on a periodic interval.
pub struct TimerComponent;

impl TimerComponent {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TimerComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for TimerComponent {
    fn scheme(&self) -> &str {
        "timer"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let config = TimerConfig::from_uri(uri)?;
        Ok(Box::new(TimerEndpoint {
            uri: uri.to_string(),
            config,
        }))
    }
}

// ---------------------------------------------------------------------------
// TimerEndpoint
// ---------------------------------------------------------------------------

struct TimerEndpoint {
    uri: String,
    config: TimerConfig,
}

impl Endpoint for TimerEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(TimerConsumer {
            config: self.config.clone(),
        }))
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        Err(CamelError::EndpointCreationFailed(
            "timer endpoint does not support producers".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// TimerConsumer
// ---------------------------------------------------------------------------

struct TimerConsumer {
    config: TimerConfig,
}

#[async_trait]
impl Consumer for TimerConsumer {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        let config = self.config.clone();

        // Initial delay (cancellable so shutdown isn't blocked by long delays)
        if !config.delay.is_zero() {
            tokio::select! {
                _ = time::sleep(config.delay) => {}
                _ = context.cancelled() => {
                    debug!(timer = config.name, "Timer cancelled during initial delay");
                    return Ok(());
                }
            }
        }

        let mut interval = time::interval(config.period);
        let mut count: u32 = 0;

        loop {
            tokio::select! {
                _ = context.cancelled() => {
                    debug!(timer = config.name, "Timer received cancellation, stopping");
                    break;
                }
                _ = interval.tick() => {
                    count += 1;

                    debug!(timer = config.name, count, "Timer tick");

                    let mut exchange = Exchange::new(Message::new(format!(
                        "timer://{} tick #{}",
                        config.name, count
                    )));
                    exchange.input.set_header(
                        "CamelTimerName",
                        serde_json::Value::String(config.name.clone()),
                    );
                    exchange
                        .input
                        .set_header("CamelTimerCounter", serde_json::Value::Number(count.into()));

                    if context.send(exchange).await.is_err() {
                        // Channel closed, route was stopped
                        break;
                    }

                    if let Some(max) = config.repeat_count
                        && count >= max
                    {
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::NoOpComponentContext;

    #[test]
    fn test_timer_config_defaults() {
        let config = TimerConfig::from_uri("timer:tick").unwrap();
        assert_eq!(config.name, "tick");
        assert_eq!(config.period, Duration::from_millis(1000));
        assert_eq!(config.delay, Duration::from_millis(0));
        assert_eq!(config.repeat_count, None);
    }

    #[test]
    fn test_timer_config_with_params() {
        let config =
            TimerConfig::from_uri("timer:myTimer?period=500&delay=100&repeatCount=5").unwrap();
        assert_eq!(config.name, "myTimer");
        assert_eq!(config.period, Duration::from_millis(500));
        assert_eq!(config.delay, Duration::from_millis(100));
        assert_eq!(config.repeat_count, Some(5));
    }

    #[test]
    fn test_timer_config_wrong_scheme() {
        let result = TimerConfig::from_uri("log:info");
        assert!(result.is_err());
    }

    #[test]
    fn test_timer_component_scheme() {
        let component = TimerComponent::new();
        assert_eq!(component.scheme(), "timer");
    }

    #[test]
    fn test_timer_component_creates_endpoint() {
        let component = TimerComponent::new();
        let endpoint = component.create_endpoint("timer:tick?period=1000", &NoOpComponentContext);
        assert!(endpoint.is_ok());
    }

    #[test]
    fn test_timer_endpoint_no_producer() {
        let ctx = ProducerContext::new();
        let component = TimerComponent::new();
        let endpoint = component.create_endpoint("timer:tick", &NoOpComponentContext).unwrap();
        let producer = endpoint.create_producer(&ctx);
        assert!(producer.is_err());
    }

    #[tokio::test]
    async fn test_timer_consumer_fires() {
        let component = TimerComponent::new();
        let endpoint = component
            .create_endpoint("timer:test?period=50&repeatCount=3", &NoOpComponentContext)
            .unwrap();
        let mut consumer = endpoint.create_consumer().unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let ctx = ConsumerContext::new(tx, tokio_util::sync::CancellationToken::new());

        // Run consumer in background
        tokio::spawn(async move {
            consumer.start(ctx).await.unwrap();
        });

        // Collect exchanges
        let mut received = Vec::new();
        while let Some(envelope) = rx.recv().await {
            received.push(envelope.exchange);
            if received.len() == 3 {
                break;
            }
        }

        assert_eq!(received.len(), 3);

        // Verify headers on the first exchange
        let first = &received[0];
        assert_eq!(
            first.input.header("CamelTimerName"),
            Some(&serde_json::Value::String("test".into()))
        );
        assert_eq!(
            first.input.header("CamelTimerCounter"),
            Some(&serde_json::Value::Number(1.into()))
        );
    }

    #[tokio::test]
    async fn test_timer_consumer_respects_cancellation() {
        use tokio_util::sync::CancellationToken;

        let token = CancellationToken::new();
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let ctx = ConsumerContext::new(tx, token.clone());

        let mut consumer = TimerConsumer {
            config: TimerConfig::from_uri("timer:cancel-test?period=50").unwrap(),
        };

        let handle = tokio::spawn(async move {
            consumer.start(ctx).await.unwrap();
        });

        // Let it fire a few times
        tokio::time::sleep(Duration::from_millis(180)).await;
        token.cancel();

        let result = tokio::time::timeout(Duration::from_secs(1), handle).await;
        assert!(
            result.is_ok(),
            "Consumer should have stopped after cancellation"
        );

        let mut count = 0;
        while rx.try_recv().is_ok() {
            count += 1;
        }
        assert!(
            count >= 2,
            "Expected at least 2 exchanges before cancellation, got {count}"
        );
    }
}
