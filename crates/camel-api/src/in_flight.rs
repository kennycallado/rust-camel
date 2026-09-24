//! Context-global in-flight claim (drainclaim), Exchange-carried
//! extension (claimfamily).
//!
//! [`InFlightClaim`] is one accepted-not-completed unit on the
//! context-global gauge (`CamelContext::total_in_flight()`). The type
//! lives in camel-api — not camel-component-api — so [`crate::Exchange`]
//! can carry a claim directly (rc-hllkk, rc-qbigm): stash sites behind
//! `dyn` traits and raw-`Exchange` channels (resequencer buffers,
//! embedded aggregator buckets) receive resident exchanges as bare
//! `Exchange` values with no envelope, and their residency stays counted
//! only if the claim rides the exchange itself.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Context-global in-flight gauge (drainclaim): the accepted-not-completed
/// exchange counter plus a zero-transition notification.
///
/// Counting is owned by [`InFlightClaim`] RAII (attach increments, drop
/// decrements) — `inc`/`dec` are crate-private so no cross-crate code can
/// skew the count outside the claim lifecycle. Observers read the live
/// count via [`InFlightGauge::total`] and await quiescence via
/// [`InFlightGauge::idle`]: the last release (1→0) fires
/// `notify_waiters()`, waking every waiter registered before the release
/// (register-before-check: pin the `notified()` future and `enable()` it,
/// then read state). The sync-callable notify is what makes `Drop` a
/// completion signal for notification-based settle.
pub struct InFlightGauge {
    count: AtomicU64,
    idle: tokio::sync::Notify,
}

impl InFlightGauge {
    /// Create a gauge with a zero count.
    pub fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
            idle: tokio::sync::Notify::new(),
        }
    }

    /// Increment the in-flight count. Crate-private: claims are the only
    /// counting path.
    pub(crate) fn inc(&self) {
        self.count.fetch_add(1, Ordering::AcqRel);
    }

    /// Decrement the in-flight count; the last release (1→0) wakes every
    /// registered idle waiter. Crate-private: claims are the only counting
    /// path.
    pub(crate) fn dec(&self) {
        let prev = self.count.fetch_sub(1, Ordering::Release);
        if prev == 1 {
            self.idle.notify_waiters();
        }
    }

    /// Read the live in-flight count (Acquire load; pairs with the
    /// `Release` decrement so a woken waiter observes the zero).
    pub fn total(&self) -> u64 {
        self.count.load(Ordering::Acquire)
    }

    /// Notification slot resolved on the last release (1→0). Waiters must
    /// register before checking the count (pin + `enable()`); notifications
    /// are not stored.
    pub fn idle(&self) -> &tokio::sync::Notify {
        &self.idle
    }
}

impl Default for InFlightGauge {
    fn default() -> Self {
        Self::new()
    }
}

/// One accepted-not-completed unit on the context-global in-flight
/// gauge.
///
/// Attaching a claim increments the gauge; dropping it decrements the
/// gauge exactly once (RAII), covering every release path — normal
/// pipeline completion, dispatch push failure, queued-envelope drop,
/// pipeline task abort, panic, and readiness failure — with no manual
/// rollback code. Fanout sites mint one sibling claim per subscriber copy
/// via [`InFlightClaim::split`], so each copy counts and releases
/// independently (drainclaim). The type is deliberately NOT `Clone`:
/// duplicating a claim would double-release on drop; fanout copies mint
/// siblings via [`InFlightClaim::split`] instead.
///
/// Claim lifecycle during a pipeline run (claimfamily): the pipeline
/// drain site holds the envelope's claim in task scope AND splits a
/// sibling onto the exchange, so residency inside pipeline-embedded
/// stash sites (resequencer buffer, aggregator bucket) stays counted
/// after the pipeline task itself completes. Out-of-band stash emissions
/// escape with their carried claims; the in-band pipeline result has its
/// sibling taken back before the reply, so release stays at task end for
/// exchanges that complete inside the pipeline.
pub struct InFlightClaim(Arc<InFlightGauge>);

