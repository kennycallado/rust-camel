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

/// Fail-closed report returned when a probe exceeds the handler timeout.
fn probe_timeout_report(probe: &str) -> HealthReport {
    HealthReport {
        status: HealthStatus::Unhealthy,
        services: vec![ServiceHealth {
            name: "probe-timeout".to_string(),
            status: ServiceStatus::Failed,
            message: Some(format!("{probe} probe exceeded handler timeout")),
        }],
        timestamp: chrono::Utc::now(),
    }
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
                    let report =
                        match tokio::time::timeout(handler_timeout, s.liveness_report()).await {
                            Ok(r) => r,
                            Err(_) => probe_timeout_report("liveness"),
                        };
                    if is_ok_status(&report.status) {
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
                    let report =
                        match tokio::time::timeout(handler_timeout, s.readiness_report()).await {
                            Ok(r) => r,
                            Err(_) => probe_timeout_report("readiness"),
                        };
                    if is_ok_status(&report.status) {
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
                    let report =
                        match tokio::time::timeout(handler_timeout, s.startup_report()).await {
                            Ok(r) => r,
                            Err(_) => probe_timeout_report("startup"),
                        };
                    if is_ok_status(&report.status) {
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
        liveness_report: Option<HealthReport>,
        readiness_report: Option<HealthReport>,
        startup_report: Option<HealthReport>,
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

        async fn liveness_report(&self) -> HealthReport {
            self.liveness_report.clone().unwrap_or(HealthReport {
                status: self.liveness,
                ..Default::default()
            })
        }

        async fn readiness_report(&self) -> HealthReport {
            self.readiness_report.clone().unwrap_or(HealthReport {
                status: self.readiness,
                ..Default::default()
            })
        }

        async fn startup_report(&self) -> HealthReport {
            self.startup_report.clone().unwrap_or(HealthReport {
                status: self.startup,
                ..Default::default()
            })
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
            liveness_report: None,
            readiness_report: None,
            startup_report: None,
        })
    }

    /// A source whose report methods carry per-service detail. The bare status
    /// methods mirror the derived report status for the matching probe.
    fn detail_source(
        liveness_report: Option<HealthReport>,
        readiness_report: Option<HealthReport>,
        startup_report: Option<HealthReport>,
    ) -> Arc<dyn HealthSource> {
        fn status_of(report: &Option<HealthReport>) -> HealthStatus {
            report
                .as_ref()
                .map(|r| r.status)
                .unwrap_or(HealthStatus::Healthy)
        }
        Arc::new(MockHealthSource {
            liveness: status_of(&liveness_report),
            readiness: status_of(&readiness_report),
            startup: status_of(&startup_report),
            health_report: HealthReport::default(),
            liveness_report,
            readiness_report,
            startup_report,
        })
    }

    fn service(name: &str, status: ServiceStatus, message: Option<&str>) -> ServiceHealth {
        ServiceHealth {
            name: name.to_string(),
            status,
            message: message.map(str::to_string),
        }
    }

    fn report(status: HealthStatus, services: Vec<ServiceHealth>) -> HealthReport {
        HealthReport {
            status,
            services,
            timestamp: chrono::Utc::now(),
        }
    }

    async fn get(app: Router<()>, uri: &str) -> axum::response::Response {
        app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn body_json(response: axum::response::Response) -> serde_json::Value {
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    fn key_set(value: &serde_json::Value) -> Vec<String> {
        let mut keys: Vec<String> = value.as_object().expect("object").keys().cloned().collect();
        keys.sort();
        keys
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

        async fn liveness_report(&self) -> HealthReport {
            std::future::pending::<HealthReport>().await
        }

        async fn readiness_report(&self) -> HealthReport {
            std::future::pending::<HealthReport>().await
        }

        async fn startup_report(&self) -> HealthReport {
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

    // --- Per-service probe detail tests ---

    #[tokio::test]
    async fn readyz_degraded_names_degraded_service() {
        let app = health_router(detail_source(
            None,
            Some(report(
                HealthStatus::Degraded,
                vec![service("broker", ServiceStatus::Started, Some("stale"))],
            )),
            None,
        ));

        let response = get(app, "/readyz").await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["status"], "Degraded");
        assert_eq!(body["services"][0]["name"], "broker");
        assert_eq!(body["services"][0]["message"], "stale");
    }

    #[tokio::test]
    async fn healthz_unhealthy_names_failed_service() {
        let app = health_router(detail_source(
            Some(report(
                HealthStatus::Unhealthy,
                vec![service("db", ServiceStatus::Failed, Some("down"))],
            )),
            None,
            None,
        ));

        let response = get(app, "/healthz").await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = body_json(response).await;
        assert_eq!(body["status"], "Unhealthy");
        assert_eq!(body["services"][0]["name"], "db");
    }

    #[tokio::test]
    async fn probe_timeout_synthesizes_entry_on_all_three_probes() {
        let source: Arc<dyn HealthSource> = Arc::new(HangingHealthSource);
        for (uri, probe) in [
            ("/healthz", "liveness"),
            ("/readyz", "readiness"),
            ("/startupz", "startup"),
        ] {
            let app = health_router_with_timeout(Arc::clone(&source), Duration::from_millis(50));
            let response = get(app, uri).await;
            assert_eq!(
                response.status(),
                StatusCode::SERVICE_UNAVAILABLE,
                "{uri} must be 503"
            );
            let body = body_json(response).await;
            assert_eq!(body["status"], "Unhealthy", "{uri} status");
            let services = body["services"].as_array().expect("services array");
            assert_eq!(services.len(), 1, "{uri} entry count");
            assert_eq!(services[0]["name"], "probe-timeout", "{uri} entry name");
            let message = services[0]["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(probe),
                "{uri} message {message:?} must mention {probe}"
            );
        }
    }

    #[tokio::test]
    async fn ordinary_unhealthy_has_no_probe_timeout_entry() {
        let app = health_router(detail_source(
            None,
            Some(report(
                HealthStatus::Unhealthy,
                vec![service("broker", ServiceStatus::Failed, Some("down"))],
            )),
            None,
        ));

        let response = get(app, "/readyz").await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = body_json(response).await;
        let services = body["services"].as_array().expect("services array");
        assert_eq!(services.len(), 1);
        assert_eq!(services[0]["name"], "broker");
        assert!(
            !services.iter().any(|s| s["name"] == "probe-timeout"),
            "ordinary failure must not synthesize a timeout entry"
        );
    }

    #[tokio::test]
    async fn healthy_detail_capable_body_lists_services() {
        let app = health_router(detail_source(
            None,
            Some(report(
                HealthStatus::Healthy,
                vec![
                    service("alpha", ServiceStatus::Started, None),
                    service("beta", ServiceStatus::Started, Some("ok")),
                ],
            )),
            None,
        ));

        let response = get(app, "/readyz").await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["status"], "Healthy");
        let services = body["services"].as_array().expect("services array");
        assert_eq!(services.len(), 2);
        assert_eq!(services[0]["name"], "alpha");
        assert_eq!(services[1]["name"], "beta");
    }

    #[tokio::test]
    async fn serialized_keys_exact() {
        let app = health_router(detail_source(
            None,
            Some(report(
                HealthStatus::Degraded,
                vec![service("broker", ServiceStatus::Started, None)],
            )),
            None,
        ));

        let response = get(app, "/readyz").await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;

        assert_eq!(
            key_set(&body),
            vec!["services", "status", "timestamp"],
            "top-level keys"
        );
        let entry = &body["services"][0];
        assert_eq!(
            key_set(entry),
            vec!["message", "name", "status"],
            "service entry keys"
        );
        assert!(
            entry["message"].is_null(),
            "absent message serializes as null"
        );
    }

    /// Compatibility source: overrides only the bare status methods, leaving
    /// the report methods on their trait defaults.
    struct StatusOnlySource {
        readiness: HealthStatus,
    }

    #[async_trait]
    impl HealthSource for StatusOnlySource {
        async fn liveness(&self) -> HealthStatus {
            HealthStatus::Healthy
        }

        async fn readiness(&self) -> HealthStatus {
            self.readiness
        }
    }

    #[tokio::test]
    async fn status_only_source_healthy_returns_200_empty_services() {
        let source: Arc<dyn HealthSource> = Arc::new(StatusOnlySource {
            readiness: HealthStatus::Healthy,
        });
        let response = get(health_router(source), "/readyz").await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["status"], "Healthy");
        assert_eq!(body["services"].as_array().map(Vec::len), Some(0));
    }

    #[tokio::test]
    async fn status_only_source_nonhealthy_preserves_empty_services() {
        let source: Arc<dyn HealthSource> = Arc::new(StatusOnlySource {
            readiness: HealthStatus::Unhealthy,
        });
        let response = get(health_router(source), "/readyz").await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = body_json(response).await;
        assert_eq!(body["status"], "Unhealthy");
        assert_eq!(body["services"].as_array().map(Vec::len), Some(0));
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
