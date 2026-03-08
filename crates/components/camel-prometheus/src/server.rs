//! HTTP server for exposing Prometheus metrics
//!
//! This module provides an HTTP server that exposes metrics via the `/metrics` endpoint
//! in Prometheus text format.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use tokio::net::TcpListener;
use tracing::info;

use camel_api::{HealthReport, HealthStatus};

use crate::PrometheusMetrics;

/// Type alias for the health checker function
type HealthChecker = Arc<dyn Fn() -> HealthReport + Send + Sync>;

/// Type alias for the combined state
type ServerState = (Arc<PrometheusMetrics>, Option<HealthChecker>);

/// HTTP server for exposing Prometheus metrics
pub struct MetricsServer;

impl MetricsServer {
    /// Starts the metrics server on the given address
    ///
    /// This function runs indefinitely until the server is stopped.
    ///
    /// # Arguments
    /// * `addr` - The socket address to bind to (e.g., "0.0.0.0:9090")
    /// * `metrics` - The PrometheusMetrics instance to expose
    ///
    /// # Panics
    /// Panics if unable to bind to the specified address or if the server fails to start.
    pub async fn run(addr: SocketAddr, metrics: Arc<PrometheusMetrics>) {
        Self::run_with_health_checker(addr, metrics, HealthReport::default).await;
    }

    /// Starts the metrics server with a health checker function
    ///
    /// This function runs indefinitely until the server is stopped.
    ///
    /// # Arguments
    /// * `addr` - The socket address to bind to (e.g., "0.0.0.0:9090")
    /// * `metrics` - The PrometheusMetrics instance to expose
    /// * `health_checker` - Function that returns a HealthReport when called
    ///
    /// # Panics
    /// Panics if unable to bind to the specified address or if the server fails to start.
    pub async fn run_with_health_checker<F>(
        addr: SocketAddr,
        metrics: Arc<PrometheusMetrics>,
        health_checker: F,
    ) where
        F: Fn() -> HealthReport + Send + Sync + 'static,
    {
        let health_checker = Arc::new(health_checker);

        let app = Router::new()
            .route("/metrics", get(Self::metrics_handler))
            .route("/healthz", get(Self::healthz))
            .route("/readyz", get(Self::readyz))
            .route("/health", get(Self::health))
            .with_state((metrics, Some(health_checker)));

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .unwrap_or_else(|e| panic!("Failed to bind to {}: {}", addr, e));

        info!("Prometheus metrics server listening on {}", addr);

        axum::serve(listener, app)
            .await
            .expect("Failed to start metrics server");
    }

    /// Starts the metrics server with an existing listener
    ///
    /// This function runs indefinitely until the server is stopped.
    /// Use this when you need to bind the listener before spawning the server task.
    ///
    /// # Arguments
    /// * `listener` - A bound TCP listener
    /// * `metrics` - The PrometheusMetrics instance to expose
    ///
    /// # Panics
    /// Panics if the server fails to start.
    pub async fn run_with_listener(listener: TcpListener, metrics: Arc<PrometheusMetrics>) {
        let app = Router::new()
            .route("/metrics", get(Self::metrics_handler))
            .route("/healthz", get(Self::healthz))
            .route("/readyz", get(Self::readyz))
            .route("/health", get(Self::health))
            .with_state((metrics, None));

        info!(
            "Prometheus metrics server listening on {:?}",
            listener.local_addr()
        );

        axum::serve(listener, app)
            .await
            .expect("Failed to start metrics server");
    }

    /// Starts the metrics server with an existing listener and health checker
    ///
    /// This function runs indefinitely until the server is stopped.
    /// Use this when you need to bind the listener before spawning the server task.
    ///
    /// # Arguments
    /// * `listener` - A bound TCP listener
    /// * `metrics` - The PrometheusMetrics instance to expose
    /// * `health_checker` - Function that returns a HealthReport when called
    ///
    /// # Panics
    /// Panics if the server fails to start.
    pub async fn run_with_listener_and_health_checker(
        listener: TcpListener,
        metrics: Arc<PrometheusMetrics>,
        health_checker: HealthChecker,
    ) {
        let app = Router::new()
            .route("/metrics", get(Self::metrics_handler))
            .route("/healthz", get(Self::healthz))
            .route("/readyz", get(Self::readyz))
            .route("/health", get(Self::health))
            .with_state((metrics, Some(health_checker)));

        info!(
            "Prometheus metrics server listening on {:?}",
            listener.local_addr()
        );

        axum::serve(listener, app)
            .await
            .expect("Failed to start metrics server");
    }