impl InFlightClaim {
    /// Mint a claim against `gauge`, incrementing it by one.
    pub fn attach(gauge: &Arc<InFlightGauge>) -> Self {
        gauge.inc();
        Self(Arc::clone(gauge))
    }

    /// Mint a sibling claim for a fanout copy, incrementing the same
    /// gauge by one. The original claim stays live.
    pub fn split(&self) -> Self {
        self.0.inc();
        Self(Arc::clone(&self.0))
    }
}

impl Drop for InFlightClaim {
    fn drop(&mut self) {
        self.0.dec();
    }
}

impl fmt::Debug for InFlightClaim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The live count is observable through the counter itself
        // (`total_in_flight()`); the Debug shape deliberately avoids
        // leaking the Arc pointer.
        f.write_str("InFlightClaim")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_attach_increments_and_drop_decrements() {
        let gauge = Arc::new(InFlightGauge::new());
        let claim = InFlightClaim::attach(&gauge);
        assert_eq!(gauge.total(), 1);
        drop(claim);
        assert_eq!(gauge.total(), 0);
    }

    #[test]
    fn claim_split_adds_one_sibling() {
        let gauge = Arc::new(InFlightGauge::new());
        let original = InFlightClaim::attach(&gauge);
        assert_eq!(gauge.total(), 1);
        let sibling = original.split();
        assert_eq!(gauge.total(), 2);
        drop(sibling);
        assert_eq!(gauge.total(), 1);
        drop(original);
        assert_eq!(gauge.total(), 0);
    }

    #[test]
    fn claim_debug_does_not_leak_pointer() {
        let gauge = Arc::new(InFlightGauge::new());
        let claim = InFlightClaim::attach(&gauge);
        assert_eq!(format!("{claim:?}"), "InFlightClaim");
    }

    #[test]
    fn gauge_counts_claim_lifecycle() {
        let gauge = Arc::new(InFlightGauge::new());
        let first = InFlightClaim::attach(&gauge);
        let second = InFlightClaim::attach(&gauge);
        assert_eq!(gauge.total(), 2);
        drop(first);
        assert_eq!(gauge.total(), 1);
        drop(second);
        assert_eq!(gauge.total(), 0);
    }

    #[tokio::test]
    async fn gauge_notifies_on_last_release_only() {
        let gauge = Arc::new(InFlightGauge::new());
        // Register-before-check (D4): enable() subscribes the waiter
        // without awaiting it, so no release can slip past unobserved.
        let mut idle = std::pin::pin!(gauge.idle().notified());
        idle.as_mut().enable();
        let first = InFlightClaim::attach(&gauge);
        let second = InFlightClaim::attach(&gauge);

        drop(first);
        let notified_early =
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut idle).await;
        assert!(
            notified_early.is_err(),
            "a non-final release must not notify idle waiters"
        );

        drop(second);
        tokio::time::timeout(std::time::Duration::from_secs(1), &mut idle)
            .await
            .expect("the last release must resolve the enabled waiter");
    }

    #[tokio::test]
    async fn gauge_notify_wakes_all_enabled_waiters() {
        let gauge = Arc::new(InFlightGauge::new());
        let mut first = std::pin::pin!(gauge.idle().notified());
        first.as_mut().enable();
        let mut second = std::pin::pin!(gauge.idle().notified());
        second.as_mut().enable();

        let claim = InFlightClaim::attach(&gauge);
        drop(claim);

        tokio::time::timeout(std::time::Duration::from_secs(1), &mut first)
            .await
            .expect("first enabled waiter must resolve on last release");
        tokio::time::timeout(std::time::Duration::from_secs(1), &mut second)
            .await
            .expect("second enabled waiter must resolve on last release");
    }
}
