use camel_api::{Lifecycle, ServiceStatus};
use camel_otel::{OtelConfig, OtelSampler, OtelService};

#[tokio::test]
#[serial_test::serial]
async fn test_full_lifecycle_creates_and_cleans_providers() {
    let config = OtelConfig::new("http://localhost:9999", "test-lifecycle");
    let mut service = OtelService::new(config);

    assert_eq!(service.status(), ServiceStatus::Stopped);

    service.start().await.unwrap();
    assert_eq!(service.status(), ServiceStatus::Started);

    service.stop().await.unwrap();
    assert_eq!(service.status(), ServiceStatus::Stopped);
}

#[tokio::test]
#[serial_test::serial]
async fn test_stop_when_already_stopped_is_idempotent() {
    let mut service = OtelService::with_defaults();

    service.start().await.unwrap();
    service.stop().await.unwrap();
    service.stop().await.unwrap();

    assert_eq!(service.status(), ServiceStatus::Stopped);
}

#[tokio::test]
#[serial_test::serial]
async fn test_start_failure_does_not_leave_partial_state() {
    let config = OtelConfig::new("http://localhost:9999", "test-partial")
        .with_sampler(OtelSampler::TraceIdRatioBased(2.0));
    let mut service = OtelService::new(config);

    let result = service.start().await;
    assert!(result.is_err());
    assert_eq!(service.status(), ServiceStatus::Failed);

    service.stop().await.unwrap();
    assert_eq!(service.status(), ServiceStatus::Stopped);
}

#[tokio::test]
#[serial_test::serial]
async fn test_stop_flushes_logger_provider() {
    let config = OtelConfig::new("http://localhost:9999", "test-logger-flush");
    let mut service = OtelService::new(config);

    service.init_logger_provider().unwrap();
    service.start().await.unwrap();
    service.stop().await.unwrap();

    assert_eq!(service.status(), ServiceStatus::Stopped);

    service.start().await.unwrap();
    assert_eq!(service.status(), ServiceStatus::Started);
    service.stop().await.unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn test_shutdown_logger_provider_removes_field() {
    let config = OtelConfig::new("http://localhost:9999", "test-logger-shutdown");
    let mut service = OtelService::new(config);

    service.init_logger_provider().unwrap();
    service.start().await.unwrap();
    service.stop().await.unwrap();
    service.shutdown_logger_provider();

    service.init_logger_provider().unwrap();
    service.start().await.unwrap();
    assert_eq!(service.status(), ServiceStatus::Started);
    service.stop().await.unwrap();
}
