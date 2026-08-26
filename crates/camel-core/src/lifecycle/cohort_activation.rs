//! The gate parks first consumer-envelope dispatch until the startup cohort completes (rc-jxkj
//! lineage; the full story is in `openspec/changes/cohort-activation-barrier/`).

use tokio::sync::watch;

/// Shared level-triggered barrier for the current startup cohort.
pub(crate) struct CohortActivationGate {
    open_tx: watch::Sender<bool>,
    open_rx: watch::Receiver<bool>,
}

impl CohortActivationGate {
    /// Creates a gate that initially parks dispatch.
    pub(crate) fn new_closed() -> Self {
        let (open_tx, open_rx) = watch::channel(false);
        Self { open_tx, open_rx }
    }

    /// Opens the gate; repeated calls are no-ops to avoid needless wakeups.
    pub(crate) fn open(&self) {
        self.open_tx.send_if_modified(|open| {
            if !*open {
                *open = true;
                true
            } else {
                false
            }
        });
    }

    /// Closes the gate; repeated calls are no-ops.
    pub(crate) fn close(&self) {
        self.open_tx.send_if_modified(|open| {
            if *open {
                *open = false;
                true
            } else {
                false
            }
        });
    }

    /// Returns the gate's current level.
    // Test-only probe: production code goes through open/close (ordering
    // port, called by start_context) and subscribe (drain loops); only
    // test assertions read the level directly.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn is_open(&self) -> bool {
        *self.open_rx.borrow()
    }

    /// Clones a receiver for a waiter task; callers await `wait_for` on it.
    ///
    /// `wait_for` needs a mutable receiver, so exposing subscriptions keeps
    /// each waiter independent and preserves level-triggered wakeup behavior.
    pub(crate) fn subscribe(&self) -> watch::Receiver<bool> {
        self.open_rx.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::CohortActivationGate;

    #[test]
    fn open_is_idempotent() {
        let gate = CohortActivationGate::new_closed();

        gate.open();
        let mut rx = gate.subscribe();
        rx.borrow_and_update();
        gate.open();

        assert!(gate.is_open());
        assert!(!rx.has_changed().unwrap_or(false));
    }

    #[tokio::test]
    async fn opened_resolves_immediately_when_open() {
        let gate = CohortActivationGate::new_closed();
        let mut rx = gate.subscribe();
        gate.open();

        let wait = rx.wait_for(|open| *open);
        tokio::pin!(wait);
        let result = std::future::poll_fn(|cx| wait.as_mut().poll(cx)).await;

        assert!(result.is_ok());
    }

    #[test]
    fn close_then_open_cycle() {
        let gate = CohortActivationGate::new_closed();

        gate.open();
        gate.close();
        assert!(!gate.is_open());
        gate.open();

        assert!(gate.is_open());
    }

    #[tokio::test]
    async fn opened_parks_until_open() {
        let gate = CohortActivationGate::new_closed();
        let mut rx = gate.subscribe();
        let waiter = tokio::spawn(async move { rx.wait_for(|open| *open).await.is_ok() });

        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());

        gate.open();
        assert!(matches!(waiter.await, Ok(true)));
    }
}
