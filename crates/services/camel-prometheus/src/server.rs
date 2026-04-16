use std::net::SocketAddr;
use std::sync::Arc;

use axum::{Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use tokio::net::TcpListener;
use tracing::info;

use camel_api::HealthChecker;

use crate::PrometheusMetrics;

pub struct MetricsServer;

impl MetricsServer {
    pub async fn run(addr: SocketAddr, metrics: Arc<PrometheusMetrics>) {
        Self::run_with_health_checker(addr, metrics, None).await;
    }

    pub async fn run_with_health_checker(
        addr: SocketAddr,
        metrics: Arc<PrometheusMetrics>,
        checker: Option<HealthChecker>,
    ) {
        let health = camel_health::health_router(checker, None, None);
        let app = Router::new()
            .route("/metrics", get(Self::metrics_handler))
            .with_state(metrics)
            .merge(health);

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .unwrap_or_else(|e| panic!("Failed to bind to {}: {}", addr, e));

        info!("Prometheus metrics server listening on {}", addr);
        axum::serve(listener, app).await.unwrap();
    }

    pub async fn run_with_listener(listener: TcpListener, metrics: Arc<PrometheusMetrics>) {
        Self::run_with_listener_and_health_checker(listener, metrics, None).await;
    }

    pub async fn run_with_listener_and_health_checker(
        listener: TcpListener,
        metrics: Arc<PrometheusMetrics>,
        checker: Option<HealthChecker>,
    ) {
        let health = camel_health::health_router(checker, None, None);
        let app = Router::new()
            .route("/metrics", get(Self::metrics_handler))
            .with_state(metrics)
            .merge(health);

        info!(
            "Prometheus metrics server listening on {}",
            listener.local_addr().unwrap()
        );
        axum::serve(listener, app).await.unwrap();
    }

    async fn metrics_handler(State(metrics): State<Arc<PrometheusMetrics>>) -> impl IntoResponse {
        let output = metrics.gather();
        (
            StatusCode::OK,
            [("content-type", "text/plain; version=0.0.4")],
            output,
        )
    }
}

pub struct MetricsResponse {
    pub status: u16,
    pub content_type: String,
    pub body: String,
}

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

        assert_eq!(response.content_type, "text/plain; version=0.0.4");
    }
}
