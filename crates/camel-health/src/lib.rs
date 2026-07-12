mod server;

use std::sync::Arc;
use std::time::Duration;

use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::get};
use camel_api::{HealthReport, HealthSource, HealthStatus, ServiceHealth, ServiceStatus};

pub use server::HealthServer;

/// Default handler-level probe timeout (6s, strictly > internal 5s registry tick).
pub const DEFAULT_HANDLER_TIMEOUT: Duration = Duration::from_secs(6);

fn is_ok_status(status: &HealthStatus) -> bool {
    matches!(status, HealthStatus::Healthy | HealthStatus::Degraded)
}

/// Build the health router with the default handler timeout (6s).
pub fn health_router(source: Arc<dyn HealthSource>) -> Router<()> {
    health_router_with_timeout(source, DEFAULT_HANDLER_TIMEOUT)
}

/// R4-L11: health router with explicit handler-level probe timeout.
///
/// Each probe `.await` is wrapped in `tokio::time::timeout`. On timeout the
/// handler returns a fail-closed response (503 for `/healthz`, `/readyz`,
/// `/startupz`; `Unhealthy` status for `/health`). `timeout` must be > 0.
pub fn health_router_with_timeout(
    source: Arc<dyn HealthSource>,
    handler_timeout: Duration,
) -> Router<()> {
    assert!(
        !handler_timeout.is_zero(),
        "health handler timeout must be > 0"
    );

    let s_liveness = Arc::clone(&source);
    let s_readiness = Arc::clone(&source);
    let s_startup = Arc::clone(&source);
    let s_health = Arc::clone(&source);

    Router::new()
        .route(
            "/healthz",
            get(move || {
                let s = Arc::clone(&s_liveness);
                async move {
                    let status = match tokio::time::timeout(handler_timeout, s.liveness()).await {
                        Ok(st) => st,
                        Err(_) => HealthStatus::Unhealthy,
                    };
                    let report = HealthReport {
                        status,
                        ..Default::default()
                    };
                    if is_ok_status(&status) {
                        (StatusCode::OK, Json(report)).into_response()
                    } else {
                        (StatusCode::SERVICE_UNAVAILABLE, Json(report)).into_response()
                    }
                }
            }),
        )
        .route(
            "/readyz",
            get(move || {
                let s = Arc::clone(&s_readiness);
                async move {
                    let status = match tokio::time::timeout(handler_timeout, s.readiness()).await {
                        Ok(st) => st,
                        Err(_) => HealthStatus::Unhealthy,
                    };
                    let report = HealthReport {
                        status,
                        ..Default::default()
                    };
                    if is_ok_status(&status) {
                        (StatusCode::OK, Json(report)).into_response()
                    } else {
                        (StatusCode::SERVICE_UNAVAILABLE, Json(report)).into_response()
                    }
                }
            }),
        )
        .route(
            "/startupz",
            get(move || {
                let s = Arc::clone(&s_startup);
                async move {
                    let status = match tokio::time::timeout(handler_timeout, s.startup()).await {
                        Ok(st) => st,
                        Err(_) => HealthStatus::Unhealthy,
                    };
                    let report = HealthReport {
                        status,
                        ..Default::default()
                    };
                    if is_ok_status(&status) {
                        (StatusCode::OK, Json(report)).into_response()
                    } else {
                        (StatusCode::SERVICE_UNAVAILABLE, Json(report)).into_response()
                    }
                }
            }),
        )
        .route(
            "/health",
            get(move || {
                let s = Arc::clone(&s_health);
                async move {
                    let report =
                        match tokio::time::timeout(handler_timeout, s.health_report()).await {
                            Ok(r) => r,
                            Err(_) => HealthReport {
                                status: HealthStatus::Unhealthy,
                                services: vec![ServiceHealth {
                                    name: "probe-timeout".to_string(),
                                    status: ServiceStatus::Failed,
                                    message: Some(
                                        "health_report probe exceeded handler timeout".to_string(),
                                    ),
                                }],
                                timestamp: chrono::Utc::now(),
                            },
                        };
                    (StatusCode::OK, Json(report)).into_response()
                }
            }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use camel_api::{ServiceHealth, ServiceStatus};
    use tower::ServiceExt;

    struct MockHealthSource {
        liveness: HealthStatus,
        readiness: HealthStatus,
        startup: HealthStatus,
        health_report: HealthReport,
    }

    #[async_trait]
    impl HealthSource for MockHealthSource {
        async fn liveness(&self) -> HealthStatus {
            self.liveness
        }

        async fn readiness(&self) -> HealthStatus {
            self.readiness
        }

        async fn startup(&self) -> HealthStatus {
            self.startup
        }

        async fn health_report(&self) -> HealthReport {
            self.health_report.clone()
        }
    }

    fn mock_source(
        liveness: HealthStatus,
        readiness: HealthStatus,
        startup: HealthStatus,
        health_report: HealthReport,
    ) -> Arc<dyn HealthSource> {
        Arc::new(MockHealthSource {
            liveness,
            readiness,
            startup,
            health_report,
        })
    }

    #[tokio::test]
    async fn test_healthz_returns_200() {
        let app = health_router(mock_source(
            HealthStatus::Healthy,
            HealthStatus::Healthy,
            HealthStatus::Healthy,
            HealthReport::default(),
        ));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_readyz_unhealthy_returns_503() {
        let app = health_router(mock_source(
            HealthStatus::Healthy,
            HealthStatus::Unhealthy,
            HealthStatus::Healthy,
            HealthReport::default(),
        ));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_startupz_unhealthy_returns_503() {
        let app = health_router(mock_source(
            HealthStatus::Healthy,
            HealthStatus::Healthy,
            HealthStatus::Unhealthy,
            HealthReport::default(),
        ));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/startupz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_health_always_returns_200() {
        let app = health_router(mock_source(
            HealthStatus::Healthy,
            HealthStatus::Unhealthy,
            HealthStatus::Healthy,
            HealthReport::default(),
        ));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_readyz_degraded_returns_200() {
        let app = health_router(mock_source(
            HealthStatus::Healthy,
            HealthStatus::Degraded,
            HealthStatus::Healthy,
            HealthReport::default(),
        ));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_health_returns_full_report_with_services() {
        let app = health_router(mock_source(
            HealthStatus::Healthy,
            HealthStatus::Healthy,
            HealthStatus::Healthy,
            HealthReport {
                status: HealthStatus::Degraded,
                services: vec![ServiceHealth {
                    name: "db".to_string(),
                    status: ServiceStatus::Started,
                    message: Some("slow".to_string()),
                }],
                ..Default::default()
            },
        ));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let report: HealthReport = serde_json::from_slice(&body).unwrap();
        assert_eq!(report.status, HealthStatus::Degraded);
        assert_eq!(report.services.len(), 1);
        assert_eq!(report.services[0].name, "db");
    }

    // --- R4-L11 timeout tests ---

    /// A HealthSource whose probes never resolve (used to trigger timeouts).
    struct HangingHealthSource;

    #[async_trait]
    impl HealthSource for HangingHealthSource {
        async fn liveness(&self) -> HealthStatus {
            std::future::pending::<HealthStatus>().await
        }

        async fn readiness(&self) -> HealthStatus {
            std::future::pending::<HealthStatus>().await
        }

        async fn startup(&self) -> HealthStatus {
            std::future::pending::<HealthStatus>().await
        }

        async fn health_report(&self) -> HealthReport {
            std::future::pending::<HealthReport>().await
        }
    }

    #[tokio::test]
    async fn healthz_timeout_returns_503() {
        let source: Arc<dyn HealthSource> = Arc::new(HangingHealthSource);
        let app = health_router_with_timeout(source, Duration::from_millis(100));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn readyz_timeout_returns_503() {
        let source: Arc<dyn HealthSource> = Arc::new(HangingHealthSource);
        let app = health_router_with_timeout(source, Duration::from_millis(100));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn startupz_timeout_returns_503() {
        let source: Arc<dyn HealthSource> = Arc::new(HangingHealthSource);
        let app = health_router_with_timeout(source, Duration::from_millis(100));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/startupz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn health_timeout_returns_200_with_unhealthy_report() {
        let source: Arc<dyn HealthSource> = Arc::new(HangingHealthSource);
        let app = health_router_with_timeout(source, Duration::from_millis(100));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // /health always returns 200, but the report status is Unhealthy on timeout
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let report: HealthReport = serde_json::from_slice(&body).unwrap();
        assert_eq!(report.status, HealthStatus::Unhealthy);
        assert!(report.services.iter().any(|s| s.name == "probe-timeout"));
    }

    #[tokio::test]
    async fn fast_probe_returns_200_with_timeout_wrapper() {
        let source = mock_source(
            HealthStatus::Healthy,
            HealthStatus::Healthy,
            HealthStatus::Healthy,
            HealthReport::default(),
        );
        let app = health_router_with_timeout(source, Duration::from_secs(5));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    #[should_panic(expected = "health handler timeout must be > 0")]
    async fn zero_timeout_rejected() {
        let source = mock_source(
            HealthStatus::Healthy,
            HealthStatus::Healthy,
            HealthStatus::Healthy,
            HealthReport::default(),
        );
        let _ = health_router_with_timeout(source, Duration::ZERO);
    }
}
