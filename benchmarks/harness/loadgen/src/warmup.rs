//! Warmup stability criterion (spec §4.3).
//!
//! Warmup is time-bounded AND message-bounded, whichever first:
//! - 30 seconds OR 1,000 messages, whichever first.
//! - All warmup samples discarded.
//!
//! **Stability criterion** (non-circular): warmup complete when p50 of
//! messages 501–1000 is within 10% of p50 of messages 1–500. Comparisons
//! are WITHIN warmup, not warmup-vs-measurement.
//!
//! If not stable after 1,000 messages (or 30s), the cell FAILS with
//! diagnostics (`✗ v3 — failed-stability` in COVERAGE.md).

use crate::stats::median;

/// Warmup protocol configuration (spec §4.3 verbatim).
///
/// The brief fixes these values: 30s OR 1000 messages, 10% stability
/// tolerance. They are exposed as a struct so unit tests can exercise
/// edge cases without recreating the defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WarmupConfig {
    /// Maximum wall-clock seconds in warmup before timeout.
    pub max_time_seconds: u32,
    /// Maximum messages before warmup terminates.
    pub max_messages: u32,
    /// Tolerance for the p50 comparison: |p50_b - p50_a| <= tolerance * p50_a.
    /// Stored as parts-per-thousand (10% = 100) to keep this `Eq`.
    pub tolerance_ppth: u32,
}

impl Default for WarmupConfig {
    fn default() -> Self {
        Self {
            max_time_seconds: 30,
            max_messages: 1000,
            tolerance_ppth: 100, // 10% = 100 parts per thousand
        }
    }
}

impl WarmupConfig {
    /// Tolerance as a fraction (e.g., 0.10 for 10%).
    pub fn tolerance(&self) -> f64 {
        self.tolerance_ppth as f64 / 1000.0
    }
}

/// Warmup outcome after the configured window elapses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WarmupOutcome {
    /// Warmup reached the stability criterion. The cell can proceed to
    /// calibration/measurement.
    Stable {
        /// p50 of messages 1–500 (first half).
        p50_first_half_ns: u64,
        /// p50 of messages 501–1000 (second half).
        p50_second_half_ns: u64,
        /// Number of messages observed when stability was declared.
        messages_observed: u32,
        /// Wall-clock nanoseconds spent in warmup.
        elapsed_ns: u64,
    },
    /// Warmup did NOT reach the stability criterion within the time/message
    /// bounds. Cell is marked `✗ v3 — failed-stability` (brief 2f: NO
    /// retry; deterministic failure mode).
    FailedStability {
        p50_first_half_ns: Option<u64>,
        p50_second_half_ns: Option<u64>,
        messages_observed: u32,
        elapsed_ns: u64,
        /// Why the warmup failed (time bound hit, message bound hit, etc.).
        reason: WarmupFailureReason,
    },
}

/// Reasons warmup can fail to reach stability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarmupFailureReason {
    /// Hit the message bound (1000) without converging.
    MessageBoundUnconverged,
    /// Hit the time bound (30s) without converging.
    TimeBoundUnconverged,
    /// Fewer than 1000 messages observed AND time bound hit (slow producer).
    InsufficientSamples,
}

