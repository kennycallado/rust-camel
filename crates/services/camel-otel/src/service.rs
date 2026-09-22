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
use camel_api::redact::redact_url;
use camel_api::{CamelError, Lifecycle, MetricsCollector, ServiceStatus};
use opentelemetry::KeyValue;
use opentelemetry::global;
use opentelemetry_otlp::{LogExporter, MetricExporter, SpanExporter, WithExportConfig};
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::resource::Resource;
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;
use tracing::{error, info, warn};

use crate::OtelMetrics;
use crate::config::{OtelConfig, OtelProtocol, OtelSampler};

#[cfg(test)]
#[path = "sampler_tests.rs"]
mod sampler_tests;
#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;

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
///
/// # Constraints
///
/// Only one `OtelService` should be active per process. Creating multiple
/// instances (e.g., in tests) may leave stale batch exporter tasks running.
/// Use `shutdown_logger_provider()` at process exit to fully clean up.
///
/// After `stop()`, the global meter provider installed by `start()` remains
/// registered (the OpenTelemetry global API has no unset), but it is shut
/// down; recordings after `stop()` hit a shut-down provider and are
/// effectively no-ops.
pub struct OtelService {
    config: OtelConfig,
    tracer_provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
    logger_provider: Option<SdkLoggerProvider>,
    metrics: Option<Arc<OtelMetrics>>,
    status: AtomicU8,
    #[cfg(test)]
    test_span_exporter: Option<opentelemetry_sdk::trace::InMemorySpanExporter>,
    #[cfg(test)]
    test_log_exporter: Option<opentelemetry_sdk::logs::InMemoryLogExporter>,
}

impl OtelService {
    pub fn new(config: OtelConfig) -> Self {
        let metrics = Arc::new(OtelMetrics::new(config.service_name.clone()));
        Self {
            config,
            tracer_provider: None,
            meter_provider: None,
            logger_provider: None,
            metrics: Some(metrics),
            status: AtomicU8::new(STATUS_STOPPED),
            #[cfg(test)]
            test_span_exporter: None,
            #[cfg(test)]
            test_log_exporter: None,
        }
    }

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
    ///
    /// This is a pure function — it does not mutate service status.
    /// The caller is responsible for setting status on error.
    fn build_logger_provider_internal(&self) -> Result<SdkLoggerProvider, CamelError> {
        let exporter = match self.config.protocol {
            OtelProtocol::Grpc => LogExporter::builder()
                .with_tonic()
                .with_endpoint(&self.config.endpoint)
                .build()
                .map_err(|e| CamelError::Config(format!("Failed to build log exporter: {}", e)))?,
            OtelProtocol::HttpProtobuf => LogExporter::builder()
                .with_http()
                .with_endpoint(format!("{}/v1/logs", self.config.endpoint))
                .build()
                .map_err(|e| CamelError::Config(format!("Failed to build log exporter: {}", e)))?,
        };

        let provider = SdkLoggerProvider::builder()
            .with_resource(self.build_resource())
            .with_batch_exporter(exporter)
            .build();

        Ok(provider)
    }

    /// Build the TracerProvider from configuration.
    ///
    /// In test builds, prefer an in-memory exporter wrapped in a synchronous
    /// `SimpleSpanProcessor` when the test has injected one; otherwise fall
    /// through to the production OTLP path. The non-test build always takes
    /// the production path.
    #[cfg(test)]
    fn build_tracer_provider(
        &self,
        sampler: Sampler,
        resource: Resource,
    ) -> Result<SdkTracerProvider, CamelError> {
        use opentelemetry_sdk::trace::SimpleSpanProcessor;
        if let Some(exporter) = &self.test_span_exporter {
            return Ok(SdkTracerProvider::builder()
                .with_sampler(sampler)
                .with_resource(resource)
                .with_span_processor(SimpleSpanProcessor::new(exporter.clone()))
                .build());
        }
        let span_exporter = self.build_span_exporter()?;
        Ok(SdkTracerProvider::builder()
            .with_sampler(sampler)
            .with_resource(resource)
            .with_batch_exporter(span_exporter)
            .build())
    }

    #[cfg(not(test))]
    fn build_tracer_provider(
        &self,
        sampler: Sampler,
        resource: Resource,
    ) -> Result<SdkTracerProvider, CamelError> {
        let span_exporter = self.build_span_exporter()?;
        Ok(SdkTracerProvider::builder()
            .with_sampler(sampler)
            .with_resource(resource)
            .with_batch_exporter(span_exporter)
            .build())
    }

    /// Build the LoggerProvider from configuration.
    ///
    /// In test builds, prefer an in-memory exporter wrapped in a synchronous
    /// `SimpleLogProcessor` when the test has injected one; otherwise fall
    /// through to the production OTLP path. The non-test build delegates
    /// directly to `build_logger_provider_internal`.
    #[cfg(test)]
    fn build_logger_provider(&self) -> Result<SdkLoggerProvider, CamelError> {
        use opentelemetry_sdk::logs::SimpleLogProcessor;
        if let Some(exporter) = &self.test_log_exporter {
            return Ok(SdkLoggerProvider::builder()
                .with_resource(self.build_resource())
                .with_log_processor(SimpleLogProcessor::new(exporter.clone()))
                .build());
        }
        self.build_logger_provider_internal()
    }

