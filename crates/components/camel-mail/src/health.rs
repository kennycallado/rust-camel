//! IMAP health: cached consumer state first, a bounded probe only when stale.
//!
//! The check is registered at endpoint creation, the only hook with a
//! `ComponentContext`, before it is known whether a push consumer will run.
//! It stays healthy without probing until a push consumer attaches, so an
//! endpoint used only by `pollEnrich` never logs in periodically (same gate
//! as the http listener check). A consumer that stopped for good keeps it
//! unhealthy until a new consumer start succeeds.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::{AsyncHealthCheck, CheckResult};
use tokio::time::Instant;

use crate::store::MailStoreConnector;

/// Last known consumer connection outcomes.
#[derive(Clone, Debug, Default)]
pub(crate) struct HealthSnapshot {
    /// A push consumer was created for the endpoint.
    pub(crate) attached: bool,
    pub(crate) last_ok: Option<Instant>,
    pub(crate) last_failure: Option<(Instant, String)>,
    /// Why the consumer stopped for good (permanent error or exhausted
    /// reconnects); cleared only by a consumer that connects again.
    pub(crate) stopped: Option<String>,
}

/// Shared between the consumer (writer) and the health check (reader).
#[derive(Debug, Default)]
pub(crate) struct MailHealthState {
    inner: Mutex<HealthSnapshot>,
}

impl MailHealthState {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HealthSnapshot> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A push consumer now feeds this state; the check starts reporting.
    pub(crate) fn attach(&self) {
        self.lock().attached = true;
    }

    /// The consumer completed a connect or a poll.
    pub(crate) fn record_ok(&self) {
        let mut s = self.lock();
        s.last_ok = Some(Instant::now());
        s.last_failure = None;
        s.stopped = None;
    }

    /// A probe connected; ignored once the consumer stopped for good.
    fn record_probe_ok(&self) {
        let mut s = self.lock();
        if s.stopped.is_none() {
            s.last_ok = Some(Instant::now());
            s.last_failure = None;
        }
    }

    /// The consumer stopped and will not reconnect by itself.
    pub(crate) fn record_stopped(&self, reason: String) {
        let mut s = self.lock();
        s.last_failure = Some((Instant::now(), reason.clone()));
        s.stopped = Some(reason);
    }

    pub(crate) fn record_failure(&self, reason: String) {
        self.lock().last_failure = Some((Instant::now(), reason));
    }

    pub(crate) fn snapshot(&self) -> HealthSnapshot {
        self.lock().clone()
    }
}

/// `AsyncHealthCheck` for an IMAP endpoint.
pub(crate) struct MailHealthCheck {
    connector: Arc<dyn MailStoreConnector>,
    state: Arc<MailHealthState>,
    stale_after: Duration,
    timeout: Duration,
}

impl MailHealthCheck {
    pub(crate) fn new(
        connector: Arc<dyn MailStoreConnector>,
        state: Arc<MailHealthState>,
        stale_after: Duration,
    ) -> Self {
        Self {
            connector,
            state,
            stale_after,
            timeout: Duration::from_secs(5),
        }
    }

    #[cfg(test)]
    fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[async_trait]
impl AsyncHealthCheck for MailHealthCheck {
    fn name(&self) -> &str {
        "mail"
    }

