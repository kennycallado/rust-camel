//! Timer component for rust-camel — fires `Exchange` events on configurable period, delay, and repeat-count schedules.
//!
//! Main types: `TimerComponent`, `TimerConsumer`, `TimerConfig`, `TimerEndpoint`.
//! URI format: `timer:name?period=1000&delay=0&repeatCount=0`.
//!
//! # Features
//!
//! - **fixedRate**: When enabled, uses skip-missed-tick semantics instead of burst.
//! - **includeMetadata**: Controls whether timer metadata headers are included in exchanges.
//! - Double-start protection via `AtomicBool` guard.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tokio::time;
use tracing::debug;

use camel_api::component_metadata::{
    ComponentCapabilities, ComponentMetadata, OptionKind, UriOption,
};
use camel_component_api::UriConfig;
use camel_component_api::{BoxProcessor, CamelError, Exchange, Message};
use camel_component_api::{Component, Consumer, ConsumerContext, Endpoint, ProducerContext};

// ---------------------------------------------------------------------------
// TimerConfig
// ---------------------------------------------------------------------------

/// Configuration parsed from a timer URI.
///
/// Format: `timer:name?period=1000&delay=0&repeatCount=0&fixedRate=false&includeMetadata=true`
#[derive(Debug, Clone, UriConfig)]
#[uri_scheme = "timer"]
#[uri_config(skip_impl, crate = "camel_component_api")]
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

    /// When true, use fixed-rate semantics (skip missed ticks).
    /// When false (default), use burst semantics (fire all missed ticks immediately).
    #[uri_param(name = "fixedRate", default = "false")]
    pub fixed_rate: bool,

    /// When true (default), include metadata headers (CamelTimerFiredTime,
    /// CamelMessageTimestamp, CamelTimerName) in each exchange.
    /// When false, send a minimal exchange without metadata headers.
    #[uri_param(name = "includeMetadata", default = "true")]
    pub include_metadata: bool,
}

// Inherent validate — callable as TimerConfig::validate(&self)
impl TimerConfig {
    /// Validate the configuration without consuming self.
    pub fn validate(&self) -> Result<(), CamelError> {
        if self.name.trim().is_empty() {
            return Err(CamelError::InvalidUri(
                "timer name must not be empty".to_string(),
            ));
        }
        if self.period.is_zero() {
            return Err(CamelError::InvalidUri(
                "timer period must be greater than 0".to_string(),
            ));
        }
        Ok(())
    }
}

