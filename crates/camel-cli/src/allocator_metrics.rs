//! jemalloc memory gauges for the `camel run` process (OpenSpec
//! `memory-gauges` task 2.2).
//!
//! The sampler reads jemalloc's `stats.*` MIBs every 5 seconds and publishes
//! each value through [`camel_api::MetricsCollector::set_allocator_memory`].
//! The emission seam ([`emit_allocator_snapshot`]) is decoupled from the
//! allocator so tests drive it with stub reads; `spawn_allocator_sampler` is
//! the only jemalloc-coupled piece and exists solely under the `jemalloc`
//! feature (same feature that swaps the global allocator in `main.rs`).
//!
//! Every item is gated `#[cfg(any(test, feature = "jemalloc"))]`: seam tests
//! build on the default feature set via `cfg(test)`, and production callers
//! build via the feature — plain un-gated `pub(crate)` items would trip
//! `dead_code` under default-feature clippy.

#[cfg(any(test, feature = "jemalloc"))]
use std::sync::Arc;

#[cfg(any(test, feature = "jemalloc"))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct AllocatorSnapshot {
    pub allocated: u64,
    pub resident: u64,
    pub active: u64,
    pub mapped: u64,
}

#[cfg(any(test, feature = "jemalloc"))]
pub(crate) fn emit_allocator_snapshot(
    read: impl Fn() -> Result<AllocatorSnapshot, String>,
    metrics: &Arc<dyn camel_api::MetricsCollector>,
) -> bool {
    use camel_api::metrics::AllocatorStat;

    match read() {
        Ok(snap) => {
            metrics.set_allocator_memory(AllocatorStat::Allocated, snap.allocated);
            metrics.set_allocator_memory(AllocatorStat::Resident, snap.resident);
            metrics.set_allocator_memory(AllocatorStat::Active, snap.active);
            metrics.set_allocator_memory(AllocatorStat::Mapped, snap.mapped);
            true
        }
        Err(_) => {
            tracing::warn!("jemalloc stats read failed; retrying next tick");
            false
        }
    }
}

/// The five MIBs the sampler needs, resolved once per process.
///
/// jemalloc's string-based `mallctl` name parsing is the per-call cost the
/// MIB API exists to avoid; resolving eagerly and caching in a static keeps
/// every 5-second tick to five integer-indexed lookups. A resolution failure
/// is cached too (and reported by `spawn_allocator_sampler` as a disabled
/// sampler) — there is no realistic recovery path mid-process.
#[cfg(feature = "jemalloc")]
struct JemallocMibs {
    epoch: tikv_jemalloc_ctl::epoch_mib,
    allocated: tikv_jemalloc_ctl::stats::allocated_mib,
    resident: tikv_jemalloc_ctl::stats::resident_mib,
    active: tikv_jemalloc_ctl::stats::active_mib,
    mapped: tikv_jemalloc_ctl::stats::mapped_mib,
}

#[cfg(feature = "jemalloc")]
fn mibs() -> Result<&'static JemallocMibs, String> {
    use std::sync::OnceLock;
    static MIBS: OnceLock<Result<JemallocMibs, String>> = OnceLock::new();
    MIBS.get_or_init(|| {
        Ok(JemallocMibs {
            epoch: tikv_jemalloc_ctl::epoch::mib().map_err(|e| e.to_string())?,
            allocated: tikv_jemalloc_ctl::stats::allocated::mib().map_err(|e| e.to_string())?,
            resident: tikv_jemalloc_ctl::stats::resident::mib().map_err(|e| e.to_string())?,
            active: tikv_jemalloc_ctl::stats::active::mib().map_err(|e| e.to_string())?,
            mapped: tikv_jemalloc_ctl::stats::mapped::mib().map_err(|e| e.to_string())?,
        })
    })
    .as_ref()
    .map_err(Clone::clone)
}

/// Advance the jemalloc epoch, then read the four stats MIBs.
///
/// The epoch advance MUST precede the reads (design D4): most statistics are
/// cached by jemalloc and only refresh when the epoch is bumped. Reading
/// without advancing, or in the other order, yields stale values that still
/// pass this function's type shape — the ordering is upheld by this function
/// being the single production read path.
#[cfg(feature = "jemalloc")]
pub(crate) fn read_jemalloc_snapshot() -> Result<AllocatorSnapshot, String> {
    let mibs = mibs()?;
    mibs.epoch.advance().map_err(|e| format!("epoch: {e}"))?;
    Ok(AllocatorSnapshot {
        allocated: mibs
            .allocated
            .read()
            .map_err(|e| format!("stats.allocated: {e}"))? as u64,
        resident: mibs
            .resident
            .read()
            .map_err(|e| format!("stats.resident: {e}"))? as u64,
        active: mibs
            .active
            .read()
            .map_err(|e| format!("stats.active: {e}"))? as u64,
        mapped: mibs
            .mapped
            .read()
            .map_err(|e| format!("stats.mapped: {e}"))? as u64,
    })
}