    #[cfg(not(test))]
    fn build_logger_provider(&self) -> Result<SdkLoggerProvider, CamelError> {
        self.build_logger_provider_internal()
    }

    /// Initializes and returns the `SdkLoggerProvider`.
    ///
    /// This should be called before initializing the `tracing_subscriber` so you can attach
    /// the OpenTelemetry log layer to your global subscriber.
    ///
    /// # Example
    /// ```rust,ignore
    /// let provider = otel_service.init_logger_provider()?;
    /// let layer = opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(&provider);
    /// tracing_subscriber::registry().with(tracing_subscriber::fmt::layer()).with(layer).init();
    /// ```
    pub fn init_logger_provider(&mut self) -> Result<SdkLoggerProvider, CamelError> {
        if let Some(p) = &self.logger_provider {
            return Ok(p.clone());
        }
        let p = self.build_logger_provider_internal()?;
        self.logger_provider = Some(p.clone());
        Ok(p)
    }

    pub fn shutdown_logger_provider(&mut self) {
        if let Some(provider) = self.logger_provider.take() {
            if let Err(e) = provider.force_flush() {
                warn!(
                    "Error force-flushing LoggerProvider during shutdown: {:?}",
                    e
                );
            }
            if let Err(e) = provider.shutdown() {
                warn!("Error shutting down LoggerProvider: {:?}", e);
            }
        }
    }