impl UriConfig for TimerConfig {
    fn scheme() -> &'static str {
        "timer"
    }

    fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = camel_component_api::parse_uri(uri)?;
        Self::from_components(parts)
    }

    fn from_components(parts: camel_component_api::UriComponents) -> Result<Self, CamelError> {
        let config = Self::parse_uri_components(parts)?;
        TimerConfig::validate(&config)?;
        Ok(config)
    }

    fn validate(self) -> Result<Self, CamelError> {
        // Delegate to the inherent validate(&self)
        TimerConfig::validate(&self)?;
        Ok(self)
    }
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

    fn metadata(&self) -> ComponentMetadata {
        ComponentMetadata {
            scheme: "timer".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            description: "Generates timer events at fixed intervals".to_string(),
            uri_syntax: "timer:name?period=1000&delay=0&repeatCount=0".to_string(),
            capabilities: ComponentCapabilities {
                supports_consumer: true,
                ..Default::default()
            },
            uri_options: vec![
                UriOption::new(
                    "period",
                    "Interval between ticks in milliseconds",
                    OptionKind::Int,
                )
                .with_default("1000"),
                UriOption::new(
                    "delay",
                    "Initial delay before first tick in milliseconds",
                    OptionKind::Int,
                )
                .with_default("0"),
                UriOption::new(
                    "repeatCount",
                    "Maximum number of ticks; omit for infinite. 0 = never fires.",
                    OptionKind::Int,
                ),
                UriOption::new(
                    "fixedRate",
                    "true=skip missed ticks, false=fire all missed",
                    OptionKind::Bool,
                )
                .with_default("false"),
                UriOption::new(
                    "includeMetadata",
                    "Include CamelTimer* headers in exchanges",
                    OptionKind::Bool,
                )
                .with_default("true"),
            ],
            ..ComponentMetadata::minimal("timer")
        }
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

pub struct TimerEndpoint {
    uri: String,
    config: TimerConfig,
}

impl Endpoint for TimerEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(TimerConsumer {
            config: self.config.clone(),
            started: AtomicBool::new(false),
        }))
    }

    fn create_producer(
        &self,
        _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Err(CamelError::EndpointCreationFailed(
            "timer endpoint does not support producers".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// TimerConsumer
// ---------------------------------------------------------------------------

pub struct TimerConsumer {
    config: TimerConfig,
    /// Guard against double-start (TIMER-003).
    started: AtomicBool,
}

#[async_trait]
impl Consumer for TimerConsumer {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        // TIMER-003: Guard against double-start
        self.started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| {
                CamelError::EndpointCreationFailed("timer consumer already started".to_string())
            })?;

        TimerConfig::validate(&self.config)?;
        let config = self.config.clone();
        let cancel_token = context.cancel_token();

        // Initial delay (cancellable so shutdown isn't blocked by long delays)
        if !config.delay.is_zero() {
            tokio::select! {
                _ = time::sleep(config.delay) => {}
                _ = cancel_token.cancelled() => {
                    debug!(timer = config.name, "Timer cancelled during initial delay");
                    self.started.store(false, Ordering::SeqCst);
                    return Ok(());
                }
            }
        }

        // If repeat_count is explicitly 0, fire zero times — stop immediately.
        if config.repeat_count == Some(0) {
            debug!(timer = config.name, "repeat_count=0, timer will not fire");
            self.started.store(false, Ordering::SeqCst);
            return Ok(());
        }

        let mut interval = time::interval(config.period);

        // TIMER-002: fixedRate controls missed-tick behavior
        if config.fixed_rate {
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        } else {
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);
        }

        let mut count: u32 = 0;

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
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

                    // TIMER-005 & TIMER-006: include metadata headers when enabled
                    if config.include_metadata {
                        exchange.input.set_header(
                            "CamelTimerName",
                            serde_json::Value::String(config.name.clone()),
                        );
                        exchange
                            .input
                            .set_header("CamelTimerCounter", serde_json::Value::Number(count.into()));

                        // TIMER-005: CamelTimerFiredTime (ISO-8601) and CamelMessageTimestamp (epoch millis)
                        let now = Utc::now();
                        exchange.input.set_header(
                            "CamelTimerFiredTime",
                            serde_json::Value::String(now.to_rfc3339()),
                        );
                        exchange.input.set_header(
                            "CamelMessageTimestamp",
                            serde_json::Value::Number(
                                now.timestamp_millis().into(),
                            ),
                        );
                    }

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

        // Reset started flag so consumer can be restarted after stop
        self.started.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        self.started.store(false, Ordering::SeqCst);
        debug!(timer = self.config.name, "timer consumer stopped");
        Ok(())
    }
}

impl TimerConsumer {
    /// Test helper: pre-set the started flag to simulate an already-running consumer.
    #[cfg(test)]
    pub(crate) fn mark_started_for_test(&self) {
        self.started.store(true, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use camel_component_api::test_support::PanicRuntimeObservability;
    fn rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(PanicRuntimeObservability)
    }

    use super::*;
    use camel_component_api::NoOpComponentContext;

    #[test]
    fn test_zero_period_rejected() {
        let result = TimerConfig::from_uri("timer:tick?period=0");
        assert!(result.is_err(), "period=0 should be rejected");
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("period"), "error should mention 'period'");
    }

