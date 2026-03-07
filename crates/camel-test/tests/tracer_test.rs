use camel_api::Value;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_mock::MockComponent;
use camel_component_timer::TimerComponent;
use camel_core::CamelContext;
use std::time::Duration;

#[tokio::test]
async fn test_tracer_enabled_spans_emitted() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.set_tracing(true);

    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .route_id("test-trace-route")
        .set_header("test-header", Value::String("test-value".into()))
        .to("mock:result")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;

    ctx.stop().await.unwrap();

    let endpoint = mock.get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;
}

#[tokio::test]
async fn test_tracer_disabled_zero_overhead() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    // Tracing disabled by default

    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .route_id("test-no-trace")
        .to("mock:result")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;

    ctx.stop().await.unwrap();

    let endpoint = mock.get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;
}

#[test]
fn test_tracer_file_output_invalid_path_returns_error() {
    use camel_config::config::{CamelConfig, ComponentsConfig, ObservabilityConfig};
    use camel_core::{
        DetailLevel, FileOutput, OutputFormat, StdoutOutput, TracerConfig, TracerOutputs,
    };

    // Build a CamelConfig that enables file tracing with an invalid path
    let config = CamelConfig {
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
                        enabled: false,
                        format: OutputFormat::Json,
                    },
                    file: Some(FileOutput {
                        enabled: true,
                        path: "/invalid/nonexistent/path/trace.log".to_string(),
                        format: OutputFormat::Json,
                    }),
                    opentelemetry: None,
                },
            },
        },
        supervision: None,
    };

    // configure_context should propagate the file-open error
    let result = CamelConfig::configure_context(&config);

    assert!(
        result.is_err(),
        "Expected an error when using invalid trace file path"
    );

    let error_msg = match result {
        Err(camel_api::CamelError::Config(msg)) => msg,
        Err(other) => panic!("Expected CamelError::Config, got {:?}", other),
        Ok(_) => panic!("Expected an error but got Ok"),
    };

    assert!(
        error_msg.contains("trace file")
            || error_msg.contains("/invalid/nonexistent/path/trace.log"),
        "Error message should mention the trace file or path, got: {}",
        error_msg
    );
}
