//! Health monitoring types for rust-camel.
//!
//! This module provides types for tracking and reporting the health status
//! of services in a rust-camel application.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::lifecycle::{HealthStatus, ServiceStatus};

/// System-wide health report containing aggregated status of all services.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    pub status: HealthStatus,
    pub services: Vec<ServiceHealth>,
    pub timestamp: DateTime<Utc>,
}

impl Default for HealthReport {
    fn default() -> Self {
        Self {
            status: HealthStatus::Healthy,
            services: Vec::new(),
            timestamp: Utc::now(),
        }
    }
}

/// Health status of an individual service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceHealth {
    pub name: String,
    pub status: ServiceStatus,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    pub name: String,
    pub status: HealthStatus,
    pub message: Option<String>,
}

impl CheckResult {
    pub fn healthy(name: &str) -> Self {
        Self {
            name: name.to_string(),
            status: HealthStatus::Healthy,
            message: None,
        }
    }

    pub fn unhealthy(name: &str, reason: &str) -> Self {
        Self {
            name: name.to_string(),
            status: HealthStatus::Unhealthy,
            message: Some(reason.to_string()),
        }
    }

    pub fn degraded(name: &str, reason: &str) -> Self {
        Self {
            name: name.to_string(),
            status: HealthStatus::Degraded,
            message: Some(reason.to_string()),
        }
    }
}

#[async_trait]
pub trait AsyncHealthCheck: Send + Sync {
    fn name(&self) -> &str;
    async fn check(&self) -> CheckResult;
}

/// Programmatic health state readable by platform adapters.
/// `camel-health` implements this; `camel-platform-kubernetes` consumes it via this trait.
/// Neither crate depends on the other.
#[async_trait]
pub trait HealthSource: Send + Sync {
    async fn liveness(&self) -> HealthStatus;
    async fn readiness(&self) -> HealthStatus;

    async fn health_report(&self) -> HealthReport {
        HealthReport {
            status: self.readiness().await,
            services: vec![],
            timestamp: chrono::Utc::now(),
        }
    }

    /// Default: `Healthy` — non-K8s implementors need not override.
    async fn startup(&self) -> HealthStatus {
        HealthStatus::Healthy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_health_source_default_startup() {
        struct MinimalSource;
        #[async_trait]
        impl HealthSource for MinimalSource {
            async fn liveness(&self) -> HealthStatus {
                HealthStatus::Healthy
            }

            async fn readiness(&self) -> HealthStatus {
                HealthStatus::Healthy
            }
        }
        let s = MinimalSource;
        assert_eq!(s.startup().await, HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn test_health_source_custom_startup() {
        struct BootingSource;
        #[async_trait]
        impl HealthSource for BootingSource {
            async fn liveness(&self) -> HealthStatus {
                HealthStatus::Healthy
            }

            async fn readiness(&self) -> HealthStatus {
                HealthStatus::Healthy
            }

            async fn startup(&self) -> HealthStatus {
                HealthStatus::Unhealthy
            }
        }
        let s = BootingSource;
        assert_eq!(s.startup().await, HealthStatus::Unhealthy);
    }

    #[test]
    fn test_health_report_serialization() {
        let report = HealthReport {
            status: HealthStatus::Healthy,
            services: vec![ServiceHealth {
                name: "prometheus".to_string(),
                status: ServiceStatus::Started,
                message: None,
            }],
            timestamp: chrono::Utc::now(),
        };

        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("Healthy"));
        assert!(json.contains("prometheus"));
        assert!(json.contains("Started"));
        assert!(json.contains("timestamp"));
    }

    #[test]
    fn test_health_report_default() {
        let report = HealthReport::default();
        assert_eq!(report.status, HealthStatus::Healthy);
        assert!(report.services.is_empty());
        assert!(report.timestamp <= chrono::Utc::now());
    }

    #[test]
    fn test_service_health_serialization_round_trip() {
        let svc = ServiceHealth {
            name: "kafka".to_string(),
            status: ServiceStatus::Stopped,
            message: None,
        };

        let json = serde_json::to_string(&svc).unwrap();
        assert!(json.contains("kafka"));
        assert!(json.contains("Stopped"));

        let decoded: ServiceHealth = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.name, "kafka");
        assert_eq!(decoded.status, ServiceStatus::Stopped);
        assert!(decoded.message.is_none());
    }

    #[tokio::test]
    async fn test_health_source_liveness_and_readiness() {
        struct MixedSource;
        #[async_trait]
        impl HealthSource for MixedSource {
            async fn liveness(&self) -> HealthStatus {
                HealthStatus::Healthy
            }

            async fn readiness(&self) -> HealthStatus {
                HealthStatus::Unhealthy
            }
        }

        let s = MixedSource;
        assert_eq!(s.liveness().await, HealthStatus::Healthy);
        assert_eq!(s.readiness().await, HealthStatus::Unhealthy);
    }

    #[test]
    fn test_health_report_serialization_round_trip() {
        let report = HealthReport {
            status: HealthStatus::Unhealthy,
            services: vec![
                ServiceHealth {
                    name: "db".to_string(),
                    status: ServiceStatus::Started,
                    message: None,
                },
                ServiceHealth {
                    name: "queue".to_string(),
                    status: ServiceStatus::Stopped,
                    message: None,
                },
            ],
            timestamp: chrono::Utc::now(),
        };

        let json = serde_json::to_string(&report).unwrap();
        let back: HealthReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back.status, HealthStatus::Unhealthy);
        assert_eq!(back.services.len(), 2);
        assert_eq!(back.services[0].name, "db");
        assert_eq!(back.services[1].status, ServiceStatus::Stopped);
    }

    #[tokio::test]
    async fn test_health_source_startup_override_independent_from_readiness() {
        struct StartupOnly;
        #[async_trait]
        impl HealthSource for StartupOnly {
            async fn liveness(&self) -> HealthStatus {
                HealthStatus::Healthy
            }

            async fn readiness(&self) -> HealthStatus {
                HealthStatus::Unhealthy
            }

            async fn startup(&self) -> HealthStatus {
                HealthStatus::Healthy
            }
        }

        let source = StartupOnly;
        assert_eq!(source.liveness().await, HealthStatus::Healthy);
        assert_eq!(source.readiness().await, HealthStatus::Unhealthy);
        assert_eq!(source.startup().await, HealthStatus::Healthy);
    }

    #[test]
    fn test_check_result_healthy() {
        let r = CheckResult::healthy("kafka");
        assert_eq!(r.name, "kafka");
        assert_eq!(r.status, HealthStatus::Healthy);
        assert!(r.message.is_none());
    }

    #[test]
    fn test_check_result_unhealthy() {
        let r = CheckResult::unhealthy("redis", "connection refused");
        assert_eq!(r.name, "redis");
        assert_eq!(r.status, HealthStatus::Unhealthy);
        assert_eq!(r.message.as_deref(), Some("connection refused"));
    }

    #[test]
    fn test_check_result_degraded() {
        let r = CheckResult::degraded("sql", "pool exhausted");
        assert_eq!(r.name, "sql");
        assert_eq!(r.status, HealthStatus::Degraded);
        assert_eq!(r.message.as_deref(), Some("pool exhausted"));
    }

    #[test]
    fn test_check_result_serialization_round_trip() {
        let r = CheckResult::unhealthy("opensearch", "timeout");
        let json = serde_json::to_string(&r).unwrap();
        let back: CheckResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "opensearch");
        assert_eq!(back.status, HealthStatus::Unhealthy);
        assert_eq!(back.message.as_deref(), Some("timeout"));
    }
}
