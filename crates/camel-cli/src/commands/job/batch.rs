//! Drain observer for `mode: batch` jobs: wait after the trigger send
//! until every expected seda queue is empty.
//!
//! Drain contract:
//!
//! - A queue counts as drained after [`DRAIN_ZERO_SAMPLES_REQUIRED`]
//!   CONSECUTIVE POST-SEND zero-depth samples. The seda endpoint gauge
//!   does NOT see route-pipeline residency: a fire-and-forget `to:
//!   seda:` producer counts its envelope in only when the consumer
//!   route's pipeline reaches the step, so between one envelope leaving
//!   the endpoint and the next being counted the gauge honestly reads
//!   zero. A self-feeding route therefore emits zero samples at up to
//!   ~50% duty indefinitely, and no short streak can tell cycling from
//!   settled. The required window (2.5 s at the sampler's 250 ms tick)
//!   is longer than the smallest meaningful job timeout, so a queue
//!   that never drains deterministically hits the overall deadline
//!   (`Timeout` verdict) before the streak can complete.
//! - PRE-SEND ZEROS NEVER SATISFY the drain: [`BatchDepthProbe::reset`]
//!   runs after the trigger send completes and zeroes every streak, so
//!   only samples observed after the send count.
//! - An EMPTY EXPECTED SET COMPLETES IMMEDIATELY: a document with no
//!   seda consumers has nothing to wait for.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::{CamelError, Lifecycle, MetricsCollector};

/// Consecutive post-send zero-depth samples required per queue. The
/// seda sampler publishes every 250 ms, so ten samples span 2.5 s —
/// strictly longer than any 2 s overall timeout can observe (at most 8
/// samples fit between the post-send reset and a 2 s deadline), which
/// makes the `Timeout` verdict for a never-draining queue independent
/// of scheduling luck. See the module doc for why a shorter streak is
/// unsound against route-pipeline residency gaps.
const DRAIN_ZERO_SAMPLES_REQUIRED: u32 = 10;

/// Per-queue drain state: the current consecutive zero-sample streak
/// (any nonzero required threshold already implies the last sample was
/// 0, so no separate last-sample field).
#[derive(Default)]
struct BatchDepthProbeState {
    zero_run: u32,
}

/// Queue-depth observer for the batch drain: a
/// [`MetricsCollector`] that tracks the zero-sample streak of every
/// expected seda queue label (`seda:<name>` — the existing gauge label
/// set; this probe declares no new labels).
pub(crate) struct BatchDepthProbe {
    expected: HashSet<String>,
    queues: Mutex<HashMap<String, BatchDepthProbeState>>,
}

impl BatchDepthProbe {
    pub(crate) fn new(expected: HashSet<String>) -> Self {
        Self {
            expected,
            queues: Mutex::new(HashMap::new()),
        }
    }

    /// Zero every queue's streak. Called after the trigger send
    /// completes, so pre-send zero samples never satisfy the drain.
    pub(crate) fn reset(&self) {
        for state in self
            .queues
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values_mut()
        {
            state.zero_run = 0;
        }
    }

    /// True when every expected label has an entry with a zero streak
    /// of at least [`DRAIN_ZERO_SAMPLES_REQUIRED`] (consecutive zero
    /// samples). An empty expected set returns true — a document with
    /// no seda consumers completes immediately.
    pub(crate) fn all_drained(&self) -> bool {
        let queues = self.queues.lock().unwrap_or_else(|e| e.into_inner());
        self.expected.iter().all(|label| {
            queues
                .get(label)
                .is_some_and(|state| state.zero_run >= DRAIN_ZERO_SAMPLES_REQUIRED)
        })
    }
}

impl MetricsCollector for BatchDepthProbe {
    fn record_exchange_duration(&self, _route_id: &str, _duration: Duration) {}

    fn increment_errors(&self, _route_id: &str, _error_type: &str) {}

    fn increment_exchanges(&self, _route_id: &str) {}

    fn record_circuit_breaker_change(&self, _route_id: &str, _from: &str, _to: &str) {}

    fn set_queue_depth(&self, queue: &str, depth: usize) {
        if !self.expected.contains(queue) {
            return;
        }
        let mut queues = self.queues.lock().unwrap_or_else(|e| e.into_inner());
        let state = queues.entry(queue.to_string()).or_default();
        if depth == 0 {
            state.zero_run += 1;
        } else {
            state.zero_run = 0;
        }
    }
}

/// Lifecycle wrapper registering the probe into the context's shared
/// metrics handle. Registration must happen BEFORE `ctx.start()` so the
/// seda samplers' emissions fan out to the probe from their first tick.
pub(crate) struct BatchProbeLifecycle(pub(crate) Arc<BatchDepthProbe>);

#[async_trait]
impl Lifecycle for BatchProbeLifecycle {
    fn name(&self) -> &str {
        "batch-depth-probe"
    }

    async fn start(&mut self) -> Result<(), CamelError> {
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }

    fn as_metrics_collector(&self) -> Option<Arc<dyn MetricsCollector>> {
        Some(Arc::clone(&self.0) as Arc<dyn MetricsCollector>)
    }
}

/// Wait until `probe.all_drained()` holds, napping at most 100 ms at a
/// time and never past `deadline`. Returns false once the deadline has
/// passed.
pub(crate) async fn drain_until_empty(
    probe: &BatchDepthProbe,
    deadline: tokio::time::Instant,
) -> bool {
    loop {
        if probe.all_drained() {
            return true;
        }
        let now = tokio::time::Instant::now();
        let nap = deadline
            .saturating_duration_since(now)
            .min(Duration::from_millis(100));
        if nap.is_zero() {
            return false;
        }
        tokio::time::sleep(nap).await;
    }
}