    /// Build and install the global TracerProvider early.
    ///
    /// Must be called before `init_tracing_subscriber()` so that
    /// `tracing_opentelemetry::layer()` picks up the global provider.
    pub fn init_tracer_provider(&mut self) -> Result<(), CamelError> {
        self.validate_config()?;

        if self.tracer_provider.is_some() {
            return Ok(());
        }

        let resource = self.build_resource();
        let span_exporter = self.build_span_exporter()?;
        let sampler = Self::to_sdk_sampler(&self.config.sampler);

        let tracer_provider = SdkTracerProvider::builder()
            .with_sampler(sampler)
            .with_resource(resource)
            .with_batch_exporter(span_exporter)
            .build();

        global::set_tracer_provider(tracer_provider.clone());
        self.tracer_provider = Some(tracer_provider);
        info!(
            endpoint = %redact_url(&self.config.endpoint),
            service_name = %self.config.service_name,
            "OTel TracerProvider initialized"
        );
        Ok(())
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

    /// Convert `OtelSampler` to a parent-based SDK sampler.
    ///
    /// Children inherit the parent sampling decision; an unsampled parent
    /// records nothing.
    fn to_sdk_sampler(sampler: &OtelSampler) -> Sampler {
        match sampler {
            OtelSampler::AlwaysOn => Sampler::ParentBased(Box::new(Sampler::AlwaysOn)),
            OtelSampler::AlwaysOff => Sampler::ParentBased(Box::new(Sampler::AlwaysOff)),
            OtelSampler::TraceIdRatioBased(ratio) => {
                Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(*ratio)))
            }
        }
    }

    /// Validate the configuration before starting.
    fn validate_config(&self) -> Result<(), CamelError> {
        // Delegate to OtelConfig::validate (URL parsing + service_name)
        self.config.validate()?;

        if let OtelSampler::TraceIdRatioBased(ratio) = self.config.sampler
            && !(0.0..=1.0).contains(&ratio)
        {
            return Err(CamelError::Config(format!(
                "TraceIdRatioBased sampler ratio must be in [0.0, 1.0], got {}",
                ratio
            )));
        }
        if self.config.metrics_interval_ms == 0 {
            return Err(CamelError::Config(
                "metrics_interval_ms must be > 0".to_string(),
            ));
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
        if self.status.load(Ordering::SeqCst) == STATUS_STARTED {
            info!("OTel service already started");
            return Ok(());
        }

        if let Err(e) = self.validate_config() {
            self.status.store(STATUS_FAILED, Ordering::SeqCst);
            // log-policy: system-broken
            error!(error = %e, "OTel config validation failed");
            return Err(e);
        }

        let resource = self.build_resource();

        // Build and install TracerProvider (if not already initialized early)
        if self.tracer_provider.is_none() {
            let sampler = Self::to_sdk_sampler(&self.config.sampler);

            let tracer_provider = match self.build_tracer_provider(sampler, resource.clone()) {
                Ok(tp) => tp,
                Err(e) => {
                    self.status.store(STATUS_FAILED, Ordering::SeqCst);
                    return Err(e);
                }
            };

            global::set_tracer_provider(tracer_provider.clone());
            self.tracer_provider = Some(tracer_provider);
        }

        // Build and install MeterProvider (if not already initialized)
        if self.meter_provider.is_none() {
            let metric_exporter = match self.build_metric_exporter() {
                Ok(exporter) => exporter,
                Err(e) => {
                    self.status.store(STATUS_FAILED, Ordering::SeqCst);
                    return Err(e);
                }
            };

            let periodic_reader = PeriodicReader::builder(metric_exporter)
                .with_interval(Duration::from_millis(self.config.metrics_interval_ms))
                .build();

            let meter_provider = SdkMeterProvider::builder()
                .with_resource(resource)
                .with_reader(periodic_reader)
                .build();

            global::set_meter_provider(meter_provider.clone());
            if let Some(m) = &self.metrics {
                m.mark_started();
            }
            self.meter_provider = Some(meter_provider);
            info!(
                endpoint = %redact_url(&self.config.endpoint),
                interval_ms = self.config.metrics_interval_ms,
                "OTel MeterProvider initialized"
            );
        }

        // Initialize logger provider if not already initialized
        if self.logger_provider.is_none() {
            let logger_provider = match self.build_logger_provider() {
                Ok(p) => p,
                Err(e) => {
                    self.status.store(STATUS_FAILED, Ordering::SeqCst);
                    return Err(e);
                }
            };

            self.logger_provider = Some(logger_provider);
        }

        self.status.store(STATUS_STARTED, Ordering::SeqCst);
        info!(service_name = %self.config.service_name, "OTel service started");

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        if self.status.load(Ordering::SeqCst) == STATUS_STOPPED {
            info!(service_name = %self.config.service_name, "OTel service already stopped, skipping stop()");
            return Ok(());
        }

        // Shutdown TracerProvider. `shutdown()` performs the final batch
        // flush (BSP worker drains the queue on the Shutdown message) and
        // is capped at 5s by the SDK. We intentionally do NOT call
        // force_flush() first: it is redundant (shutdown flushes the same
        // queue) and doubles the worst-case blocking under a stalled
        // export. rc-6ju71: in opentelemetry_sdk 0.32.1 the std
        // BatchSpanProcessor::force_flush() is itself capped at 5s
        // (`recv_timeout(forceflush_timeout)`), so this was never the
        // unbounded rc-q74u class here — but dropping it matches the
        // meter-path policy (shutdown-only) and removes exposure if
        // upstream ever reintroduces an unbounded flush receive
        // (PeriodicReader::force_flush had exactly that trap).
        if let Some(provider) = self.tracer_provider.take()
            && let Err(e) = provider.shutdown()
        {
            warn!(
                error = ?e,
                "Error shutting down TracerProvider; recent spans may not have been delivered"
            );
        }

        // Shutdown MeterProvider. PeriodicReader::shutdown() performs the
        // final collect+export and waits at most 5s (hardcoded in
        // opentelemetry_sdk 0.32.1). We intentionally do NOT call
        // force_flush() first: PeriodicReader::force_flush() waits
        // UNBOUNDED for the reader thread, and that thread can stall
        // forever inside the OTLP export when the ambient runtime cannot
        // make progress (current-thread runtime blocked in the flush
        // itself) — rc-q74u deadlock class.
        if let Some(provider) = self.meter_provider.take()
            && let Err(e) = provider.shutdown()
        {
            warn!(
                error = ?e,
                "Error shutting down MeterProvider; recent metrics may not have been delivered"
            );
        }

        // Shutdown LoggerProvider (flushes batch log exporter)
        if let Some(provider) = self.logger_provider.take() {
            if let Err(e) = provider.force_flush() {
                warn!("Error force-flushing LoggerProvider: {:?}", e);
            }
            if let Err(e) = provider.shutdown() {
                warn!("Error shutting down LoggerProvider: {:?}", e);
            }
        }

        // Note: metrics collector remains available (Arc<OtelMetrics> is stateless)

        self.status.store(STATUS_STOPPED, Ordering::SeqCst);
        info!(service_name = %self.config.service_name, "OTel service stopped");

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

impl Drop for OtelService {
    fn drop(&mut self) {
        if self.tracer_provider.is_none()
            && self.meter_provider.is_none()
            && self.logger_provider.is_none()
        {
            return;
        }

        // log-policy: system-broken
        warn!(
            service_name = %self.config.service_name,
            "OtelService dropped without stop(); shutting down providers best-effort"
        );

        // TracerProvider: shutdown only — no force_flush(). See stop() for
        // the rc-6ju71 rationale (shutdown performs the final flush with
        // the SDK's 5s cap; a separate force_flush is redundant).
        if let Some(provider) = self.tracer_provider.take() {
            let _ = provider.shutdown();
        }

        // MeterProvider: shutdown only — no force_flush(). See stop() for the
        // rc-q74u deadlock rationale (PeriodicReader::force_flush is
        // unbounded; shutdown is capped at 5s by the SDK).
        if let Some(provider) = self.meter_provider.take() {
            let _ = provider.shutdown();
        }

        if let Some(provider) = self.logger_provider.take() {
            let _ = provider.force_flush();
            let _ = provider.shutdown();
        }
    }
}