/// Spawn the background task that publishes jemalloc memory gauges every
/// five seconds into the context's metrics collector.
///
/// The collector is `ctx.metrics()` — the late-bound `MetricsHandle` coerced
/// to `Arc<dyn MetricsCollector>` — so emissions flow to whatever is
/// registered now or later (unwired handles silently no-op per ADR-0066).
/// If the MIBs cannot be resolved the sampler is disabled with a warning
/// instead of spawning a task that would fail every tick.
#[cfg(feature = "jemalloc")]
pub(crate) fn spawn_allocator_sampler(metrics: std::sync::Arc<dyn camel_api::MetricsCollector>) {
    if let Err(err) = mibs() {
        tracing::warn!(
            error = %err,
            "jemalloc stats MIB init failed; allocator sampler disabled"
        );
        return;
    }
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            tick.tick().await;
            let _ = emit_allocator_snapshot(read_jemalloc_snapshot, &metrics);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{AllocatorSnapshot, emit_allocator_snapshot};
    use camel_api::MetricsCollector;
    use camel_api::metrics::AllocatorStat;
    use std::sync::{Arc, Mutex};

    /// Recording double capturing `set_allocator_memory` calls.
    #[derive(Default)]
    struct RecordingCollector {
        allocator: Mutex<Vec<(AllocatorStat, u64)>>,
    }

    impl MetricsCollector for RecordingCollector {
        fn record_exchange_duration(&self, _route_id: &str, _duration: std::time::Duration) {}
        fn increment_errors(&self, _route_id: &str, _error_type: &str) {}
        fn increment_exchanges(&self, _route_id: &str) {}
        fn set_queue_depth(&self, _queue: &str, _depth: usize) {}
        fn record_circuit_breaker_change(&self, _route_id: &str, _from: &str, _to: &str) {}

        fn set_allocator_memory(&self, stat: AllocatorStat, bytes: u64) {
            self.allocator
                .lock()
                .expect("allocator lock")
                .push((stat, bytes));
        }
    }

    #[test]
    fn ok_snapshot_maps_to_four_exact_emissions() {
        let recorder = Arc::new(RecordingCollector::default());
        let metrics: Arc<dyn MetricsCollector> = recorder.clone();
        let read = || {
            Ok(AllocatorSnapshot {
                allocated: 11,
                resident: 22,
                active: 33,
                mapped: 44,
            })
        };

        let emitted = emit_allocator_snapshot(read, &metrics);

        assert!(emitted);
        let captures = recorder.allocator.lock().expect("allocator lock").clone();
        assert_eq!(
            captures,
            vec![
                (AllocatorStat::Allocated, 11),
                (AllocatorStat::Resident, 22),
                (AllocatorStat::Active, 33),
                (AllocatorStat::Mapped, 44),
            ]
        );
    }

    #[test]
    fn err_read_emits_nothing_and_returns_false() {
        let recorder = Arc::new(RecordingCollector::default());
        let metrics: Arc<dyn MetricsCollector> = recorder.clone();
        let read = || Err("epoch".to_string());

        let emitted = emit_allocator_snapshot(read, &metrics);

        assert!(!emitted);
        assert!(
            recorder
                .allocator
                .lock()
                .expect("allocator lock")
                .is_empty()
        );
    }

    #[test]
    fn unwired_handle_snapshot_is_silent_noop() {
        let metrics: Arc<dyn MetricsCollector> = Arc::new(camel_api::MetricsHandle::new());
        let read = || {
            Ok(AllocatorSnapshot {
                allocated: 1,
                resident: 2,
                active: 3,
                mapped: 4,
            })
        };

        let emitted = emit_allocator_snapshot(read, &metrics);

        assert!(emitted);
    }

    #[cfg(feature = "jemalloc")]
    #[test]
    fn real_read_closure_advances_epoch_and_returns_snapshot() {
        let snapshot = super::read_jemalloc_snapshot();
        assert!(
            snapshot.is_ok(),
            "real jemalloc read failed: {:?}",
            snapshot.err()
        );
    }
}