    async fn check(&self) -> CheckResult {
        let snap = self.state.snapshot();
        if !snap.attached {
            return CheckResult::healthy(self.name());
        }
        if let Some(reason) = &snap.stopped {
            return CheckResult::unhealthy(self.name(), reason);
        }
        let now = Instant::now();
        let fresh = |t: Instant| now.saturating_duration_since(t) < self.stale_after;
        if let Some((at, reason)) = &snap.last_failure
            && snap.last_ok.is_none_or(|ok| *at > ok)
            && fresh(*at)
        {
            return CheckResult::unhealthy(self.name(), reason);
        }
        if snap.last_ok.is_some_and(fresh) {
            return CheckResult::healthy(self.name());
        }
        let probe = tokio::time::timeout(self.timeout, async {
            let mut store = self.connector.connect().await?;
            store.logout().await
        })
        .await;
        match probe {
            Ok(Ok(())) => {
                self.state.record_probe_ok();
                CheckResult::healthy(self.name())
            }
            Ok(Err(e)) => {
                self.state.record_failure(e.to_string());
                CheckResult::unhealthy(self.name(), &e.to_string())
            }
            Err(_) => CheckResult::unhealthy(self.name(), "IMAP probe timed out"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::MailError;
    use crate::store::MailStore;
    use crate::store::fake::{FakeConnector, FakeMailbox, StoreCall};
    use camel_api::HealthStatus;

    fn failing() -> FakeConnector {
        let mut mb = FakeMailbox::new();
        mb.connect_failures = u32::MAX;
        FakeConnector::new(mb)
    }

    fn calls(conn: &FakeConnector) -> Vec<StoreCall> {
        conn.mailbox
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .calls
            .clone()
    }

    fn attached() -> Arc<MailHealthState> {
        let state = MailHealthState::new();
        state.attach();
        state
    }

    fn check_for(
        conn: &FakeConnector,
        state: &Arc<MailHealthState>,
        stale: Duration,
    ) -> MailHealthCheck {
        MailHealthCheck::new(Arc::new(conn.clone()), Arc::clone(state), stale)
    }

    async fn run(check: &MailHealthCheck) -> CheckResult {
        tokio::time::timeout(Duration::from_secs(6), check.check())
            .await
            .expect("check timed out")
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn disconnected_consumer_unhealthy() {
        let conn = failing();
        let check = check_for(&conn, &attached(), Duration::from_secs(120));
        let r = run(&check).await;
        assert_eq!(r.status, HealthStatus::Unhealthy);
        assert_eq!(calls(&conn), vec![StoreCall::Connect]);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn unattached_endpoint_is_healthy_without_probe() {
        let conn = failing();
        let r = run(&check_for(
            &conn,
            &MailHealthState::new(),
            Duration::from_secs(120),
        ))
        .await;
        assert_eq!(r.status, HealthStatus::Healthy);
        assert!(
            calls(&conn).is_empty(),
            "a pull-only endpoint never logs in"
        );
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn permanent_stop_stays_unhealthy_after_staleness() {
        let conn = FakeConnector::new(FakeMailbox::new());
        let state = attached();
        state.record_ok();
        state.record_stopped("IMAP server refused the login".into());
        tokio::time::advance(Duration::from_secs(600)).await;
        let check = check_for(&conn, &state, Duration::from_secs(120));
        for _ in 0..2 {
            let r = run(&check).await;
            assert_eq!(r.status, HealthStatus::Unhealthy);
            assert!(r.message.unwrap_or_default().contains("login"));
        }
        assert!(
            calls(&conn).is_empty(),
            "a working probe must not clear a stop"
        );
        state.record_ok();
        assert_eq!(run(&check).await.status, HealthStatus::Healthy);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn fresh_ok_state_skips_probe() {
        let conn = failing();
        let state = attached();
        state.record_ok();
        let r = run(&check_for(&conn, &state, Duration::from_secs(120))).await;
        assert_eq!(r.status, HealthStatus::Healthy);
        assert!(calls(&conn).is_empty());
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn recent_failure_reported_without_probe() {
        let conn = FakeConnector::new(FakeMailbox::new());
        let state = attached();
        state.record_failure("IMAP connect error: refused".into());
        let r = run(&check_for(&conn, &state, Duration::from_secs(120))).await;
        assert_eq!(r.status, HealthStatus::Unhealthy);
        assert!(r.message.unwrap_or_default().contains("refused"));
        assert!(calls(&conn).is_empty());
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn stale_state_probes() {
        let conn = failing();
        let state = attached();
        state.record_ok();
        tokio::time::advance(Duration::from_secs(121)).await;
        let r = run(&check_for(&conn, &state, Duration::from_secs(120))).await;
        assert_eq!(r.status, HealthStatus::Unhealthy);
        assert_eq!(calls(&conn), vec![StoreCall::Connect]);
    }

    struct PendingConnector;

    #[async_trait]
    impl MailStoreConnector for PendingConnector {
        async fn connect(&self) -> Result<Box<dyn MailStore>, MailError> {
            std::future::pending().await
        }
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn stalled_probe_times_out() {
        let check = MailHealthCheck::new(
            Arc::new(PendingConnector),
            attached(),
            Duration::from_secs(120),
        )
        .with_timeout(Duration::from_secs(5));
        let r = run(&check).await;
        assert_eq!(r.status, HealthStatus::Unhealthy);
        assert!(r.message.unwrap_or_default().contains("timed out"));
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn connected_store_healthy() {
        let conn = FakeConnector::new(FakeMailbox::new());
        let state = attached();
        let r = run(&check_for(&conn, &state, Duration::from_secs(120))).await;
        assert_eq!(r.status, HealthStatus::Healthy);
        assert_eq!(calls(&conn), vec![StoreCall::Connect, StoreCall::Logout]);
        assert!(state.snapshot().last_ok.is_some());
    }
}
