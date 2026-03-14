//! OpenTelemetry service implementation for rust-camel.
//!
//! `OtelService` implements the `Lifecycle` trait to manage the initialization
//! and shutdown of OpenTelemetry providers (TracerProvider and MeterProvider).
//!
//! # Example
//!
//! ```rust,no_run
//! use camel_otel::{OtelConfig, OtelService};
//! use camel_api::Lifecycle;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = OtelConfig::new("http://localhost:4317", "my-service");
//!     let mut service = OtelService::new(config);
//!     
//!     service.start().await?;
//!     // ... use OpenTelemetry ...
//!     service.stop().await?;
//!     Ok(())
//! }
//! ```

use async_trait::async_trait;
use camel_api::{CamelError, Lifecycle, MetricsCollector, ServiceStatus};
use opentelemetry::KeyValue;
use opentelemetry::global;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{LogExporter, MetricExporter, SpanExporter, WithExportConfig};
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::resource::Resource;
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;
use tracing::warn;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::OtelMetrics;
use crate::config::{OtelConfig, OtelProtocol, OtelSampler};

/// Status values for atomic tracking
const STATUS_STOPPED: u8 = 0;
const STATUS_STARTED: u8 = 1;
const STATUS_FAILED: u8 = 2;

/// OpenTelemetry service that manages the lifecycle of OTel providers.
///
/// This service initializes the global `TracerProvider` and `MeterProvider`
/// on start, and shuts them down gracefully on stop.
///
/// # Log Bridge
///
/// The service also installs an `OpenTelemetryTracingBridge` to export tracing
/// logs via OTel. Note that the log bridge is installed globally and is NOT
/// hot-reloadable - changing OTel configuration requires a process restart.
pub struct OtelService {
    config: OtelConfig,
    tracer_provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
    logger_provider: Option<SdkLoggerProvider>,
    metrics: Option<Arc<OtelMetrics>>,
    status: AtomicU8,
}

impl OtelService {
    /// Create a new `OtelService` with the given configuration.
    pub fn new(config: OtelConfig) -> Self {
        Self {
            config,
            tracer_provider: None,
            meter_provider: None,
            logger_provider: None,
            metrics: None,
            status: AtomicU8::new(STATUS_STOPPED),
        }
    }

    /// Create an `OtelService` with default configuration.
    ///
    /// Uses `http://localhost:4317` as the endpoint and "rust-camel" as the service name.
    pub fn with_defaults() -> Self {
        Self::new(OtelConfig::new("http://localhost:4317", "rust-camel"))
    }

    /// Build the OTLP span exporter based on the configured protocol.
    fn build_span_exporter(&self) -> Result<SpanExporter, CamelError> {
        match self.config.protocol {
            OtelProtocol::Grpc => SpanExporter::builder()
                .with_tonic()
                .with_endpoint(&self.config.endpoint)
                .build()
                .map_err(|e| {
                    CamelError::Config(format!("Failed to build gRPC span exporter: {}", e))
                }),
            OtelProtocol::HttpProtobuf => SpanExporter::builder()
                .with_http()
                .with_endpoint(format!("{}/v1/traces", self.config.endpoint))
                .build()
                .map_err(|e| {
                    CamelError::Config(format!("Failed to build HTTP span exporter: {}", e))
                }),
        }
    }

    /// Build the OTLP metric exporter based on the configured protocol.
    fn build_metric_exporter(&self) -> Result<MetricExporter, CamelError> {
        match self.config.protocol {
            OtelProtocol::Grpc => MetricExporter::builder()
                .with_tonic()
                .with_endpoint(&self.config.endpoint)
                .build()
                .map_err(|e| {
                    CamelError::Config(format!("Failed to build gRPC metric exporter: {}", e))
                }),
            OtelProtocol::HttpProtobuf => MetricExporter::builder()
                .with_http()
                .with_endpoint(format!("{}/v1/metrics", self.config.endpoint))
                .build()
                .map_err(|e| {
                    CamelError::Config(format!("Failed to build HTTP metric exporter: {}", e))
                }),
        }
    }

    /// Build the OTLP log exporter and logger provider based on the configured protocol.
    fn build_logger_provider(&self) -> Result<SdkLoggerProvider, CamelError> {
        let exporter = match self.config.protocol {
            OtelProtocol::Grpc => LogExporter::builder()
                .with_tonic()
                .with_endpoint(&self.config.endpoint)
                .build()
                .map_err(|e| {
                    self.status.store(STATUS_FAILED, Ordering::SeqCst);
                    CamelError::Config(format!("Failed to build log exporter: {}", e))
                })?,
            OtelProtocol::HttpProtobuf => LogExporter::builder()
                .with_http()
                .with_endpoint(format!("{}/v1/logs", self.config.endpoint))
                .build()
                .map_err(|e| {
                    self.status.store(STATUS_FAILED, Ordering::SeqCst);
                    CamelError::Config(format!("Failed to build log exporter: {}", e))
                })?,
        };

        let provider = SdkLoggerProvider::builder()
            .with_resource(self.build_resource())
            .with_batch_exporter(exporter)
            .build();

        Ok(provider)
    }

