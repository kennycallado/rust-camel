//! Tests for the unified tracing subscriber behavior.

use camel_config::{CamelConfig, ComponentsConfig, ObservabilityConfig, OtelCamelConfig};
use camel_core::config::{DetailLevel, OutputFormat, StdoutOutput, TracerConfig, TracerOutputs};

fn make_config_with_stdout_format(format: OutputFormat, otel_enabled: bool) -> CamelConfig {
    CamelConfig {
        routes: vec![],
        watch: false,
        log_level: "INFO".to_string(),
        timeout_ms: 5000,
        components: ComponentsConfig::default(),
        observability: ObservabilityConfig {
            metrics_enabled: false,
            metrics_port: 9090,
            tracer: TracerConfig {
                enabled: true,
                detail_level: DetailLevel::Minimal,
                outputs: TracerOutputs {
                    stdout: StdoutOutput {
                        enabled: true,
                        format,
                    },
                    file: None,
                },
                ..Default::default()
            },
            otel: if otel_enabled {
                Some(OtelCamelConfig {
                    enabled: true,
                    endpoint: "http://localhost:9999".to_string(),
                    service_name: "test".to_string(),
                    log_level: "INFO".to_string(),
                })
            } else {
                None
            },
        },
        supervision: None,
    }
}

#[test]
fn test_configure_context_succeeds_with_json_format() {
    let config = make_config_with_stdout_format(OutputFormat::Json, false);
    let result = CamelConfig::configure_context(&config);
    assert!(
        result.is_ok(),
        "configure_context with JSON format must succeed"
    );
}

#[test]
fn test_configure_context_succeeds_with_plain_format() {
    let config = make_config_with_stdout_format(OutputFormat::Plain, false);
    let result = CamelConfig::configure_context(&config);
    assert!(
        result.is_ok(),
        "configure_context with Plain format must succeed"
    );
}

#[test]
fn test_configure_context_with_otel_enabled_does_not_error() {
    // When OTel is "enabled" in config but camel-otel service is not started,
    // configure_context must still succeed (it just adds layers, doesn't connect).
    let config = make_config_with_stdout_format(OutputFormat::Json, true);
    let result = CamelConfig::configure_context(&config);
    assert!(
        result.is_ok(),
        "configure_context with otel enabled must succeed"
    );
}
