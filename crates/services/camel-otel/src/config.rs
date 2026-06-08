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
    pub endpoint: String,
    pub service_name: String,
    pub protocol: OtelProtocol,
    pub sampler: OtelSampler,
    pub resource_attrs: Vec<(String, String)>,
    pub logs_enabled: bool,
    pub metrics_interval_ms: u64,
}

impl OtelConfig {
    pub fn new(endpoint: impl Into<String>, service_name: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            service_name: service_name.into(),
            protocol: OtelProtocol::default(),
            sampler: OtelSampler::default(),
            resource_attrs: vec![],
            logs_enabled: true,
            metrics_interval_ms: 60000,
        }
    }

    pub fn with_protocol(mut self, protocol: OtelProtocol) -> Self {
        self.protocol = protocol;
        self
    }

    pub fn with_sampler(mut self, sampler: OtelSampler) -> Self {
        self.sampler = sampler;
        self
    }

    pub fn with_resource_attr(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.resource_attrs.push((key.into(), value.into()));
        self
    }

    pub fn with_logs_enabled(mut self, enabled: bool) -> Self {
        self.logs_enabled = enabled;
        self
    }

    pub fn with_metrics_interval_ms(mut self, ms: u64) -> Self {
        self.metrics_interval_ms = ms;
        self
    }

    /// Validate the configuration, returning an error if any field is invalid.
    pub fn validate(&self) -> Result<(), camel_api::CamelError> {
        // service_name must be non-empty
        if self.service_name.trim().is_empty() {
            return Err(camel_api::CamelError::Config(
                "service_name must not be empty".to_string(),
            ));
        }

        // endpoint must be a valid URL
        if self.endpoint.trim().is_empty() {
            return Err(camel_api::CamelError::Config(
                "endpoint must not be empty".to_string(),
            ));
        }
        url::Url::parse(self.endpoint.trim()).map_err(|e| {
            camel_api::CamelError::Config(format!("endpoint is not a valid URL: {}", e))
        })?;

        Ok(())
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
    fn test_otel_config_logs_disabled() {
        let cfg = OtelConfig::new("http://localhost:4317", "my-service");
        // Mutate directly since it's pub
        let mut cfg = cfg;
        cfg.logs_enabled = false;
        assert!(!cfg.logs_enabled);
    }

    #[test]
    fn test_otel_rejects_malformed_endpoint() {
        let cfg = OtelConfig {
            endpoint: "not-a-url".into(),
            service_name: "myservice".into(),
            ..OtelConfig::new("http://localhost:4317", "myservice")
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_otel_rejects_empty_service_name() {
        let cfg = OtelConfig {
            endpoint: "http://localhost:4317".into(),
            service_name: "".into(),
            ..OtelConfig::new("http://localhost:4317", "myservice")
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_otel_accepts_valid_config() {
        let cfg = OtelConfig {
            endpoint: "http://localhost:4317".into(),
            service_name: "myservice".into(),
            ..OtelConfig::new("http://localhost:4317", "myservice")
        };
        assert!(cfg.validate().is_ok());
    }
}
