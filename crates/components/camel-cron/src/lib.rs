//! cron component for rust-camel — fires `Exchange` events on Unix cron schedules.
//!
//! Main types: `CronComponent`, `CronConsumer`, `CronConfig`, `CronEndpoint`.
//! URI format: `cron:name?schedule=0 2 * * *&timeZone=UTC`.
//!
//! # Features
//!
//! - **Unix 5-field cron**: `min hour dom month dow`
//! - **Timezone-aware**: UTC default, configurable via `timeZone` param
//! - **Misfire skip**: missed schedules during downtime are not replayed
//! - **SPI-backed**: delegates scheduling to `CronService` (default: `TokioCronService`)

mod tokio_impl;

pub use tokio_impl::TokioCronService;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tracing::debug;

use camel_component_api::{
    BoxProcessor, CamelError, Component, ComponentContext, Consumer, ConsumerContext, CronCallback,
    CronFire, CronSchedule, CronService, Endpoint, ProducerContext, RuntimeObservability,
    UriConfig,
};
use camel_component_api::{Exchange, Message};
use chrono_tz::Tz;

// ---------------------------------------------------------------------------
// CronConfig
// ---------------------------------------------------------------------------

/// Configuration parsed from a cron URI.
///
/// Format: `cron:name?schedule=0 2 * * *&timeZone=UTC&includeMetadata=true`
#[derive(Debug, Clone, UriConfig)]
#[uri_scheme = "cron"]
#[uri_config(skip_impl, crate = "camel_component_api")]
pub struct CronConfig {
    /// Cron trigger name (the path portion of the URI).
    pub name: String,

    /// 5-field Unix cron expression.
    #[uri_param(name = "schedule")]
    pub schedule: String,

    /// IANA timezone. Default: UTC.
    #[uri_param(name = "timeZone", default = "UTC")]
    time_zone_str: String,

    /// Whether to include cron metadata headers in exchanges.
    #[uri_param(name = "includeMetadata", default = "true")]
    include_metadata: bool,
}

impl CronConfig {
    /// Parsed timezone.
    pub fn time_zone(&self) -> Result<Tz, CamelError> {
        self.time_zone_str.parse::<Tz>().map_err(|e| {
            CamelError::EndpointCreationFailed(format!(
                "invalid timeZone '{}': {}",
                self.time_zone_str, e
            ))
        })
    }

    /// Whether metadata headers are included.
    pub fn include_metadata(&self) -> bool {
        self.include_metadata
    }

    /// Normalize a 5-field Unix cron expression to the `cron` crate's
    /// 6-field format (prepend `0` for the seconds field).
    ///
    /// `"0 2 * * *"` → `"0 0 2 * * *"` (sec=0, min=0, hour=2)
    fn normalized_cron_expression(&self) -> String {
        format!("0 {}", self.schedule)
    }

    /// Validate: name non-empty, schedule is valid 5-field cron, timezone parseable.
    pub fn validate(&self) -> Result<(), CamelError> {
        if self.name.is_empty() {
            return Err(CamelError::EndpointCreationFailed(
                "cron name must not be empty".to_string(),
            ));
        }
        if self.schedule.is_empty() {
            return Err(CamelError::EndpointCreationFailed(
                "cron schedule must not be empty".to_string(),
            ));
        }
        // We restrict to 5-field Unix by counting fields.
        let field_count = self.schedule.split_whitespace().count();
        if field_count != 5 {
            return Err(CamelError::EndpointCreationFailed(format!(
                "cron schedule must be 5-field Unix format (min hour dom month dow), got {} field(s): '{}'",
                field_count, self.schedule
            )));
        }
        // The `cron` crate expects 6-7 fields (sec min hour dom month dow [year]).
        // Normalize our 5-field Unix expression before parsing.
        self.normalized_cron_expression()
            .parse::<cron::Schedule>()
            .map_err(|e| {
                CamelError::EndpointCreationFailed(format!(
                    "invalid cron expression '{}': {}",
                    self.schedule, e
                ))
            })?;
        // Validate timezone parses
        self.time_zone()?;
        Ok(())
    }
}