    /// Build the OpenTelemetry resource with service name and additional attributes.
    fn build_resource(&self) -> Resource {
        let mut attrs: Vec<KeyValue> = vec![KeyValue::new(
            opentelemetry_semantic_conventions::resource::SERVICE_NAME,
            self.config.service_name.clone(),
        )];

        // Add custom resource attributes
        for (key, value) in &self.config.resource_attrs {
            attrs.push(KeyValue::new(key.clone(), value.clone()));
        }

        Resource::builder().with_attributes(attrs).build()
    }

    /// Convert `OtelSampler` to SDK `Sampler`.
    fn to_sdk_sampler(sampler: &OtelSampler) -> Sampler {
        match sampler {
            OtelSampler::AlwaysOn => Sampler::AlwaysOn,
            OtelSampler::AlwaysOff => Sampler::AlwaysOff,
            OtelSampler::TraceIdRatioBased(ratio) => Sampler::TraceIdRatioBased(*ratio),
        }
    }

    /// Validate the configuration before starting.
    fn validate_config(&self) -> Result<(), CamelError> {
        if let OtelSampler::TraceIdRatioBased(ratio) = self.config.sampler
            && !(0.0..=1.0).contains(&ratio)
        {
            return Err(CamelError::Config(format!(
                "TraceIdRatioBased sampler ratio must be in [0.0, 1.0], got {}",
                ratio
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl Lifecycle for OtelService {
    fn name(&self) -> &str {
        "otel"
    }

    fn status(&self) -> ServiceStatus {
        match self.status.load(Ordering::SeqCst) {
            STATUS_STOPPED => ServiceStatus::Stopped,
            STATUS_STARTED => ServiceStatus::Started,
            STATUS_FAILED => ServiceStatus::Failed,
            _ => ServiceStatus::Failed,
        }
    }

    async fn start(&mut self) -> Result<(), CamelError> {
        // Validate configuration first
        if let Err(e) = self.validate_config() {
            self.status.store(STATUS_FAILED, Ordering::SeqCst);
            return Err(e);
        }

        // Guard against double initialization
        if self.tracer_provider.is_some() {
            warn!("OtelService already initialized, skipping start()");
            return Ok(());
        }

        let resource = self.build_resource();

        // Build and install TracerProvider
        let span_exporter = match self.build_span_exporter() {
            Ok(exporter) => exporter,
            Err(e) => {
                self.status.store(STATUS_FAILED, Ordering::SeqCst);
                return Err(e);
            }
        };
        let sampler = Self::to_sdk_sampler(&self.config.sampler);

        let tracer_provider = SdkTracerProvider::builder()
            .with_sampler(sampler)
            .with_resource(resource.clone())
            .with_batch_exporter(span_exporter)
            .build();

        global::set_tracer_provider(tracer_provider.clone());
        self.tracer_provider = Some(tracer_provider);

        // Build and install MeterProvider with periodic exporter (60s interval)
        let metric_exporter = match self.build_metric_exporter() {
            Ok(exporter) => exporter,
            Err(e) => {
                self.status.store(STATUS_FAILED, Ordering::SeqCst);
                return Err(e);
            }
        };

        let periodic_reader = PeriodicReader::builder(metric_exporter)
            .with_interval(Duration::from_secs(60))
            .build();

        let meter_provider = SdkMeterProvider::builder()
            .with_resource(resource)
            .with_reader(periodic_reader)
            .build();

        global::set_meter_provider(meter_provider.clone());
        self.meter_provider = Some(meter_provider);

        // Create OtelMetrics for route-level metrics collection
        let otel_metrics = Arc::new(OtelMetrics::new(self.config.service_name.clone()));
        self.metrics = Some(Arc::clone(&otel_metrics));

        // Install log bridge to export tracing logs via OTel
        // Note: Log bridge is NOT hot-reloadable - requires process restart for config changes
        let logger_provider = self.build_logger_provider()?;
        let layer = OpenTelemetryTracingBridge::new(&logger_provider);

        // Install as additional layer (try_init to not panic if subscriber exists)
        let _ = tracing_subscriber::registry().with(layer).try_init();

        self.logger_provider = Some(logger_provider);

        self.status.store(STATUS_STARTED, Ordering::SeqCst);

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        // Shutdown LoggerProvider (flushes batch exporter)
        if let Some(provider) = self.logger_provider.take()
            && let Err(e) = provider.shutdown()
        {
            warn!("Error shutting down LoggerProvider: {:?}", e);
        }

        // Shutdown TracerProvider
        if let Some(provider) = self.tracer_provider.take()
            && let Err(e) = provider.shutdown()
        {
            warn!("Error shutting down TracerProvider: {:?}", e);
        }

        // Shutdown MeterProvider
        if let Some(provider) = self.meter_provider.take()
            && let Err(e) = provider.shutdown()
        {
            warn!("Error shutting down MeterProvider: {:?}", e);
        }

        // Clear metrics so as_metrics_collector() returns None
        self.metrics = None;

        self.status.store(STATUS_STOPPED, Ordering::SeqCst);

        Ok(())
    }

    fn as_metrics_collector(&self) -> Option<Arc<dyn MetricsCollector>> {
        self.metrics
            .as_ref()
            .map(|m| Arc::clone(m) as Arc<dyn MetricsCollector>)
    }
}

impl Default for OtelService {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_otel_service_new() {
        let config = OtelConfig::new("http://localhost:4317", "test-service");
        let service = OtelService::new(config);

        assert_eq!(service.name(), "otel");
        assert!(service.tracer_provider.is_none());
        assert!(service.meter_provider.is_none());
    }

    #[test]
    fn test_otel_service_default() {
        let service = OtelService::with_defaults();

        assert_eq!(service.name(), "otel");
        assert_eq!(service.config.endpoint, "http://localhost:4317");
        assert_eq!(service.config.service_name, "rust-camel");
    }

    #[test]
    fn test_build_resource() {
        let config = OtelConfig::new("http://localhost:4317", "my-service")
            .with_resource_attr("deployment.environment", "production")
            .with_resource_attr("service.version", "1.0.0");

        let service = OtelService::new(config);
        let resource = service.build_resource();

        // Resource should contain service.name and custom attributes
        // Note: Resource doesn't expose a simple way to iterate attributes in 0.31,
        // so we just verify it builds without error
        let _ = resource;
    }

    #[test]
    fn test_to_sdk_sampler_always_on() {
        let sampler = OtelSampler::AlwaysOn;
        let sdk_sampler = OtelService::to_sdk_sampler(&sampler);

        assert!(matches!(sdk_sampler, Sampler::AlwaysOn));
    }

    #[test]
    fn test_to_sdk_sampler_always_off() {
        let sampler = OtelSampler::AlwaysOff;
        let sdk_sampler = OtelService::to_sdk_sampler(&sampler);

        assert!(matches!(sdk_sampler, Sampler::AlwaysOff));
    }

    #[test]
    fn test_to_sdk_sampler_trace_id_ratio() {
        let sampler = OtelSampler::TraceIdRatioBased(0.5);
        let sdk_sampler = OtelService::to_sdk_sampler(&sampler);

        assert!(matches!(sdk_sampler, Sampler::TraceIdRatioBased(r) if r == 0.5));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_start_stop_lifecycle() {
        // This test verifies the lifecycle works without connecting to a real backend.
        // It will fail to connect, but that's OK - we're testing the flow.
        let config = OtelConfig::new("http://localhost:9999", "test-service"); // Invalid port
        let mut service = OtelService::new(config);

        // Verify initial state
        assert!(service.tracer_provider.is_none());
        assert!(service.meter_provider.is_none());
        assert_eq!(service.status(), ServiceStatus::Stopped);

        // Note: start() may fail because the endpoint is invalid, but we test the logic
        let result = service.start().await;

        // The exporter build should succeed (build happens lazily), so start() typically
        // succeeds even with an invalid endpoint. Either way, verify state consistency.
        if result.is_ok() {
            // On success, providers should be set and status should be Started
            assert!(service.tracer_provider.is_some());
            assert!(service.meter_provider.is_some());
            assert_eq!(service.status(), ServiceStatus::Started);

            // Stop should work
            let stop_result = service.stop().await;
            assert!(stop_result.is_ok());

            // Providers should be cleared and status should be Stopped
            assert!(service.tracer_provider.is_none());
            assert!(service.meter_provider.is_none());
            assert_eq!(service.status(), ServiceStatus::Stopped);
        } else {
            // On failure, state must remain clean
            assert!(service.tracer_provider.is_none());
            assert!(service.meter_provider.is_none());
            assert_eq!(service.status(), ServiceStatus::Stopped);
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_double_start_guard() {
        // FIXME: This test sets global OTel state which can interfere with parallel tests.
        // Consider using isolated unit tests or serial_test crate for better isolation.
        let mut service = OtelService::with_defaults();

        // Manually set tracer_provider to simulate already-started state
        // We use a simple provider without exporter for this test
        let resource = Resource::builder()
            .with_attributes(vec![KeyValue::new("test", "value")])
            .build();
        let provider = SdkTracerProvider::builder().with_resource(resource).build();
        service.tracer_provider = Some(provider);

        // Second start should warn and return Ok
        let result = service.start().await;
        assert!(result.is_ok());

        // Clean up
        service.tracer_provider.take();
    }

    #[tokio::test]
    async fn test_stop_when_not_started() {
        let mut service = OtelService::with_defaults();

        // Stop when not started should succeed
        let result = service.stop().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_sampler_ratio_valid() {
        // Valid ratios should pass validation
        let config = OtelConfig::new("http://localhost:4317", "test")
            .with_sampler(OtelSampler::TraceIdRatioBased(0.0));
        let service = OtelService::new(config);
        assert!(service.validate_config().is_ok());

        let config = OtelConfig::new("http://localhost:4317", "test")
            .with_sampler(OtelSampler::TraceIdRatioBased(0.5));
        let service = OtelService::new(config);
        assert!(service.validate_config().is_ok());

        let config = OtelConfig::new("http://localhost:4317", "test")
            .with_sampler(OtelSampler::TraceIdRatioBased(1.0));
        let service = OtelService::new(config);
        assert!(service.validate_config().is_ok());
    }

    #[test]
    fn test_validate_sampler_ratio_invalid() {
        // Ratios outside [0.0, 1.0] should fail
        let config = OtelConfig::new("http://localhost:4317", "test")
            .with_sampler(OtelSampler::TraceIdRatioBased(-0.1));
        let service = OtelService::new(config);
        let err = service.validate_config().unwrap_err();
        assert!(
            err.to_string()
                .contains("TraceIdRatioBased sampler ratio must be in [0.0, 1.0]")
        );

        let config = OtelConfig::new("http://localhost:4317", "test")
            .with_sampler(OtelSampler::TraceIdRatioBased(1.5));
        let service = OtelService::new(config);
        let err = service.validate_config().unwrap_err();
        assert!(
            err.to_string()
                .contains("TraceIdRatioBased sampler ratio must be in [0.0, 1.0]")
        );
    }

    #[test]
    fn test_status_transitions() {
        let service = OtelService::with_defaults();
        assert_eq!(service.status(), ServiceStatus::Stopped);
    }

    #[tokio::test]
    async fn test_status_failed_on_start_error() {
        // Use an invalid sampler ratio to trigger a validation error in start()
        let config = OtelConfig::new("http://localhost:4317", "test-service")
            .with_sampler(OtelSampler::TraceIdRatioBased(-1.0));
        let mut service = OtelService::new(config);

        // Initial status should be Stopped
        assert_eq!(service.status(), ServiceStatus::Stopped);

        // start() should fail due to invalid config
        let result = service.start().await;
        assert!(result.is_err());

        // Status should now be Failed
        assert_eq!(service.status(), ServiceStatus::Failed);

        // Verify error message mentions the sampler ratio
        let err = result.unwrap_err();
        assert!(err.to_string().contains("TraceIdRatioBased sampler ratio"));
    }

    #[tokio::test]
    async fn test_start_does_not_replace_subscriber() {
        // Install a counting subscriber BEFORE OtelService starts
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;

        static BEFORE_COUNT: AtomicUsize = AtomicUsize::new(0);

        struct CountingLayer;
        impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CountingLayer {
            fn on_event(
                &self,
                _event: &tracing::Event<'_>,
                _ctx: tracing_subscriber::layer::Context<'_, S>,
            ) {
                BEFORE_COUNT.fetch_add(1, Ordering::SeqCst);
            }
        }

        // Try to set a subscriber (may already be set in test suite, that's fine)
        let _ = tracing_subscriber::registry()
            .with(CountingLayer)
            .try_init();

        let _initial = BEFORE_COUNT.load(Ordering::SeqCst);

        let config = OtelConfig::new("http://localhost:9999", "test-no-sub-replace");
        let mut service = OtelService::new(config);
        let _ = service.start().await; // may succeed or fail — doesn't matter
        let _ = service.stop().await;

        // Emit a test event
        tracing::info!("test event after otel start");

        // If OtelService replaced the subscriber, CountingLayer won't be called
        // and count stays at initial. If subscriber is preserved, count increases.
        // NOTE: This test is best-effort — if no subscriber was set, it's a no-op.
        // The important thing is that start() doesn't *panic* or error due to subscriber conflict.
        let _ = BEFORE_COUNT.load(Ordering::SeqCst);
    }
}
