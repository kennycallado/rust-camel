use camel_api::Value;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_test::CamelTestContext;
use std::time::Duration;

#[tokio::test]
async fn tracer_enabled_spans_emitted() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;
    h.ctx().lock().await.set_tracing(true);

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .route_id("test-trace-route")
        .set_header("test-header", Value::String("test-value".into()))
        .to("mock:result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(Duration::from_millis(200)).await;

    h.stop().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;
}

#[tokio::test]
async fn tracer_disabled_zero_overhead() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;
    // Tracing disabled by default

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .route_id("test-no-trace")
        .to("mock:result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(Duration::from_millis(200)).await;

    h.stop().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;
}

#[tokio::test]
async fn tracer_file_output_invalid_path_returns_error() {
    use camel_config::config::{CamelConfig, ComponentsConfig, ObservabilityConfig};
    use camel_core::{
        DetailLevel, FileOutput, OutputFormat, StdoutOutput, TracerConfig, TracerOutputs,
    };

    // Build a CamelConfig that enables file tracing with an invalid path
    let config = CamelConfig {
        routes: vec![],
        watch: false,
        runtime_journal: None,
        log_level: "INFO".to_string(),
        timeout_ms: 5000,
        drain_timeout_ms: 10_000,
        watch_debounce_ms: 300,
        components: ComponentsConfig::default(),
        observability: ObservabilityConfig {
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
                },
                ..Default::default()
            },
            otel: None,
            prometheus: None,
        },
        supervision: None,
    };

    // configure_context should propagate the file-open error
    let result = CamelConfig::configure_context(&config).await;

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