impl UriConfig for CronConfig {
    fn scheme() -> &'static str {
        "cron"
    }

    fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = camel_component_api::parse_uri(uri)?;
        Self::from_components(parts)
    }

    fn from_components(parts: camel_component_api::UriComponents) -> Result<Self, CamelError> {
        let config = Self::parse_uri_components(parts)?;
        CronConfig::validate(&config)?;
        Ok(config)
    }

    fn validate(self) -> Result<Self, CamelError> {
        CronConfig::validate(&self)?;
        Ok(self)
    }
}

// ---------------------------------------------------------------------------
// CronEndpoint
// ---------------------------------------------------------------------------

/// Endpoint for the `cron:` scheme.
pub struct CronEndpoint {
    uri: String,
    config: CronConfig,
    service: Arc<dyn CronService>,
}

impl CronEndpoint {
    pub fn new(uri: String, config: CronConfig, service: Arc<dyn CronService>) -> Self {
        Self {
            uri,
            config,
            service,
        }
    }
}

impl Endpoint for CronEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(CronConsumer::new(
            self.config.clone(),
            self.service.clone(),
        )))
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Err(CamelError::EndpointCreationFailed(
            "cron: component is consumer-only".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// CronConsumer
// ---------------------------------------------------------------------------

/// Consumer that fires Exchanges on a cron schedule.
pub struct CronConsumer {
    config: CronConfig,
    service: Arc<dyn CronService>,
    started: AtomicBool,
}

impl CronConsumer {
    /// Create a new CronConsumer with the given config and scheduling service.
    pub fn new(config: CronConfig, service: Arc<dyn CronService>) -> Self {
        Self {
            config,
            service,
            started: AtomicBool::new(false),
        }
    }

    /// Test helper: force the started flag to true.
    #[cfg(test)]
    pub(crate) fn mark_started_for_test(&self) {
        self.started.store(true, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl Consumer for CronConsumer {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        self.started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| {
                CamelError::EndpointCreationFailed("cron consumer already started".to_string())
            })?;

        CronConfig::validate(&self.config)?;
        let config = self.config.clone();

        let schedule = CronSchedule {
            expression: config.normalized_cron_expression(),
            time_zone: config.time_zone()?,
            trigger_id: config.name.clone(),
            route_id: Some(context.route_id().to_string()),
        };

        let include_metadata = config.include_metadata();
        let trigger_name = config.name.clone();
        let trigger_schedule = config.schedule.clone();
        let time_zone_str = config.time_zone_str.clone();

        // Build the callback that submits Exchanges to the pipeline.
        let ctx = context.clone();
        let callback: CronCallback = Arc::new(move |fire: CronFire| {
            let ctx = ctx.clone();
            let name = trigger_name.clone();
            let sched = trigger_schedule.clone();
            let tz = time_zone_str.clone();
            Box::pin(async move {
                let mut exchange = Exchange::new(Message::default());
                if include_metadata {
                    exchange.input.set_header("CamelCronName", name);
                    exchange.input.set_header("CamelCronSchedule", sched);
                    exchange.input.set_header("CamelCronTimezone", tz);
                    exchange
                        .input
                        .set_header("CamelCronScheduledTime", fire.scheduled_at.to_rfc3339());
                    exchange
                        .input
                        .set_header("CamelCronFiredTime", fire.fired_at.to_rfc3339());
                    exchange
                        .input
                        .set_header("CamelCronCounter", fire.counter.to_string());
                }
                ctx.send(exchange).await
            })
        });

        let cancel_token = context.cancel_token();
        let trigger_id = schedule.trigger_id.clone();

        // Delegate to the CronService. On error, propagate to Route supervision.
        let result = self.service.run(&schedule, callback, cancel_token).await;

        self.started.store(false, Ordering::SeqCst);

        debug!(trigger = trigger_id, "cron consumer stopped");
        result
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        // The cancel token (held by ConsumerContext) triggers service::run to return.
        // started flag is reset in start() after run returns.
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// CronComponent
// ---------------------------------------------------------------------------

/// Component for the `cron:` scheme. Factory for `CronEndpoint`s.
///
/// By default uses `TokioCronService`. Inject a custom `CronService` via
/// [`CronComponent::with_service`] for alternative backends.
pub struct CronComponent {
    service: Arc<dyn CronService>,
}

impl Default for CronComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl CronComponent {
    /// Create with the default `TokioCronService`.
    pub fn new() -> Self {
        Self {
            service: Arc::new(TokioCronService),
        }
    }

    /// Create with a custom `CronService` implementation.
    pub fn with_service(service: Arc<dyn CronService>) -> Self {
        Self { service }
    }
}

impl Component for CronComponent {
    fn scheme(&self) -> &str {
        "cron"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let config = CronConfig::from_uri(uri)?;
        Ok(Box::new(CronEndpoint::new(
            uri.to_string(),
            config,
            self.service.clone(),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cron_config_basic() {
        let config = CronConfig::from_uri("cron:nightly?schedule=0%202%20*%20*%20*").unwrap();
        assert_eq!(config.name, "nightly");
        assert_eq!(config.schedule, "0 2 * * *");
        assert_eq!(config.time_zone_str, "UTC");
        assert!(config.include_metadata);
    }

    #[test]
    fn test_cron_config_with_timezone() {
        let config =
            CronConfig::from_uri("cron:morning?schedule=0%209%20*%20*%201&timeZone=America/Lima")
                .unwrap();
        assert_eq!(config.name, "morning");
        assert_eq!(config.time_zone().unwrap(), chrono_tz::Tz::America__Lima);
    }

    #[test]
    fn test_cron_config_percent_encoded_spaces() {
        // URI parser should decode %20 to space
        let config = CronConfig::from_uri("cron:test?schedule=0%202%20*%20*%20*").unwrap();
        assert_eq!(config.schedule, "0 2 * * *");
        config.validate().unwrap();
    }

    #[test]
    fn test_rejects_empty_name() {
        let err = CronConfig::from_uri("cron:?schedule=0%202%20*%20*%20*").unwrap_err();
        assert!(
            err.to_string().contains("must not be empty"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_rejects_missing_schedule() {
        let result = CronConfig::from_uri("cron:test");
        // schedule is required — should fail at URI parse or validate
        assert!(result.is_err() || result.unwrap().validate().is_err());
    }

    #[test]
    fn test_rejects_six_field_cron() {
        let err = CronConfig::from_uri("cron:test?schedule=0%200%202%20*%20*%20*").unwrap_err();
        assert!(err.to_string().contains("5-field"));
    }

    #[test]
    fn test_rejects_invalid_cron() {
        let err = CronConfig::from_uri("cron:test?schedule=99%2099%2099%2099%2099").unwrap_err();
        assert!(
            err.to_string().contains("invalid cron"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_rejects_invalid_timezone() {
        let err = CronConfig::from_uri("cron:test?schedule=0%202%20*%20*%20*&timeZone=NotAZone")
            .unwrap_err();
        assert!(
            err.to_string().contains("invalid timeZone"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_valid_expressions() {
        // Each URI must encode a valid 5-field Unix cron expression.
        // 1. "0 2 * * *"        → daily at 02:00
        // 2. "*/5 * * * *"      → every 5 minutes
        // 3. "0 9 * * 1"        → Mondays at 09:00
        for expr in &[
            "0%202%20*%20*%20*",
            "*%2F5%20*%20*%20*%20*",
            "0%209%20*%20*%201",
        ] {
            let config = CronConfig::from_uri(&format!("cron:t?schedule={}", expr)).unwrap();
            config
                .validate()
                .unwrap_or_else(|e| panic!("'{}' should be valid: {}", expr, e));
        }
    }

    // --- Mock CronService for consumer tests (fires every 10ms) ---

    struct FastCronService;
    #[async_trait::async_trait]
    impl CronService for FastCronService {
        async fn run(
            &self,
            _schedule: &CronSchedule,
            callback: CronCallback,
            cancel: tokio_util::sync::CancellationToken,
        ) -> Result<(), CamelError> {
            let mut counter = 0u64;
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return Ok(()),
                    _ = tokio::time::sleep(tokio::time::Duration::from_millis(10)) => {
                        counter += 1;
                        let now = chrono::Utc::now();
                        let fire = CronFire {
                            scheduled_at: now,
                            fired_at: now,
                            counter,
                        };
                        callback(fire).await?;
                    }
                }
            }
        }
    }

    // --- CronConsumer tests ---

    #[tokio::test]
    async fn test_double_start_returns_error() {
        let config = CronConfig::from_uri("cron:test?schedule=0 2 * * *").unwrap();
        let mut consumer = CronConsumer::new(config, Arc::new(FastCronService));
        consumer.mark_started_for_test();

        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let cancel = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, cancel, "test-route".to_string());
        let result = consumer.start(ctx).await;
        assert!(result.is_err(), "double-start should error");
    }

    #[tokio::test]
    async fn test_consumer_fires_exchange() {
        let config = CronConfig::from_uri("cron:test?schedule=* * * * *").unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let cancel = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, cancel.clone(), "test-route".to_string());
        let mut consumer = CronConsumer::new(config, Arc::new(FastCronService));

        let cancel2 = cancel.clone();
        let handle = tokio::spawn(async move { consumer.start(ctx).await });

        // FastCronService fires every 10ms — should get an exchange quickly
        let exchange = tokio::time::timeout(tokio::time::Duration::from_secs(2), rx.recv()).await;
        assert!(exchange.is_ok(), "should have received an exchange");

        cancel2.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_consumer_metadata_headers() {
        let config = CronConfig::from_uri("cron:test?schedule=* * * * *").unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let cancel = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, cancel.clone(), "test-route".to_string());
        let mut consumer = CronConsumer::new(config, Arc::new(FastCronService));

        let cancel2 = cancel.clone();
        tokio::spawn(async move { consumer.start(ctx).await });

        let envelope = tokio::time::timeout(tokio::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();

        let headers = &envelope.exchange.input.headers;
        assert!(headers.contains_key("CamelCronName"));
        assert!(headers.contains_key("CamelCronSchedule"));
        assert!(headers.contains_key("CamelCronTimezone"));
        assert!(headers.contains_key("CamelCronScheduledTime"));
        assert!(headers.contains_key("CamelCronFiredTime"));
        assert!(headers.contains_key("CamelCronCounter"));

        cancel2.cancel();
    }

    #[tokio::test]
    async fn test_include_metadata_false_omits_headers() {
        let config =
            CronConfig::from_uri("cron:test?schedule=* * * * *&includeMetadata=false").unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let cancel = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, cancel.clone(), "test-route".to_string());
        let mut consumer = CronConsumer::new(config, Arc::new(FastCronService));

        let cancel2 = cancel.clone();
        tokio::spawn(async move { consumer.start(ctx).await });

        let envelope = tokio::time::timeout(tokio::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert!(
            envelope.exchange.input.headers.is_empty(),
            "no headers when includeMetadata=false"
        );

        cancel2.cancel();
    }

    #[tokio::test]
    async fn test_callback_error_propagates_to_start() {
        // If send fails (channel closed), start() should return Err.
        let config = CronConfig::from_uri("cron:test?schedule=* * * * *").unwrap();
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        // Drop receiver immediately so send fails
        drop(rx);
        let cancel = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, cancel, "test-route".to_string());
        let mut consumer = CronConsumer::new(config, Arc::new(FastCronService));

        let result =
            tokio::time::timeout(tokio::time::Duration::from_secs(2), consumer.start(ctx)).await;

        // Should complete (not timeout) with an error
        let inner = result.expect("should not timeout");
        assert!(
            inner.is_err(),
            "closed channel should propagate error to start()"
        );
    }

    // --- CronComponent tests ---

    #[test]
    fn test_cron_component_scheme() {
        let comp = CronComponent::new();
        assert_eq!(comp.scheme(), "cron");
    }

    #[test]
    fn test_cron_component_creates_endpoint() {
        let comp = CronComponent::new();
        let ctx = camel_component_api::NoOpComponentContext;
        let endpoint = comp
            .create_endpoint("cron:nightly?schedule=0 2 * * *", &ctx)
            .unwrap();
        assert_eq!(endpoint.uri(), "cron:nightly?schedule=0 2 * * *");
    }

    #[test]
    fn test_cron_component_rejects_invalid_schedule() {
        let comp = CronComponent::new();
        let ctx = camel_component_api::NoOpComponentContext;
        let result = comp.create_endpoint("cron:test?schedule=invalid", &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn test_cron_component_with_custom_service() {
        let custom_service: Arc<dyn CronService> = Arc::new(TokioCronService);
        let comp = CronComponent::with_service(custom_service);
        assert_eq!(comp.scheme(), "cron");
    }
}