    /// Handler for GET /healthz - Kubernetes liveness probe
    ///
    /// Returns 200 OK if the service is running.
    async fn healthz() -> StatusCode {
        StatusCode::OK
    }

    /// Handler for GET /readyz - Kubernetes readiness probe
    ///
    /// Returns 200 OK if ready to serve traffic, 503 otherwise.
    async fn readyz(State((_, checker)): State<ServerState>) -> impl IntoResponse {
        match checker {
            Some(checker) => {
                let report = checker();
                let status = if report.status == HealthStatus::Healthy {
                    StatusCode::OK
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                };
                (status, Json(report))
            }
            None => (StatusCode::OK, Json(HealthReport::default())),
        }
    }

    /// Handler for GET /health - Detailed health report
    ///
    /// Returns JSON with detailed service health status.
    async fn health(State((_, checker)): State<ServerState>) -> impl IntoResponse {
        match checker {
            Some(checker) => {
                let report = checker();
                (StatusCode::OK, Json(report))
            }
            None => (StatusCode::OK, Json(HealthReport::default())),
        }
    }

    /// Handler for GET /metrics - Prometheus metrics endpoint
    async fn metrics_handler(State((metrics, _)): State<ServerState>) -> impl IntoResponse {
        let output = metrics.gather();
        (
            StatusCode::OK,
            [("content-type", "text/plain; version=0.0.4")],
            output,
        )
    }
}

/// Response type for metrics endpoint (used for testing)
pub struct MetricsResponse {
    pub status: u16,
    pub content_type: String,
    pub body: String,
}

/// Handles GET /metrics requests
///
/// Returns metrics in Prometheus text format with appropriate content-type header.
/// This is a testable version of the handler that returns a struct.
pub async fn metrics_handler(metrics: Arc<PrometheusMetrics>) -> MetricsResponse {
    let output = metrics.gather();
    MetricsResponse {
        status: 200,
        content_type: "text/plain; version=0.0.4".to_string(),
        body: output,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use camel_api::metrics::MetricsCollector;
    use camel_api::{ServiceHealth, ServiceStatus};
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_metrics_handler_returns_prometheus_format() {
        let metrics = Arc::new(PrometheusMetrics::new());
        metrics.increment_exchanges("test-route");

        let response = metrics_handler(metrics).await;

        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "text/plain; version=0.0.4");
        assert!(response.body.contains("camel_exchanges_total"));
        assert!(response.body.contains("test-route"));
    }

    #[tokio::test]
    async fn test_metrics_handler_content_type() {
        let metrics = Arc::new(PrometheusMetrics::new());

        let response = metrics_handler(metrics).await;

        // Prometheus requires this specific content-type
        assert_eq!(response.content_type, "text/plain; version=0.0.4");
    }

    #[tokio::test]
    async fn test_healthz_returns_200_ok() {
        let health_checker = Arc::new(HealthReport::default) as HealthChecker;
        let state: ServerState = (Arc::new(PrometheusMetrics::new()), Some(health_checker));
        let app = Router::new()
            .route("/healthz", get(MetricsServer::healthz))
            .with_state(state);

        let response = app
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_readyz_returns_200_when_healthy() {
        let health_checker =
            Arc::new(|| HealthReport {
                status: HealthStatus::Healthy,
                ..Default::default()
            }) as HealthChecker;
        let state: ServerState = (Arc::new(PrometheusMetrics::new()), Some(health_checker));
        let app = Router::new()
            .route("/readyz", get(MetricsServer::readyz))
            .with_state(state);

        let response = app
            .oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_readyz_returns_503_when_unhealthy() {
        let health_checker =
            Arc::new(|| HealthReport {
                status: HealthStatus::Unhealthy,
                ..Default::default()
            }) as HealthChecker;
        let state: ServerState = (Arc::new(PrometheusMetrics::new()), Some(health_checker));
        let app = Router::new()
            .route("/readyz", get(MetricsServer::readyz))
            .with_state(state);

        let response = app
            .oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_health_returns_json_health_report() {
        let health_checker =
            Arc::new(|| HealthReport {
                status: HealthStatus::Healthy,
                services: vec![ServiceHealth {
                    name: "test-service".to_string(),
                    status: ServiceStatus::Started,
                }],
                ..Default::default()
            }) as HealthChecker;
        let state: ServerState = (Arc::new(PrometheusMetrics::new()), Some(health_checker));
        let app = Router::new()
            .route("/health", get(MetricsServer::health))
            .with_state(state);

        let response = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["status"], "Healthy");
        assert!(json["services"].is_array());
    }
}
