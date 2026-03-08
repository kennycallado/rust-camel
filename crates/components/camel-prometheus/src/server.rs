//! HTTP server for exposing Prometheus metrics
//!
//! This module provides an HTTP server that exposes metrics via the `/metrics` endpoint
//! in Prometheus text format.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use tokio::net::TcpListener;
use tracing::info;

use crate::PrometheusMetrics;

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
        let app = Router::new()
            .route("/metrics", get(metrics_handler_axum))
            .with_state(metrics);

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
            .route("/metrics", get(metrics_handler_axum))
            .with_state(metrics);

        info!(
            "Prometheus metrics server listening on {:?}",
            listener.local_addr()
        );

        axum::serve(listener, app)
            .await
            .expect("Failed to start metrics server");
    }
}

/// Axum handler for GET /metrics requests
async fn metrics_handler_axum(State(metrics): State<Arc<PrometheusMetrics>>) -> impl IntoResponse {
    let output = metrics.gather();
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4")],
        output,
    )
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
    use camel_api::metrics::MetricsCollector;

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
}
