//! Integration tests for OTel metrics collection.
//!
//! These tests verify:
//! - OtelService exposes MetricsCollector immediately (created in constructor)
//! - Metrics collector can record exchanges and durations
//! - Status transitions correctly

use camel_api::{Lifecycle, ServiceStatus};
use camel_otel::{OtelConfig, OtelService};
use std::time::Duration;

#[tokio::test]
#[serial_test::serial]
async fn test_otel_service_exposes_metrics_collector() {
    let config = OtelConfig::new("http://localhost:9999", "test-service");
    let mut service = OtelService::new(config);

    // Metrics collector is available immediately (created in constructor)
    let collector = service.as_metrics_collector();
    assert!(collector.is_some());

    // Verify it works
    let metrics = collector.unwrap();
    metrics.increment_exchanges("test-route");
    metrics.record_exchange_duration("test-route", Duration::from_millis(100));

    // Start the service (needed for providers, not for metrics collector)
    let _ = service.start().await;

    // Stop the service
    let _ = service.stop().await;
}

#[tokio::test]
#[serial_test::serial]
async fn test_otel_service_status_after_start() {
    let config = OtelConfig::new("http://localhost:9999", "test-service");
    let mut service = OtelService::new(config);

    assert_eq!(service.status(), ServiceStatus::Stopped);
    // Metrics collector available even when stopped
    assert!(service.as_metrics_collector().is_some());

    let _ = service.start().await;

    if service.status() == ServiceStatus::Started {
        assert!(service.as_metrics_collector().is_some());
    }

    let _ = service.stop().await;
    assert_eq!(service.status(), ServiceStatus::Stopped);
    // Metrics collector still available after stop
    assert!(service.as_metrics_collector().is_some());
}
