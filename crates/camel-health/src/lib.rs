mod server;

#[cfg(test)]
use std::sync::Arc;

use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::get};
use camel_api::{HealthChecker, HealthReport, HealthStatus};

pub use server::HealthServer;

pub fn health_router(checker: Option<HealthChecker>) -> Router<()> {
    let healthz = {
        let c = checker.clone();
        move || async move {
            let _ = &c;
            StatusCode::OK
        }
    };

    let readyz = {
        let c = checker.clone();
        move || {
            let c = c.clone();
            async move {
                match c.as_ref() {
                    Some(checker) => {
                        let report = checker();
                        if report.status == HealthStatus::Healthy {
                            (StatusCode::OK, Json(report)).into_response()
                        } else {
                            (StatusCode::SERVICE_UNAVAILABLE, Json(report)).into_response()
                        }
                    }
                    None => (StatusCode::OK, Json(HealthReport::default())).into_response(),
                }
            }
        }
    };

    let health = {
        let c = checker.clone();
        move || {
            let c = c.clone();
            async move {
                match c.as_ref() {
                    Some(checker) => {
                        let report = checker();
                        (StatusCode::OK, Json(report)).into_response()
                    }
                    None => (StatusCode::OK, Json(HealthReport::default())).into_response(),
                }
            }
        }
    };

    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/health", get(health))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use camel_api::{ServiceHealth, ServiceStatus};
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_healthz_returns_200() {
        let app = health_router(None);
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
    async fn test_healthz_with_checker_returns_200() {
        let checker = Arc::new(HealthReport::default) as HealthChecker;
        let app = health_router(Some(checker));
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
    async fn test_readyz_no_checker_returns_200() {
        let app = health_router(None);
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
    async fn test_readyz_healthy_returns_200() {
        let checker = Arc::new(|| HealthReport {
            status: HealthStatus::Healthy,
            ..Default::default()
        }) as HealthChecker;
        let app = health_router(Some(checker));
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
    async fn test_readyz_unhealthy_returns_503() {
        let checker = Arc::new(|| HealthReport {
            status: HealthStatus::Unhealthy,
            ..Default::default()
        }) as HealthChecker;
        let app = health_router(Some(checker));
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
    async fn test_health_no_checker_returns_200_with_default_report() {
        let app = health_router(None);
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
    async fn test_health_with_checker_returns_json_report() {
        let checker = Arc::new(|| HealthReport {
            status: HealthStatus::Healthy,
            services: vec![ServiceHealth {
                name: "test-service".to_string(),
                status: ServiceStatus::Started,
            }],
            ..Default::default()
        }) as HealthChecker;
        let app = health_router(Some(checker));
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
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "Healthy");
        assert_eq!(json["services"][0]["name"], "test-service");
    }
}