/// Evaluate the within-warmup stability criterion on a batch of warmup
/// samples.
///
/// The caller drives warmup (sends requests / observes ticks) and supplies
/// the accumulated samples + the wall-clock elapsed time. This function
/// is pure and unit-testable.
///
/// Per spec §4.3, the comparison is **within** warmup: messages 1–500 vs
/// messages 501–1000. If both halves have a p50 (i.e., n >= 1000 OR a
/// half-boundary was reached), and they agree to within tolerance, the
/// warmup is Stable. Otherwise it is FailedStability.
///
/// The time bound is enforced by the caller; if `elapsed_ns` exceeds the
/// configured max, the warmup is forced to fail with `TimeBoundUnconverged`
/// or `InsufficientSamples` (depending on whether the message bound was
/// reached first).
pub fn check_warmup_stability(
    samples_ns: &[u64],
    elapsed_ns: u64,
    cfg: &WarmupConfig,
) -> WarmupOutcome {
    let n = samples_ns.len() as u32;
    let time_max_ns = (cfg.max_time_seconds as u64).saturating_mul(1_000_000_000);
    let time_exceeded = elapsed_ns >= time_max_ns;
    let message_bound_reached = n >= cfg.max_messages;

    // Split at the midpoint of cfg.max_messages: first half = 1..mid,
    // second half = mid+1..end (or ..max_messages if more were collected).
    let mid = cfg.max_messages / 2; // 500 for default 1000

    // If we have fewer than `mid + 1` samples, we cannot compute a second-
    // half p50. Time bound is the only way to terminate.
    if (n as usize) < (mid as usize) + 1 {
        if time_exceeded {
            return WarmupOutcome::FailedStability {
                p50_first_half_ns: median(&samples_ns[..n as usize]),
                p50_second_half_ns: None,
                messages_observed: n,
                elapsed_ns,
                reason: WarmupFailureReason::InsufficientSamples,
            };
        }
        // Not enough samples yet, no timeout — caller should keep going.
        // This is not a terminal state; return FailedStability so the
        // caller sees an explicit outcome. In practice the harness
        // loops until n == max_messages or time runs out.
        return WarmupOutcome::FailedStability {
            p50_first_half_ns: median(samples_ns),
            p50_second_half_ns: None,
            messages_observed: n,
            elapsed_ns,
            reason: WarmupFailureReason::InsufficientSamples,
        };
    }

    let first = &samples_ns[..mid as usize];
    // Second half is `mid` samples (or however many we have, capped at mid).
    let second_end = (n as usize).min(cfg.max_messages as usize);
    let second = &samples_ns[mid as usize..second_end];
    let p50_a = median(first).unwrap_or(0);
    let p50_b = median(second).unwrap_or(0);

    let stable = within_tolerance(p50_a, p50_b, cfg.tolerance());

    if stable {
        return WarmupOutcome::Stable {
            p50_first_half_ns: p50_a,
            p50_second_half_ns: p50_b,
            messages_observed: n,
            elapsed_ns,
        };
    }

    let reason = if message_bound_reached {
        WarmupFailureReason::MessageBoundUnconverged
    } else if time_exceeded {
        WarmupFailureReason::TimeBoundUnconverged
    } else {
        // Not stable, not bound — caller should keep going. Mark as
        // unconverged-so-far; the harness treats this as a continuing
        // state (loop until bound). For unit-test purposes we surface
        // the unconverged state.
        WarmupFailureReason::MessageBoundUnconverged
    };

    WarmupOutcome::FailedStability {
        p50_first_half_ns: Some(p50_a),
        p50_second_half_ns: Some(p50_b),
        messages_observed: n,
        elapsed_ns,
        reason,
    }
}

/// Within-tolerance check: |a - b| / max(a, 1) <= tolerance.
///
/// Uses `max(a, 1)` as the denominator to avoid div-by-zero when the
/// first-half p50 is 0 (theoretically impossible for latency, but
/// defensively handled).
fn within_tolerance(a: u64, b: u64, tolerance: f64) -> bool {
    let denom = a.max(1) as f64;
    let diff = a.abs_diff(b) as f64;
    (diff / denom) <= tolerance
}

#[cfg(test)]
mod tests {
    use super::*;

    fn within(v: u64, base: u64) -> Vec<u64> {
        // Generate a uniform batch of `v` repeated; lets us produce
        // known-p50 groups deterministically.
        vec![v; base as usize]
    }

    #[test]
    fn stable_when_both_halves_agree_within_10pct() {
        // First half p50 = 1000ns, second half p50 = 1080ns → 8% drift → stable.
        let cfg = WarmupConfig::default();
        let mut samples = within(1000, 500);
        samples.extend(within(1080, 500));
        let out = check_warmup_stability(&samples, 1_000_000_000, &cfg);
        assert!(matches!(out, WarmupOutcome::Stable { .. }));
    }