    #[test]
    fn test_timer_empty_name_rejected() {
        let result = TimerConfig::from_uri("timer:");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("must not be empty"), "unexpected error: {err}");
    }

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
        let endpoint = component
            .create_endpoint("timer:tick", &NoOpComponentContext)
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx);
        assert!(producer.is_err());
    }

    #[test]
    fn test_rejects_empty_timer_name() {
        let mut cfg = TimerConfig::from_uri("timer:tick").unwrap();
        cfg.name = "".into();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_rejects_zero_period() {
        let mut cfg = TimerConfig::from_uri("timer:tick").unwrap();
        cfg.period = Duration::ZERO;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_valid_config_passes() {
        let mut cfg = TimerConfig::from_uri("timer:tick").unwrap();
        cfg.name = "myTimer".into();
        cfg.period = Duration::from_millis(1000);
        assert!(cfg.validate().is_ok());
    }

    #[tokio::test]
    async fn test_repeat_count_zero_fires_never() {
        let component = TimerComponent::new();
        let endpoint = component
            .create_endpoint(
                "timer:zero-test?period=50&repeatCount=0",
                &NoOpComponentContext,
            )
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let ctx = ConsumerContext::new(
            tx,
            tokio_util::sync::CancellationToken::new(),
            "timer-test-route".to_string(),
        );

        // Start the consumer (spawns internally, returns immediately)
        consumer.start(ctx).await.unwrap();

        // Wait longer than the period — no messages should arrive
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Drain any pending messages
        let mut count = 0;
        while rx.try_recv().is_ok() {
            count += 1;
        }
        assert_eq!(
            count, 0,
            "repeat_count=0 should produce zero fires, got {count}"
        );

        // Clean up
        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_repeat_count_omitted_fires_indefinitely() {
        use tokio_util::sync::CancellationToken;

        let token = CancellationToken::new();
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        let ctx = ConsumerContext::new(tx, token.clone(), "timer-infinite-route".to_string());

        // No repeatCount in the URI — must fire indefinitely until cancelled.
        let mut consumer = TimerConsumer {
            config: TimerConfig::from_uri("timer:infinite-test?period=20").unwrap(),
            started: AtomicBool::new(false),
        };

        let handle = tokio::spawn(async move {
            consumer.start(ctx).await.unwrap();
        });

        // Window long enough that any finite repeat_count <= 5 would have
        // stopped well before the cap we assert (6 fires at 20ms = ~120ms).
        tokio::time::sleep(Duration::from_millis(200)).await;
        token.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;

        let mut count = 0;
        while rx.try_recv().is_ok() {
            count += 1;
        }
        assert!(
            count >= 6,
            "Omitted repeatCount should fire indefinitely; expected >= 6 fires in window, got {count}"
        );
    }

    #[tokio::test]
    async fn test_timer_consumer_fires() {
        let component = TimerComponent::new();
        let endpoint = component
            .create_endpoint("timer:test?period=50&repeatCount=3", &NoOpComponentContext)
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let ctx = ConsumerContext::new(
            tx,
            tokio_util::sync::CancellationToken::new(),
            "timer-test-route".to_string(),
        );

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
        let ctx = ConsumerContext::new(tx, token.clone(), "timer-test-route".to_string());

        let mut consumer = TimerConsumer {
            config: TimerConfig::from_uri("timer:cancel-test?period=50").unwrap(),
            started: AtomicBool::new(false),
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

    #[tokio::test]
    async fn test_timer_consumer_stop_shuts_down() {
        let component = TimerComponent::new();
        let endpoint = component
            .create_endpoint("timer:stop-test?period=50", &NoOpComponentContext)
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let token = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone(), "timer-test-route".to_string());

        // Run consumer in background (start() blocks until cancelled)
        tokio::spawn(async move {
            consumer.start(ctx).await.unwrap();
        });

        // Let it fire a few times
        tokio::time::sleep(Duration::from_millis(180)).await;

        // Drain any pending exchanges
        let mut count = 0;
        while rx.try_recv().is_ok() {
            count += 1;
        }
        assert!(count >= 2, "Expected at least 2 exchanges, got {count}");

        // Cancel the token to stop the consumer
        token.cancel();
    }

    // TIMER-002: fixedRate config round-trip
    #[test]
    fn test_fixed_rate_default_is_false() {
        let config = TimerConfig::from_uri("timer:tick").unwrap();
        assert!(!config.fixed_rate, "fixedRate should default to false");
    }

    #[test]
    fn test_fixed_rate_parsed_from_uri() {
        let config = TimerConfig::from_uri("timer:tick?fixedRate=true").unwrap();
        assert!(
            config.fixed_rate,
            "fixedRate should be true when set in URI"
        );
    }

    // TIMER-003: double-start guard
    #[tokio::test]
    async fn test_double_start_returns_error() {
        let component = TimerComponent::new();
        let endpoint = component
            .create_endpoint(
                "timer:double?period=50&repeatCount=2",
                &NoOpComponentContext,
            )
            .unwrap(); // allow-unwrap: test setup

        let mut consumer = TimerConsumer {
            config: TimerConfig {
                name: "double-test".to_string(),
                period: Duration::from_millis(100),
                period_ms: 100,
                delay: Duration::ZERO,
                delay_ms: 0,
                repeat_count: None,
                fixed_rate: false,
                include_metadata: true,
            },
            started: AtomicBool::new(false),
        };

        // Simulate the consumer already being started by setting the flag.
        consumer.mark_started_for_test();

        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, cancel_token.clone(), "timer-test-route".to_string());

        // Second start on an already-started consumer must return an error.
        let result = consumer.start(ctx).await;
        assert!(result.is_err(), "expected double-start to return Err");
        let err_str = format!("{:?}", result.unwrap_err());
        assert!(
            err_str.contains("already started"),
            "unexpected error: {err_str}"
        );

        drop(endpoint); // suppress unused-variable warning
    }

    // TIMER-005: CamelTimerFiredTime and CamelMessageTimestamp headers
    #[tokio::test]
    async fn test_timer_fired_time_and_message_timestamp_headers() {
        let component = TimerComponent::new();
        let endpoint = component
            .create_endpoint(
                "timer:headers?period=50&repeatCount=1",
                &NoOpComponentContext,
            )
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let ctx = ConsumerContext::new(
            tx,
            tokio_util::sync::CancellationToken::new(),
            "timer-test-route".to_string(),
        );

        tokio::spawn(async move {
            consumer.start(ctx).await.unwrap();
        });

        let envelope = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("should receive exchange")
            .expect("envelope should exist");

        let exchange = envelope.exchange;

        // CamelTimerFiredTime should be an ISO-8601 string
        let fired_time = exchange
            .input
            .header("CamelTimerFiredTime")
            .expect("CamelTimerFiredTime header should be present");
        assert!(
            fired_time.is_string(),
            "CamelTimerFiredTime should be a string"
        );
        let fired_str = fired_time.as_str().unwrap();
        // Should parse as ISO-8601 / RFC 3339
        assert!(
            chrono::DateTime::parse_from_rfc3339(fired_str).is_ok(),
            "CamelTimerFiredTime should be valid RFC 3339: {fired_str}"
        );

        // CamelMessageTimestamp should be a number (epoch millis)
        let msg_ts = exchange
            .input
            .header("CamelMessageTimestamp")
            .expect("CamelMessageTimestamp header should be present");
        assert!(
            msg_ts.is_number(),
            "CamelMessageTimestamp should be a number"
        );
        let ts_millis = msg_ts.as_i64().expect("should be i64");
        assert!(ts_millis > 0, "timestamp should be positive");
    }

    #[test]
    fn test_timer_fired_time_header_format() {
        // Verify the format independently
        let now = chrono::Utc::now();
        let rfc = now.to_rfc3339();
        assert!(chrono::DateTime::parse_from_rfc3339(&rfc).is_ok());
        let millis = now.timestamp_millis();
        assert!(millis > 0);
    }

    // TIMER-006: includeMetadata option
    #[test]
    fn test_include_metadata_default_is_true() {
        let config = TimerConfig::from_uri("timer:tick").unwrap();
        assert!(
            config.include_metadata,
            "includeMetadata should default to true"
        );
    }

    #[test]
    fn test_include_metadata_false_from_uri() {
        let config = TimerConfig::from_uri("timer:tick?includeMetadata=false").unwrap();
        assert!(
            !config.include_metadata,
            "includeMetadata should be false when set in URI"
        );
    }

    #[tokio::test]
    async fn test_include_metadata_false_omits_headers() {
        let component = TimerComponent::new();
        let endpoint = component
            .create_endpoint(
                "timer:minimal?period=50&repeatCount=1&includeMetadata=false",
                &NoOpComponentContext,
            )
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let ctx = ConsumerContext::new(
            tx,
            tokio_util::sync::CancellationToken::new(),
            "timer-test-route".to_string(),
        );

        tokio::spawn(async move {
            consumer.start(ctx).await.unwrap();
        });

        let envelope = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("should receive exchange")
            .expect("envelope should exist");

        let exchange = envelope.exchange;

        // No metadata headers should be present
        assert!(
            exchange.input.header("CamelTimerName").is_none(),
            "CamelTimerName should not be present when includeMetadata=false"
        );
        assert!(
            exchange.input.header("CamelTimerCounter").is_none(),
            "CamelTimerCounter should not be present when includeMetadata=false"
        );
        assert!(
            exchange.input.header("CamelTimerFiredTime").is_none(),
            "CamelTimerFiredTime should not be present when includeMetadata=false"
        );
        assert!(
            exchange.input.header("CamelMessageTimestamp").is_none(),
            "CamelMessageTimestamp should not be present when includeMetadata=false"
        );
    }

    #[tokio::test]
    async fn test_include_metadata_true_includes_all_headers() {
        let component = TimerComponent::new();
        let endpoint = component
            .create_endpoint(
                "timer:full?period=50&repeatCount=1&includeMetadata=true",
                &NoOpComponentContext,
            )
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let ctx = ConsumerContext::new(
            tx,
            tokio_util::sync::CancellationToken::new(),
            "timer-test-route".to_string(),
        );

        tokio::spawn(async move {
            consumer.start(ctx).await.unwrap();
        });

        let envelope = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("should receive exchange")
            .expect("envelope should exist");

        let exchange = envelope.exchange;

        assert!(exchange.input.header("CamelTimerName").is_some());
        assert!(exchange.input.header("CamelTimerCounter").is_some());
        assert!(exchange.input.header("CamelTimerFiredTime").is_some());
        assert!(exchange.input.header("CamelMessageTimestamp").is_some());
    }

    // TIMER-011: TimerEndpoint and TimerConsumer are pub
    #[test]
    fn test_timer_endpoint_is_pub() {
        let component = TimerComponent::new();
        let endpoint = component
            .create_endpoint("timer:pub-test", &NoOpComponentContext)
            .unwrap();
        assert_eq!(endpoint.uri(), "timer:pub-test");
    }
}
