use std::sync::Arc;

use async_trait::async_trait;
use camel_component_api::{AsyncHealthCheck, CamelError, CheckResult};
use tracing::warn;

use crate::pool::{BridgeState, CxfBridgePool, cxf_bridge_client};
use crate::proto::HealthRequest;

// ── CxfBridgeHealthProbe ─────────────────────────────────────────────────────

/// Trait abstracting the gRPC health probe against the CXF bridge sidecar.
#[async_trait]
pub trait CxfBridgeHealthProbe: Send + Sync {
    fn probe(&self) -> HealthFuture;

    /// True while no bridge slot exists yet (producer-only routes create it
    /// on the first call). Readiness reports this as Degraded (200).
    fn not_started(&self) -> bool {
        false
    }
}

pub type HealthFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, CamelError>> + Send>>;

// ── CxfBridgeProbe ───────────────────────────────────────────────────────────

/// Real implementation: peeks at the pool slot state and, if Ready, calls the
/// bridge's gRPC health endpoint.
pub struct CxfBridgeProbe {
    pool: Arc<CxfBridgePool>,
}

impl CxfBridgeProbe {
    pub fn new(pool: Arc<CxfBridgePool>) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl CxfBridgeHealthProbe for CxfBridgeProbe {
    fn not_started(&self) -> bool {
        !self.pool.slots.contains_key(&CxfBridgePool::slot_key())
    }

    fn probe(&self) -> HealthFuture {
        let pool = Arc::clone(&self.pool);
        Box::pin(async move {
            let slot_key = CxfBridgePool::slot_key();
            let slot = pool.slots.get(&slot_key).ok_or_else(|| {
                CamelError::ProcessorError("cxf bridge slot not found".to_string())
            })?;

            let channel = {
                let state = slot.state_rx.borrow();
                match &*state {
                    BridgeState::Ready { channel } => channel.clone(),
                    BridgeState::Degraded(reason) => {
                        return Err(CamelError::ProcessorError(format!(
                            "cxf bridge degraded: {reason}"
                        )));
                    }
                    BridgeState::Starting => {
                        return Err(CamelError::ProcessorError(
                            "cxf bridge is starting".to_string(),
                        ));
                    }
                    BridgeState::Restarting { attempt, .. } => {
                        return Err(CamelError::ProcessorError(format!(
                            "cxf bridge restarting (attempt {attempt})"
                        )));
                    }
                    BridgeState::Stopped => {
                        return Err(CamelError::ProcessorError(
                            "cxf bridge is stopped".to_string(),
                        ));
                    }
                }
            };

            let mut client = cxf_bridge_client(channel);
            let resp = client
                .health(HealthRequest {})
                .await
                .map_err(|e| CamelError::ProcessorError(format!("cxf health RPC failed: {e}")))?;

            Ok(resp.into_inner().healthy)
        })
    }
}

// ── CxfHealthCheck ───────────────────────────────────────────────────────────

/// `AsyncHealthCheck` implementation for the CXF component.
pub struct CxfHealthCheck {
    probe: Box<dyn CxfBridgeHealthProbe>,
}

impl CxfHealthCheck {
    pub fn new(pool: Arc<CxfBridgePool>) -> Self {
        Self {
            probe: Box::new(CxfBridgeProbe::new(pool)),
        }
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn with_probe(probe: Box<dyn CxfBridgeHealthProbe>) -> Self {
        Self { probe }
    }
}

#[async_trait]
impl AsyncHealthCheck for CxfHealthCheck {
    fn name(&self) -> &str {
        "cxf"
    }

    async fn check(&self) -> CheckResult {
        if self.probe.not_started() {
            return CheckResult::degraded("cxf", "bridge not started (starts on first use)");
        }
        match self.probe.probe().await {
            Ok(true) => CheckResult::healthy("cxf"),
            Ok(false) => CheckResult::unhealthy("cxf", "bridge reported unhealthy"),
            Err(e) => {
                warn!(error = %e, "cxf health probe failed");
                CheckResult::unhealthy("cxf", &e.to_string())
            }
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::HealthStatus;

    struct MockProbe {
        result: Result<bool, CamelError>,
    }

    #[async_trait]
    impl CxfBridgeHealthProbe for MockProbe {
        fn probe(&self) -> HealthFuture {
            let result = self.result.clone();
            Box::pin(async move { result })
        }
    }

    struct NotStartedProbe;

    #[async_trait]
    impl CxfBridgeHealthProbe for NotStartedProbe {
        fn probe(&self) -> HealthFuture {
            Box::pin(async { Err(CamelError::ProcessorError("must not probe".to_string())) })
        }

        fn not_started(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn not_started_bridge_returns_degraded() {
        let check = CxfHealthCheck::with_probe(Box::new(NotStartedProbe));
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Degraded);
        assert!(result.message.as_deref().unwrap().contains("not started"));
    }

    #[tokio::test]
    async fn healthy_returns_healthy() {
        let check = CxfHealthCheck::with_probe(Box::new(MockProbe { result: Ok(true) }));
        assert_eq!(check.name(), "cxf");
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Healthy);
        assert_eq!(result.name, "cxf");
    }

    #[tokio::test]
    async fn unhealthy_bridge_returns_unhealthy() {
        let check = CxfHealthCheck::with_probe(Box::new(MockProbe { result: Ok(false) }));
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert!(result.message.as_deref().unwrap().contains("unhealthy"));
    }

    #[tokio::test]
    async fn probe_error_returns_unhealthy_with_message() {
        let check = CxfHealthCheck::with_probe(Box::new(MockProbe {
            result: Err(CamelError::ProcessorError("connection refused".to_string())),
        }));
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert!(
            result
                .message
                .as_deref()
                .unwrap()
                .contains("connection refused")
        );
    }
}