    #[test]
    fn fails_when_second_half_drifts_beyond_10pct() {
        // First half p50 = 1000ns, second half p50 = 1200ns → 20% drift → unstable.
        let cfg = WarmupConfig::default();
        let mut samples = within(1000, 500);
        samples.extend(within(1200, 500));
        let out = check_warmup_stability(&samples, 1_000_000_000, &cfg);
        match out {
            WarmupOutcome::FailedStability { reason, .. } => {
                assert_eq!(reason, WarmupFailureReason::MessageBoundUnconverged);
            }
            _ => panic!("expected FailedStability"),
        }
    }

    #[test]
    fn fails_when_first_half_zero_drift_handled() {
        // Defensive: 0 first-half p50 should not divide by zero.
        let cfg = WarmupConfig::default();
        let mut samples = within(0, 500);
        samples.extend(within(100, 500));
        // |100 - 0| / max(0,1) = 100 → > 10% → unstable
        let out = check_warmup_stability(&samples, 1_000_000_000, &cfg);
        assert!(matches!(out, WarmupOutcome::FailedStability { .. }));
    }

    #[test]
    fn insufficient_samples_when_time_exceeded_before_half_bound() {
        let cfg = WarmupConfig::default();
        // Only 100 samples (< mid+1=501), time exceeded.
        let samples = within(1000, 100);
        let out = check_warmup_stability(&samples, 31_000_000_000, &cfg);
        match out {
            WarmupOutcome::FailedStability {
                reason: WarmupFailureReason::InsufficientSamples,
                ..
            } => {}
            _ => panic!("expected InsufficientSamples"),
        }
    }

    #[test]
    fn time_bound_unconverged_after_full_sample_window() {
        // 1000 samples, drifted beyond tolerance, but time bound hit.
        let cfg = WarmupConfig::default();
        let mut samples = within(1000, 500);
        samples.extend(within(2000, 500)); // 100% drift
        let out = check_warmup_stability(&samples, 35_000_000_000, &cfg);
        // Both bounds reached — message bound reached, so MessageBoundUnconverged
        // takes precedence.
        match out {
            WarmupOutcome::FailedStability { reason, .. } => {
                assert_eq!(reason, WarmupFailureReason::MessageBoundUnconverged);
            }
            _ => panic!("expected FailedStability"),
        }
    }

    #[test]
    fn exactly_at_tolerance_boundary_is_stable() {
        // |b - a| / a == tolerance exactly (10%): the criterion is "<=".
        let cfg = WarmupConfig::default();
        let mut samples = within(1000, 500);
        samples.extend(within(1100, 500)); // exactly 10%
        let out = check_warmup_stability(&samples, 1_000_000_000, &cfg);
        assert!(matches!(out, WarmupOutcome::Stable { .. }));
    }

    #[test]
    fn stable_after_full_1000_message_window() {
        let cfg = WarmupConfig::default();
        let mut samples = within(2000, 500);
        samples.extend(within(2050, 500)); // 2.5% drift → stable
        let out = check_warmup_stability(&samples, 5_000_000_000, &cfg);
        match out {
            WarmupOutcome::Stable {
                messages_observed, ..
            } => {
                assert_eq!(messages_observed, 1000);
            }
            _ => panic!("expected Stable"),
        }
    }

    #[test]
    fn second_half_lower_drift_within_tolerance_is_stable() {
        // First half p50 = 1000, second half p50 = 950 → 5% drift (lower) → stable.
        let cfg = WarmupConfig::default();
        let mut samples = within(1000, 500);
        samples.extend(within(950, 500));
        let out = check_warmup_stability(&samples, 1_000_000_000, &cfg);
        assert!(matches!(out, WarmupOutcome::Stable { .. }));
    }
}
