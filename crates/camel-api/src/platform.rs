use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// Node identity in the platform environment.
/// In Kubernetes: pod name, namespace, labels from Downward API.
/// In local/test: hostname or user-supplied string.
#[derive(Debug, Clone)]
pub struct PlatformIdentity {
    pub node_id: String,
    pub namespace: Option<String>,
    pub labels: HashMap<String, String>,
}

impl PlatformIdentity {
    pub fn local(node_id: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            namespace: None,
            labels: HashMap::new(),
        }
    }
}

/// Leadership state change events delivered asynchronously.
#[derive(Debug, Clone, PartialEq)]
pub enum LeadershipEvent {
    StartedLeading,
    StoppedLeading,
}

/// Platform errors.
#[derive(Debug, Error)]
pub enum PlatformError {
    #[error("leader elector already started")]
    AlreadyStarted,
    #[error("step_down failed: elector loop terminated unexpectedly")]
    StepDownFailed,
    #[error("platform not available: {0}")]
    NotAvailable(String),
    #[error("configuration error: {0}")]
    Config(String),
}

/// Handle returned by `LeaderElector::start()`.
/// One-shot per `LeaderElector` instance — create a new instance to restart.
pub struct LeadershipHandle {
    /// Subscribe to leadership state changes.
    pub events: watch::Receiver<Option<LeadershipEvent>>,
    /// Atomic readable shortcut for current leadership state.
    is_leader: Arc<AtomicBool>,
    /// Internal — used by `step_down()` to cancel the elector loop.
    cancel: CancellationToken,
    /// Await full loop termination after `step_down()`.
    terminated: tokio::sync::oneshot::Receiver<()>,
}

impl LeadershipHandle {
    /// Public constructor — required by `camel-platform-kubernetes` which lives in a separate crate
    /// and cannot use struct literal syntax for fields that are private.
    pub fn new(
        events: watch::Receiver<Option<LeadershipEvent>>,
        is_leader: Arc<AtomicBool>,
        cancel: CancellationToken,
        terminated: tokio::sync::oneshot::Receiver<()>,
    ) -> Self {
        Self {
            events,
            is_leader,
            cancel,
            terminated,
        }
    }

    pub fn is_leader(&self) -> bool {
        self.is_leader.load(Ordering::Acquire)
    }

    /// Signal step-down AND await full teardown:
    /// lease release + loop termination + `StoppedLeading` delivered.
    pub async fn step_down(self) -> Result<(), PlatformError> {
        self.cancel.cancel();
        self.terminated
            .await
            .map_err(|_| PlatformError::StepDownFailed)
    }
}

/// Leader election abstraction.
/// The elector owns the renew loop — callers do not manage timers.
/// One-shot: `start()` may only be called once per instance.
#[async_trait]
pub trait LeaderElector: Send + Sync {
    async fn start(&self, identity: PlatformIdentity) -> Result<LeadershipHandle, PlatformError>;
}

/// Readiness gate — local override that forces readiness state regardless of `HealthSource`.
/// Name reflects actual role: a gate on local health state, not a push to external system.
///
/// Precedence (highest to lowest):
///   1. `notify_starting()` → NotReady, always
///   2. `notify_not_ready(reason)` → NotReady
///   3. `HealthSource::readiness()` — fallback when no override active
#[async_trait]
pub trait ReadinessGate: Send + Sync {
    async fn notify_ready(&self);
    async fn notify_not_ready(&self, reason: &str);
    async fn notify_starting(&self);
}

/// No-op leader elector — always wins leadership immediately.
/// Correct for single-node deployments and tests that do not need real K8s.
pub struct NoopLeaderElector;

#[async_trait]
impl LeaderElector for NoopLeaderElector {
    async fn start(&self, _identity: PlatformIdentity) -> Result<LeadershipHandle, PlatformError> {
        let (tx, rx) = watch::channel(Some(LeadershipEvent::StartedLeading));
        let (_term_tx, term_rx) = tokio::sync::oneshot::channel::<()>();
        // Drop tx immediately — channel stays at StartedLeading, never changes.
        drop(tx);
        Ok(LeadershipHandle::new(
            rx,
            Arc::new(AtomicBool::new(true)),
            CancellationToken::new(),
            term_rx,
        ))
    }
}

/// No-op readiness gate — all calls are no-ops, never blocks.
pub struct NoopReadinessGate;

#[async_trait]
impl ReadinessGate for NoopReadinessGate {
    async fn notify_ready(&self) {}
    async fn notify_not_ready(&self, _reason: &str) {}
    async fn notify_starting(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_identity_local() {
        let id = PlatformIdentity::local("my-node");
        assert_eq!(id.node_id, "my-node");
        assert!(id.namespace.is_none());
        assert!(id.labels.is_empty());
    }

    #[tokio::test]
    async fn test_noop_leader_elector_is_leader() {
        let elector = NoopLeaderElector;
        let handle = elector
            .start(PlatformIdentity::local("test"))
            .await
            .unwrap();
        assert!(handle.is_leader());
    }

    #[tokio::test]
    async fn test_noop_leader_elector_event_started_leading() {
        let elector = NoopLeaderElector;
        let handle = elector
            .start(PlatformIdentity::local("test"))
            .await
            .unwrap();
        let event = handle.events.borrow().clone();
        assert_eq!(event, Some(LeadershipEvent::StartedLeading));
    }

    #[tokio::test]
    async fn test_noop_leader_elector_step_down() {
        let elector = NoopLeaderElector;
        let handle = elector
            .start(PlatformIdentity::local("test"))
            .await
            .unwrap();
        // step_down() should not hang — term_rx resolves when _term_tx is dropped
        let result = handle.step_down().await;
        // _term_tx was dropped in start(), so Receiver returns Err(RecvError) → StepDownFailed
        assert!(
            result.is_err(),
            "NoopLeaderElector step_down should return StepDownFailed because _term_tx is dropped"
        );
    }

    #[tokio::test]
    async fn test_noop_readiness_gate_all_methods() {
        let gate = NoopReadinessGate;
        gate.notify_starting().await;
        gate.notify_not_ready("test").await;
        gate.notify_ready().await;
        // All methods must complete without panicking
    }

    #[test]
    fn test_leadership_event_equality() {
        assert_eq!(
            LeadershipEvent::StartedLeading,
            LeadershipEvent::StartedLeading
        );
        assert_ne!(
            LeadershipEvent::StartedLeading,
            LeadershipEvent::StoppedLeading
        );
    }

    #[test]
    fn test_platform_error_display() {
        let e = PlatformError::AlreadyStarted;
        assert!(e.to_string().contains("already started"));
        let e2 = PlatformError::NotAvailable("no k8s".into());
        assert!(e2.to_string().contains("no k8s"));
    }
}
