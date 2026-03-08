/// Protocol for OTLP export.
#[derive(Debug, Clone, Default)]
pub enum OtelProtocol {
    /// gRPC (default, port 4317)
    #[default]
    Grpc,
    /// HTTP/Protobuf (port 4318)
    HttpProtobuf,
}

/// Sampling strategy.
#[derive(Debug, Clone, Default)]
pub enum OtelSampler {
    /// Sample all traces (default, good for dev)
    #[default]
    AlwaysOn,
    /// Sample a ratio of traces (0.0–1.0)
    TraceIdRatioBased(f64),
    /// Sample no traces (effectively disables tracing)
    AlwaysOff,
}

/// Configuration for the OpenTelemetry service.
#[derive(Debug, Clone)]
pub struct OtelConfig {
    /// OTLP endpoint (e.g., "http://localhost:4317" for gRPC)
    pub endpoint: String,
    /// Service name reported to the backend
    pub service_name: String,
    /// Export protocol
    pub protocol: OtelProtocol,
    /// Sampling strategy
    pub sampler: OtelSampler,
    /// Additional resource attributes (key-value pairs)
    pub resource_attrs: Vec<(String, String)>,
    /// Whether to export logs via OTLP (default: true)
    pub logs_enabled: bool,
    /// Log level filter for the OTel subscriber (default: "info")
    pub log_level: String,
}

impl OtelConfig {
    /// Create a new config with the given endpoint and service name.
    /// Uses gRPC protocol and AlwaysOn sampler by default.
    pub fn new(endpoint: impl Into<String>, service_name: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            service_name: service_name.into(),
            protocol: OtelProtocol::default(),
            sampler: OtelSampler::default(),
            resource_attrs: vec![],
            logs_enabled: true,
            log_level: "info".to_string(),
        }
    }

    /// Set the export protocol.
    pub fn with_protocol(mut self, protocol: OtelProtocol) -> Self {
        self.protocol = protocol;
        self
    }

    /// Set the sampling strategy.
    pub fn with_sampler(mut self, sampler: OtelSampler) -> Self {
        self.sampler = sampler;
        self
    }

    /// Add a resource attribute.
    pub fn with_resource_attr(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.resource_attrs.push((key.into(), value.into()));
        self
    }

    /// Set the log level filter for the OTel subscriber.
    pub fn with_log_level(mut self, level: impl Into<String>) -> Self {
        self.log_level = level.into();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_otel_config_new() {
        let cfg = OtelConfig::new("http://localhost:4317", "my-service");
        assert_eq!(cfg.endpoint, "http://localhost:4317");
        assert_eq!(cfg.service_name, "my-service");
        assert!(cfg.resource_attrs.is_empty());
    }

    #[test]
    fn test_otel_config_builder() {
        let cfg = OtelConfig::new("http://localhost:4317", "my-service")
            .with_sampler(OtelSampler::TraceIdRatioBased(0.5))
            .with_resource_attr("env", "production");
        assert_eq!(cfg.resource_attrs.len(), 1);
        assert!(matches!(cfg.sampler, OtelSampler::TraceIdRatioBased(f) if f == 0.5));
    }

    #[test]
    fn test_otel_config_logs_enabled_default() {
        let cfg = OtelConfig::new("http://localhost:4317", "my-service");
        assert!(cfg.logs_enabled, "logs_enabled should default to true");
    }

    #[test]
    fn test_otel_config_log_level_default() {
        let cfg = OtelConfig::new("http://localhost:4317", "my-service");
        assert_eq!(cfg.log_level, "info", "log_level should default to 'info'");
    }

    #[test]
    fn test_otel_config_with_log_level() {
        let cfg = OtelConfig::new("http://localhost:4317", "my-service").with_log_level("debug");
        assert_eq!(cfg.log_level, "debug");
    }

    #[test]
    fn test_otel_config_logs_disabled() {
        let cfg = OtelConfig::new("http://localhost:4317", "my-service");
        // Mutate directly since it's pub
        let mut cfg = cfg;
        cfg.logs_enabled = false;
        assert!(!cfg.logs_enabled);
    }
}
