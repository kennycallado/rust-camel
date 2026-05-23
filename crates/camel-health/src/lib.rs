mod server;

#[cfg(test)]
use std::sync::Arc;

use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::get};
use camel_api::{CamelError, HealthChecker, HealthReport, HealthStatus, ServiceHealth};
use tokio::time::Duration;

/// Returns true when the health status should be treated as OK (HTTP 200).
/// `Degraded` is treated as a non-healthy state for strict HTTP probes
/// (returns 503), but callers can inspect the `status` field in the JSON
/// body to distinguish it from fully `Unhealthy`.
fn is_ok_status(status: &HealthStatus) -> bool {
    matches!(status, HealthStatus::Healthy)
}

pub use server::HealthServer;

/// Configuration for health check behavior.
#[derive(Debug, Clone)]
pub struct HealthConfig {
    /// Timeout in milliseconds for each health checker invocation.
    /// Default: 5000 (5 seconds).
    pub check_timeout_ms: u64,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            check_timeout_ms: 5000,
        }
    }
}

/// Execute a synchronous health checker with a timeout.
/// Uses `spawn_blocking` to avoid blocking the async runtime.
async fn run_checker_with_timeout(
    checker: HealthChecker,
    timeout: Duration,
) -> Result<HealthReport, CamelError> {
    let inner = tokio::time::timeout(timeout, async move {
        tokio::task::spawn_blocking(move || checker()).await
    })
    .await
    .map_err(|_| CamelError::ProcessorError("health check timed out".to_string()))?;

    inner.map_err(|e| CamelError::ProcessorError(e.to_string()))
}

/// Build a health router with configurable timeout.
pub fn health_router_with_config(
    readyz_checker: Option<HealthChecker>,
    liveness_checker: Option<HealthChecker>,
    startup_checker: Option<HealthChecker>,
    config: HealthConfig,
) -> Router<()> {
    let timeout = Duration::from_millis(config.check_timeout_ms);

    let healthz = {
        let c = liveness_checker.clone();
        let t = timeout;
        move || {
            let c = c.clone();
            let t = t;
            async move {
                match c.as_ref() {
                    Some(checker) => match run_checker_with_timeout(checker.clone(), t).await {
                        Ok(report) => {
                            if is_ok_status(&report.status) {
                                (StatusCode::OK, Json(report)).into_response()
                            } else {
                                (StatusCode::SERVICE_UNAVAILABLE, Json(report)).into_response()
                            }
                        }
                        Err(e) => {
                            let report = HealthReport {
                                status: HealthStatus::Unhealthy,
                                services: vec![ServiceHealth {
                                    name: format!("error: {e}"),
                                    status: camel_api::ServiceStatus::Failed,
                                }],
                                ..Default::default()
                            };
                            (StatusCode::SERVICE_UNAVAILABLE, Json(report)).into_response()
                        }
                    },
                    None => (StatusCode::OK, Json(HealthReport::default())).into_response(),
                }
            }
        }
    };

    let readyz = {
        let c = readyz_checker.clone();
        let t = timeout;
        move || {
            let c = c.clone();
            let t = t;
            async move {
                match c.as_ref() {
                    Some(checker) => match run_checker_with_timeout(checker.clone(), t).await {
                        Ok(report) => {
                            if is_ok_status(&report.status) {
                                (StatusCode::OK, Json(report)).into_response()
                            } else {
                                (StatusCode::SERVICE_UNAVAILABLE, Json(report)).into_response()
                            }
                        }
                        Err(e) => {
                            let report = HealthReport {
                                status: HealthStatus::Unhealthy,
                                services: vec![ServiceHealth {
                                    name: format!("error: {e}"),
                                    status: camel_api::ServiceStatus::Failed,
                                }],
                                ..Default::default()
                            };
                            (StatusCode::SERVICE_UNAVAILABLE, Json(report)).into_response()
                        }
                    },
                    None => (StatusCode::OK, Json(HealthReport::default())).into_response(),
                }
            }
        }
    };

    let startupz = {
        let c = startup_checker.clone();
        let t = timeout;
        move || {
            let c = c.clone();
            let t = t;
            async move {
                match c.as_ref() {
                    Some(checker) => match run_checker_with_timeout(checker.clone(), t).await {
                        Ok(report) => {
                            if is_ok_status(&report.status) {
                                (StatusCode::OK, Json(report)).into_response()
                            } else {
                                (StatusCode::SERVICE_UNAVAILABLE, Json(report)).into_response()
                            }
                        }
                        Err(e) => {
                            let report = HealthReport {
                                status: HealthStatus::Unhealthy,
                                services: vec![ServiceHealth {
                                    name: format!("error: {e}"),
                                    status: camel_api::ServiceStatus::Failed,
                                }],
                                ..Default::default()
                            };
                            (StatusCode::SERVICE_UNAVAILABLE, Json(report)).into_response()
                        }
                    },
                    None => (StatusCode::OK, Json(HealthReport::default())).into_response(),
                }
            }
        }
    };

    let health = {
        let c = readyz_checker.clone();
        let t = timeout;
        move || {
            let c = c.clone();
            let t = t;
            async move {
                match c.as_ref() {
                    Some(checker) => match run_checker_with_timeout(checker.clone(), t).await {
                        Ok(report) => (StatusCode::OK, Json(report)).into_response(),
                        Err(e) => {
                            let report = HealthReport {
                                status: HealthStatus::Unhealthy,
                                services: vec![ServiceHealth {
                                    name: format!("error: {e}"),
                                    status: camel_api::ServiceStatus::Failed,
                                }],
                                ..Default::default()
                            };
                            (StatusCode::SERVICE_UNAVAILABLE, Json(report)).into_response()
                        }
                    },
                    None => (StatusCode::OK, Json(HealthReport::default())).into_response(),
                }
            }
        }
    };

    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/startupz", get(startupz))
        .route("/health", get(health))
}

