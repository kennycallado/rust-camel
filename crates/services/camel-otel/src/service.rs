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
use camel_api::{CamelError, Lifecycle, ServiceStatus};
use opentelemetry::KeyValue;
use opentelemetry::global;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{LogExporter, MetricExporter, SpanExporter, WithExportConfig};
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::resource::Resource;
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;
use tracing::warn;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

use crate::config::{OtelConfig, OtelProtocol, OtelSampler};

/// Status values for atomic tracking
const STATUS_STOPPED: u8 = 0;
const STATUS_STARTED: u8 = 1;
const STATUS_FAILED: u8 = 2;

/// OpenTelemetry service that manages the lifecycle of OTel providers.
///
/// This service initializes the global `TracerProvider` and `MeterProvider`
/// on start, and shuts them down gracefully on stop.
pub struct OtelService {
    config: OtelConfig,
    tracer_provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
    logger_provider: Option<SdkLoggerProvider>,
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

    /// Build the OTLP log exporter based on the configured protocol.
    fn build_log_exporter(&self) -> Result<LogExporter, CamelError> {
        match self.config.protocol {
            OtelProtocol::Grpc => LogExporter::builder()
                .with_tonic()
                .with_endpoint(&self.config.endpoint)
                .build()
                .map_err(|e| {
                    CamelError::Config(format!("Failed to build gRPC log exporter: {}", e))
                }),
            OtelProtocol::HttpProtobuf => LogExporter::builder()
                .with_http()
                .with_endpoint(format!("{}/v1/logs", self.config.endpoint))
                .build()
                .map_err(|e| {
                    CamelError::Config(format!("Failed to build HTTP log exporter: {}", e))
                }),
        }
    }

    /// Initialize the tracing subscriber with fmt + OTel log bridge layers.
    ///
    /// This composes:
    /// - `tracing_subscriber::fmt` layer for console output
    /// - `OpenTelemetryTracingBridge` layer for OTLP log export
    /// - `EnvFilter` to suppress noisy internal crates (hyper, tonic, h2, reqwest)
    ///
    /// Uses `try_init()` so it's safe if a subscriber is already set.
    fn init_subscriber(&self, logger_provider: &SdkLoggerProvider) {
        let otel_layer = OpenTelemetryTracingBridge::new(logger_provider);

        let filter = EnvFilter::new(format!(
            "{level},h2=off,hyper=off,tonic=off,reqwest=off,tower=off",
            level = self.config.log_level
        ));

        let fmt_layer = fmt::layer().with_target(false);

        // try_init() silently ignores "already set" errors
        let _ = tracing_subscriber::registry()
            .with(filter)
            .with(fmt_layer)
            .with(otel_layer)
            .try_init();
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
        if self.tracer_provider.is_some() || self.meter_provider.is_some() {
            warn!("OtelService already initialized, skipping start()");
            return Ok(());
        }

        let resource = self.build_resource();

        // Build and install LoggerProvider + subscriber FIRST (before any tracing calls)
        if self.config.logs_enabled {
            let log_exporter = match self.build_log_exporter() {
                Ok(exporter) => exporter,
                Err(e) => {
                    self.status.store(STATUS_FAILED, Ordering::SeqCst);
                    return Err(e);
                }
            };

            let logger_provider = SdkLoggerProvider::builder()
                .with_resource(resource.clone())
                .with_batch_exporter(log_exporter)
                .build();

            self.init_subscriber(&logger_provider);
            self.logger_provider = Some(logger_provider);
        }

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

        self.status.store(STATUS_STARTED, Ordering::SeqCst);

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        // Shutdown LoggerProvider first (flush pending logs before traces/metrics)
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

        self.status.store(STATUS_STOPPED, Ordering::SeqCst);

        Ok(())
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
        assert!(service.logger_provider.is_none());
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
    #[ignore = "Sets global OTel state which interferes with parallel tests. Run with: cargo test -p camel-otel -- --ignored"]
    async fn test_start_stop_lifecycle() {
        // This test verifies the lifecycle works without connecting to a real backend.
        // It will fail to connect, but that's OK - we're testing the flow.
        let config = OtelConfig::new("http://localhost:9999", "test-service"); // Invalid port
        let mut service = OtelService::new(config);

        // Verify initial state
        assert!(service.tracer_provider.is_none());
        assert!(service.meter_provider.is_none());
        assert!(service.logger_provider.is_none());
        assert_eq!(service.status(), ServiceStatus::Stopped);

        // Note: start() may fail because the endpoint is invalid, but we test the logic
        let result = service.start().await;

        // The exporter build should succeed (build happens lazily), so start() typically
        // succeeds even with an invalid endpoint. Either way, verify state consistency.
        if result.is_ok() {
            // On success, providers should be set and status should be Started
            assert!(service.tracer_provider.is_some());
            assert!(service.meter_provider.is_some());
            assert!(service.logger_provider.is_some());
            assert_eq!(service.status(), ServiceStatus::Started);

            // Stop should work
            let stop_result = service.stop().await;
            assert!(stop_result.is_ok());

            // Providers should be cleared and status should be Stopped
            assert!(service.tracer_provider.is_none());
            assert!(service.meter_provider.is_none());
            assert!(service.logger_provider.is_none());
            assert_eq!(service.status(), ServiceStatus::Stopped);
        } else {
            // On failure, state must remain clean
            assert!(service.tracer_provider.is_none());
            assert!(service.meter_provider.is_none());
            assert_eq!(service.status(), ServiceStatus::Stopped);
        }
    }

    #[tokio::test]
    #[ignore = "Potentially sets global OTel state. The guard should prevent it, but ignored for safety in parallel test runs. Run with: cargo test -p camel-otel -- --ignored"]
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
}
