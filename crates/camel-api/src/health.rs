//! Health monitoring types for rust-camel.
//!
//! This module provides types for tracking and reporting the health status
//! of services in a rust-camel application.

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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
