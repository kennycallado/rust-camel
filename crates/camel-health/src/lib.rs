mod server;

use std::sync::Arc;

use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::get};
use camel_api::{HealthReport, HealthSource, HealthStatus};

pub use server::HealthServer;

fn is_ok_status(status: &HealthStatus) -> bool {
    matches!(status, HealthStatus::Healthy | HealthStatus::Degraded)
}

pub fn health_router(source: Arc<dyn HealthSource>) -> Router<()> {
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
                    let status = s.liveness().await;
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
                    let status = s.readiness().await;
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
                    let status = s.startup().await;
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
                    let report = s.health_report().await;
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
}
