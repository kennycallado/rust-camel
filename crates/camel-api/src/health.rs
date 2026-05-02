//! Health monitoring types for rust-camel.
//!
//! This module provides types for tracking and reporting the health status
//! of services in a rust-camel application.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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
}

pub type HealthChecker = Arc<dyn Fn() -> HealthReport + Send + Sync>;

/// Programmatic health state readable by platform adapters.
/// `camel-health` implements this; `camel-platform-kubernetes` consumes it via this trait.
/// Neither crate depends on the other.
pub trait HealthSource: Send + Sync {
    fn liveness(&self) -> HealthStatus;
    fn readiness(&self) -> HealthStatus;

    /// Default: `Healthy` — non-K8s implementors need not override.
    fn startup(&self) -> HealthStatus {
        HealthStatus::Healthy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_source_default_startup() {
        struct MinimalSource;
        impl HealthSource for MinimalSource {
            fn liveness(&self) -> HealthStatus {
                HealthStatus::Healthy
            }

            fn readiness(&self) -> HealthStatus {
                HealthStatus::Healthy
            }
        }
        let s = MinimalSource;
        assert_eq!(s.startup(), HealthStatus::Healthy);
    }

    #[test]
    fn test_health_source_custom_startup() {
        struct BootingSource;
        impl HealthSource for BootingSource {
            fn liveness(&self) -> HealthStatus {
                HealthStatus::Healthy
            }

            fn readiness(&self) -> HealthStatus {
                HealthStatus::Healthy
            }

            fn startup(&self) -> HealthStatus {
                HealthStatus::Unhealthy
            }
        }
        let s = BootingSource;
        assert_eq!(s.startup(), HealthStatus::Unhealthy);
    }

    #[test]
    fn test_health_report_serialization() {
        let report = HealthReport {
            status: HealthStatus::Healthy,
            services: vec![ServiceHealth {
                name: "prometheus".to_string(),
                status: ServiceStatus::Started,
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
        };

        let json = serde_json::to_string(&svc).unwrap();
        assert!(json.contains("kafka"));
        assert!(json.contains("Stopped"));

        let decoded: ServiceHealth = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.name, "kafka");
        assert_eq!(decoded.status, ServiceStatus::Stopped);
    }

    #[test]
    fn test_health_checker_type_alias_invocation() {
        let checker: HealthChecker = Arc::new(|| HealthReport {
            status: HealthStatus::Unhealthy,
            services: vec![ServiceHealth {
                name: "db".to_string(),
                status: ServiceStatus::Started,
            }],
            timestamp: chrono::Utc::now(),
        });

        let report = checker();
        assert_eq!(report.status, HealthStatus::Unhealthy);
        assert_eq!(report.services.len(), 1);
        assert_eq!(report.services[0].name, "db");
    }

    #[test]
    fn test_health_source_liveness_and_readiness() {
        struct MixedSource;
        impl HealthSource for MixedSource {
            fn liveness(&self) -> HealthStatus {
                HealthStatus::Healthy
            }

            fn readiness(&self) -> HealthStatus {
                HealthStatus::Unhealthy
            }
        }

        let s = MixedSource;
        assert_eq!(s.liveness(), HealthStatus::Healthy);
        assert_eq!(s.readiness(), HealthStatus::Unhealthy);
    }
}