/// Build a health router with default timeout (5000ms).
pub fn health_router(
    readyz_checker: Option<HealthChecker>,
    liveness_checker: Option<HealthChecker>,
    startup_checker: Option<HealthChecker>,
) -> Router<()> {
    health_router_with_config(
        readyz_checker,
        liveness_checker,
        startup_checker,
        HealthConfig::default(),
    )
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
        let app = health_router(None, None, None);
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
        let app = health_router(None, Some(checker), None);
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
        let app = health_router(None, None, None);
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
        let app = health_router(Some(checker), None, None);
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
        let app = health_router(Some(checker), None, None);
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
        let app = health_router(None, None, None);
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
        let app = health_router(Some(checker), None, None);
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

    #[tokio::test]
    async fn test_healthz_with_liveness_checker_unhealthy_returns_503() {
        let checker = Arc::new(|| HealthReport {
            status: HealthStatus::Unhealthy,
            ..Default::default()
        }) as HealthChecker;
        let app = health_router(None, Some(checker), None);
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
    async fn test_startupz_no_checker_returns_200() {
        let app = health_router(None, None, None);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/startupz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_startupz_unhealthy_returns_503() {
        let checker = Arc::new(|| HealthReport {
            status: HealthStatus::Unhealthy,
            ..Default::default()
        }) as HealthChecker;
        let app = health_router(None, None, Some(checker));
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
    async fn test_health_check_timeout_aborts_slow_checker() {
        // A slow checker that takes 200ms; wrapped with 50ms timeout → must return 503
        let slow_checker = Arc::new(|| {
            std::thread::sleep(Duration::from_millis(200));
            HealthReport {
                status: HealthStatus::Healthy,
                ..Default::default()
            }
        }) as HealthChecker;

        let config = HealthConfig {
            check_timeout_ms: 50,
        };
        let app = health_router_with_config(Some(slow_checker), None, None, config);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "Slow checker should be timed out and return 503"
        );

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "Unhealthy");
        assert!(
            json["services"][0]["name"]
                .as_str()
                .unwrap()
                .contains("timed out"),
            "Error service name should mention timeout"
        );
    }

    #[tokio::test]
    async fn test_health_check_timeout_default_allows_fast_checker() {
        // Fast checker with default 5000ms timeout → should succeed
        let fast_checker = Arc::new(|| HealthReport {
            status: HealthStatus::Healthy,
            ..Default::default()
        }) as HealthChecker;

        let app =
            health_router_with_config(None, Some(fast_checker), None, HealthConfig::default());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "Fast checker should succeed within default timeout"
        );
    }

    #[tokio::test]
    async fn test_readyz_degraded_returns_503() {
        let checker = Arc::new(|| HealthReport {
            status: HealthStatus::Degraded,
            ..Default::default()
        }) as HealthChecker;
        let app = health_router(Some(checker), None, None);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "Degraded status should return 503"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "Degraded");
    }

    #[tokio::test]
    async fn test_healthz_degraded_returns_503() {
        let checker = Arc::new(|| HealthReport {
            status: HealthStatus::Degraded,
            ..Default::default()
        }) as HealthChecker;
        let app = health_router(None, Some(checker), None);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "Degraded liveness should return 503"
        );
    }
}
