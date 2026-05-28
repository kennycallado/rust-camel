use std::net::SocketAddr;
use std::sync::Arc;

use axum::{Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tracing::info;

use async_trait::async_trait;
use camel_api::{CamelError, HealthSource, HealthStatus};

use crate::PrometheusMetrics;

pub struct MetricsServer;

impl MetricsServer {
    pub async fn run(addr: SocketAddr, metrics: Arc<PrometheusMetrics>) -> Result<(), CamelError> {
        Self::run_with_health_source(addr, metrics, None).await
    }

    pub async fn run_with_health_source(
        addr: SocketAddr,
        metrics: Arc<PrometheusMetrics>,
        source: Option<Arc<dyn HealthSource>>,
    ) -> Result<(), CamelError> {
        let source =
            source.unwrap_or_else(|| Arc::new(DefaultHealthSource) as Arc<dyn HealthSource>);
        let health = camel_health::health_router(source);
        let app = Router::new()
            .route("/metrics", get(Self::metrics_handler))
            .with_state(metrics)
            .merge(health);

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| CamelError::Io(format!("Failed to bind to {addr}: {e}")))?;

        info!("Prometheus metrics server listening on {}", addr);
        axum::serve(listener, app)
            .await
            .map_err(|e| CamelError::Io(format!("Prometheus server failed: {e}")))
    }

    pub async fn run_with_listener(
        listener: TcpListener,
        metrics: Arc<PrometheusMetrics>,
    ) -> Result<(), CamelError> {
        Self::run_with_listener_and_health_source(listener, metrics, None).await
    }

    pub async fn run_with_listener_and_health_source(
        listener: TcpListener,
        metrics: Arc<PrometheusMetrics>,
        source: Option<Arc<dyn HealthSource>>,
    ) -> Result<(), CamelError> {
        let source =
            source.unwrap_or_else(|| Arc::new(DefaultHealthSource) as Arc<dyn HealthSource>);
        let health = camel_health::health_router(source);
        let app = Router::new()
            .route("/metrics", get(Self::metrics_handler))
            .with_state(metrics)
            .merge(health);

        info!(
            "Prometheus metrics server listening on {}",
            listener.local_addr().unwrap() // allow-unwrap
        );
        axum::serve(listener, app)
            .await
            .map_err(|e| CamelError::Io(format!("Prometheus server failed: {e}")))
    }

    pub async fn run_with_listener_and_health_source_with_shutdown(
        listener: TcpListener,
        metrics: Arc<PrometheusMetrics>,
        source: Option<Arc<dyn HealthSource>>,
        shutdown: oneshot::Receiver<()>,
    ) -> Result<(), CamelError> {
        let source =
            source.unwrap_or_else(|| Arc::new(DefaultHealthSource) as Arc<dyn HealthSource>);
        let health = camel_health::health_router(source);
        let app = Router::new()
            .route("/metrics", get(Self::metrics_handler))
            .with_state(metrics)
            .merge(health);

        info!(
            "Prometheus metrics server listening on {}",
            listener.local_addr().unwrap() // allow-unwrap
        );

        #[cfg(test)]
        let shutdown_signal_count = test_graceful_shutdown_signal_count();

        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown.await;
                #[cfg(test)]
                shutdown_signal_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
            .await
            .map_err(|e| CamelError::Io(format!("Prometheus server failed: {e}")))
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

struct DefaultHealthSource;

#[async_trait]
impl HealthSource for DefaultHealthSource {
    async fn liveness(&self) -> HealthStatus {
        HealthStatus::Healthy
    }

    async fn readiness(&self) -> HealthStatus {
        HealthStatus::Healthy
    }

    async fn startup(&self) -> HealthStatus {
        HealthStatus::Healthy
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
static GRACEFUL_SHUTDOWN_SIGNAL_COUNT: std::sync::OnceLock<
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
> = std::sync::OnceLock::new();

#[cfg(test)]
pub fn test_graceful_shutdown_signal_count() -> std::sync::Arc<std::sync::atomic::AtomicUsize> {
    std::sync::Arc::clone(
        GRACEFUL_SHUTDOWN_SIGNAL_COUNT
            .get_or_init(|| std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0))),
    )
}

#[cfg(test)]
pub fn test_reset_graceful_shutdown_observability() {
    test_graceful_shutdown_signal_count().store(0, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::metrics::MetricsCollector;
    use tokio::time::{Duration, sleep};

    struct MockHealthSource {
        readiness: HealthStatus,
    }

    #[async_trait]
    impl HealthSource for MockHealthSource {
        async fn liveness(&self) -> HealthStatus {
            HealthStatus::Healthy
        }

        async fn readiness(&self) -> HealthStatus {
            self.readiness
        }

        async fn startup(&self) -> HealthStatus {
            HealthStatus::Healthy
        }
    }

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

    #[tokio::test]
    async fn test_run_with_listener_serves_metrics_and_health() {
        let metrics = Arc::new(PrometheusMetrics::new());
        metrics.increment_exchanges("route-http");

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let source = Arc::new(MockHealthSource {
            readiness: HealthStatus::Healthy,
        });

        let handle = tokio::spawn(MetricsServer::run_with_listener_and_health_source(
            listener,
            Arc::clone(&metrics),
            Some(source),
        ));

        sleep(Duration::from_millis(50)).await;

        let client = reqwest::Client::new();
        let metrics_resp = client
            .get(format!("http://{}/metrics", addr))
            .send()
            .await
            .unwrap();
        assert_eq!(metrics_resp.status().as_u16(), 200);
        assert_eq!(
            metrics_resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "text/plain; version=0.0.4"
        );
        let body = metrics_resp.text().await.unwrap();
        assert!(body.contains("camel_exchanges_total"));
        assert!(body.contains("route-http"));

        let health_resp = client
            .get(format!("http://{}/health", addr))
            .send()
            .await
            .unwrap();
        assert_eq!(health_resp.status().as_u16(), 200);

        let not_found = client
            .get(format!("http://{}/does-not-exist", addr))
            .send()
            .await
            .unwrap();
        assert_eq!(not_found.status().as_u16(), 404);

        handle.abort();
    }

    #[tokio::test]
    async fn test_graceful_shutdown_signal_observed() {
        let metrics = Arc::new(PrometheusMetrics::new());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

        let handle = tokio::spawn(
            MetricsServer::run_with_listener_and_health_source_with_shutdown(
                listener,
                metrics,
                None,
                shutdown_rx,
            ),
        );

        sleep(Duration::from_millis(30)).await;
        let _ = shutdown_tx.send(());

        let join = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("server did not shutdown in time")
            .expect("join should succeed");
        assert!(join.is_ok(), "server run should return Ok, got: {join:?}");
    }

    #[tokio::test]
    async fn test_run_returns_err_on_bind_failure() {
        let metrics = Arc::new(PrometheusMetrics::new());
        let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = occupied.local_addr().unwrap();

        let result = MetricsServer::run(addr, metrics).await;
        assert!(result.is_err(), "expected bind failure, got {result:?}");
    }
}
