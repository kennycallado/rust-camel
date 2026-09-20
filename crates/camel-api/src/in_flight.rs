//! Context-global in-flight claim (drainclaim), Exchange-carried
//! extension (claimfamily).
//!
//! [`InFlightClaim`] is one accepted-not-completed unit on the
//! context-global counter (`CamelContext::total_in_flight()`). The type
//! lives in camel-api — not camel-component-api — so [`crate::Exchange`]
//! can carry a claim directly (rc-hllkk, rc-qbigm): stash sites behind
//! `dyn` traits and raw-`Exchange` channels (resequencer buffers,
//! embedded aggregator buckets) receive resident exchanges as bare
//! `Exchange` values with no envelope, and their residency stays counted
//! only if the claim rides the exchange itself.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// One accepted-not-completed unit on the context-global in-flight
/// counter.
///
/// Attaching a claim increments the counter; dropping it decrements the
/// counter exactly once (RAII), covering every release path — normal
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
pub struct InFlightClaim(Arc<AtomicU64>);

impl InFlightClaim {
    /// Mint a claim against `counter`, incrementing it by one.
    pub fn attach(counter: &Arc<AtomicU64>) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self(Arc::clone(counter))
    }

    /// Mint a sibling claim for a fanout copy, incrementing the same
    /// counter by one. The original claim stays live.
    pub fn split(&self) -> Self {
        self.0.fetch_add(1, Ordering::AcqRel);
        Self(Arc::clone(&self.0))
    }
}

impl Drop for InFlightClaim {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
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
        let counter = Arc::new(AtomicU64::new(0));
        let claim = InFlightClaim::attach(&counter);
        assert_eq!(counter.load(Ordering::Acquire), 1);
        drop(claim);
        assert_eq!(counter.load(Ordering::Acquire), 0);
    }

    #[test]
    fn claim_split_adds_one_sibling() {
        let counter = Arc::new(AtomicU64::new(0));
        let original = InFlightClaim::attach(&counter);
        assert_eq!(counter.load(Ordering::Acquire), 1);
        let sibling = original.split();
        assert_eq!(counter.load(Ordering::Acquire), 2);
        drop(sibling);
        assert_eq!(counter.load(Ordering::Acquire), 1);
        drop(original);
        assert_eq!(counter.load(Ordering::Acquire), 0);
    }

    #[test]
    fn claim_debug_does_not_leak_pointer() {
        let counter = Arc::new(AtomicU64::new(0));
        let claim = InFlightClaim::attach(&counter);
        assert_eq!(format!("{claim:?}"), "InFlightClaim");
    }
}
