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
    #[error("leadership lock already active: {lock_name}")]
    LockAlreadyActive { lock_name: String },
    #[error("step_down failed: elector loop terminated unexpectedly")]
    StepDownFailed,
    #[error("platform not available: {0}")]
    NotAvailable(String),
    #[error("configuration error: {0}")]
    Config(String),
}

/// Handle returned by `LeadershipService::start()`.
pub struct LeadershipHandle {
    /// Subscribe to leadership state changes.
    pub events: watch::Receiver<Option<LeadershipEvent>>,
    /// Atomic readable shortcut for current leadership state.
    is_leader: Arc<AtomicBool>,
    /// Internal — used by `step_down()` to cancel the elector loop.
    cancel: CancellationToken,
    /// Await full loop termination after `step_down()`.
    terminated: Option<tokio::sync::oneshot::Receiver<()>>,
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
            terminated: Some(terminated),
        }
    }

    pub fn is_leader(&self) -> bool {
        self.is_leader.load(Ordering::Acquire)
    }

    /// Signal step-down AND await full teardown:
    /// lease release + loop termination + `StoppedLeading` delivered.
    pub async fn step_down(mut self) -> Result<(), PlatformError> {
        self.cancel.cancel();
        self.terminated
            .take()
            .ok_or(PlatformError::StepDownFailed)?
            .await
            .map_err(|_| PlatformError::StepDownFailed)
    }
}

impl Drop for LeadershipHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Leadership abstraction.
#[async_trait]
pub trait LeadershipService: Send + Sync {
    async fn start(&self, lock_name: &str) -> Result<LeadershipHandle, PlatformError>;
}

/// Platform service abstraction.
pub trait PlatformService: Send + Sync {
    fn identity(&self) -> PlatformIdentity;
    fn readiness_gate(&self) -> Arc<dyn ReadinessGate>;
    fn leadership(&self) -> Arc<dyn LeadershipService>;
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

/// No-op leadership service.
/// Correct for single-node deployments and tests that do not need real K8s.
///
/// Allows multiple `start()` calls for the same lock name — each returns an
/// independent `LeadershipHandle`. This matches the `master:` semantics where
/// multiple routes can compete for the same lock.
pub struct NoopLeadershipService;

impl NoopLeadershipService {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NoopLeadershipService {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LeadershipService for NoopLeadershipService {
    async fn start(&self, lock_name: &str) -> Result<LeadershipHandle, PlatformError> {
        let (tx, rx) = watch::channel(Some(LeadershipEvent::StartedLeading));
        let (term_tx, term_rx) = tokio::sync::oneshot::channel::<()>();
        let cancel = CancellationToken::new();
        let cancel_for_task = cancel.clone();
        let is_leader = Arc::new(AtomicBool::new(true));
        let is_leader_for_task = Arc::clone(&is_leader);
        let lock_name = lock_name.to_string();

        tokio::spawn(async move {
            cancel_for_task.cancelled().await;
            is_leader_for_task.store(false, Ordering::Release);
            let _ = tx.send(Some(LeadershipEvent::StoppedLeading));
            drop(lock_name);
            let _ = term_tx.send(());
        });

        Ok(LeadershipHandle::new(rx, is_leader, cancel, term_rx))
    }
}

/// No-op platform service.
pub struct NoopPlatformService {
    identity: PlatformIdentity,
    readiness_gate: Arc<dyn ReadinessGate>,
    leadership: Arc<dyn LeadershipService>,
}

impl NoopPlatformService {
    pub fn new(identity: PlatformIdentity) -> Self {
        Self {
            identity,
            readiness_gate: Arc::new(NoopReadinessGate),
            leadership: Arc::new(NoopLeadershipService::new()),
        }
    }
}

impl Default for NoopPlatformService {
    fn default() -> Self {
        Self::new(PlatformIdentity::local("noop"))
    }
}

impl PlatformService for NoopPlatformService {
    fn identity(&self) -> PlatformIdentity {
        self.identity.clone()
    }

    fn readiness_gate(&self) -> Arc<dyn ReadinessGate> {
        Arc::clone(&self.readiness_gate)
    }

    fn leadership(&self) -> Arc<dyn LeadershipService> {
        Arc::clone(&self.leadership)
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
    async fn test_noop_leadership_service_is_leader() {
        let leadership = NoopLeadershipService::new();
        let handle = leadership.start("lock-a").await.unwrap();
        assert!(handle.is_leader());
    }

    #[tokio::test]
    async fn test_noop_leadership_service_allows_multiple_distinct_locks() {
        let leadership = NoopLeadershipService::new();
        let lock_a = leadership.start("lock-a").await.unwrap();
        let lock_b = leadership.start("lock-b").await.unwrap();

        assert!(lock_a.is_leader());
        assert!(lock_b.is_leader());

        lock_a.step_down().await.unwrap();
        lock_b.step_down().await.unwrap();
    }

    #[tokio::test]
    async fn test_noop_leadership_service_same_lock_allows_multiple() {
        let leadership = NoopLeadershipService::new();
        let first = leadership.start("lock-a").await.unwrap();
        let second = leadership.start("lock-a").await.unwrap();

        assert!(first.is_leader());
        assert!(second.is_leader());

        first.step_down().await.unwrap();
        second.step_down().await.unwrap();
    }

    #[tokio::test]
    async fn test_noop_leadership_handle_semantics_and_reacquire() {
        let leadership = NoopLeadershipService::new();
        let handle = leadership.start("lock-a").await.unwrap();
        let mut events = handle.events.clone();
        let is_leader = Arc::clone(&handle.is_leader);

        let event = handle.events.borrow().clone();
        assert_eq!(event, Some(LeadershipEvent::StartedLeading));

        handle.step_down().await.unwrap();
        events.changed().await.unwrap();
        assert_eq!(*events.borrow(), Some(LeadershipEvent::StoppedLeading));
        assert!(!is_leader.load(Ordering::Acquire));

        let reacquired = leadership.start("lock-a").await;
        assert!(reacquired.is_ok());
    }

    #[tokio::test]
    async fn test_noop_leadership_drop_cleans_up() {
        let leadership = NoopLeadershipService::new();
        let handle = leadership.start("lock-drop").await.unwrap();
        assert!(handle.is_leader());
        drop(handle);

        let handle2 = leadership.start("lock-drop").await.unwrap();
        assert!(handle2.is_leader());
        handle2.step_down().await.unwrap();
    }

    #[tokio::test]
    async fn test_noop_readiness_gate_all_methods() {
        let gate = NoopReadinessGate;
        gate.notify_starting().await;
        gate.notify_not_ready("test").await;
        gate.notify_ready().await;
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
        let e = PlatformError::LockAlreadyActive {
            lock_name: "alpha".into(),
        };
        assert!(e.to_string().contains("alpha"));
        let e2 = PlatformError::NotAvailable("no k8s".into());
        assert!(e2.to_string().contains("no k8s"));
    }
}
