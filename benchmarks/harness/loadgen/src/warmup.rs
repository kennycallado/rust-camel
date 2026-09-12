//! Warmup stability criterion (spec §4.3, trailing-window policy).
//!
//! Warmup is time-bounded: the driver collects samples until the
//! wall-clock bound expires. `max_messages` is NOT a termination cap —
//! it is the size of the trailing comparison window. All warmup samples
//! are discarded.
//!
//! **Stability criterion** (non-circular): warmup is stable when p50 of
//! the first half of the LATEST `max_messages` samples is within 10% of
//! p50 of the second half. Comparisons are WITHIN warmup, not
//! warmup-vs-measurement. Early drift (interpreter/JIT/cache warmup)
//! falls out of the trailing window as newer samples arrive.
//!
//! The criterion is evaluated exactly once, at the wall-clock deadline:
//! - Complete trailing window, halves agree → `Stable`.
//! - Complete trailing window, halves drift → `TimeBoundUnconverged`.
//! - Fewer than `max_messages` samples at the deadline →
//!   `InsufficientSamples` (rate too low to fill the window in time).
//!
//! If not stable, the cell FAILS with diagnostics (`✗ v3 —
//! failed-stability` in COVERAGE.md). `MessageBoundUnconverged` is kept
//! only for public/schema compatibility; Protocol A never emits it.

use crate::stats::median;

/// Warmup protocol configuration (spec §4.3 verbatim).
///
/// The brief fixes these values: 30s wall-clock bound, 1000-message
/// trailing window, 10% stability tolerance. They are exposed as a
/// struct so unit tests can exercise edge cases without recreating the
/// defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WarmupConfig {
    /// Maximum wall-clock seconds in warmup before timeout. The caller
    /// threads this value into `warmup_drive`'s deadline (`max_time`),
    /// so it is the single source of truth for the termination bound:
    /// the driver collects until that deadline expires. `warmup_drive`
    /// takes a `Duration` so tests can exercise sub-second deadlines
    /// without recreating the config.
    pub max_time_seconds: u32,
    /// Size of the trailing comparison window (latest N samples). Not a
    /// termination cap: collection continues past N messages until the
    /// wall-clock bound expires.
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

/// Warmup outcome after the wall-clock evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WarmupOutcome {
    /// Warmup reached the stability criterion. The cell can proceed to
    /// calibration/measurement.
    Stable {
        /// p50 of the first half of the latest trailing window.
        p50_first_half_ns: u64,
        /// p50 of the second half of the latest trailing window.
        p50_second_half_ns: u64,
        /// Total number of messages observed (may exceed the window size).
        messages_observed: u32,
        /// Wall-clock nanoseconds spent in warmup.
        elapsed_ns: u64,
    },
    /// Warmup did NOT reach the stability criterion within the wall-clock
    /// bound. Cell is marked `✗ v3 — failed-stability` (brief 2f: NO
    /// retry; deterministic failure mode).
    FailedStability {
        p50_first_half_ns: Option<u64>,
        p50_second_half_ns: Option<u64>,
        messages_observed: u32,
        elapsed_ns: u64,
        /// Why the warmup failed (time bound hit, not enough samples).
        reason: WarmupFailureReason,
    },
}

/// Reasons warmup can fail to reach stability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarmupFailureReason {
    /// Hit the message bound (1000) without converging.
    ///
    /// Retained only for public/schema compatibility: Protocol A never
    /// emits it, because `max_messages` is the trailing comparison-window
    /// size, not a termination cap.
    MessageBoundUnconverged,
    /// A complete trailing window was evaluated at the wall-clock bound
    /// and its halves drifted beyond tolerance.
    TimeBoundUnconverged,
    /// The wall-clock bound expired before one complete comparison
    /// window (`max_messages` samples) was collected (slow producer).
    InsufficientSamples,
}

/// Evaluate the within-warmup stability criterion on the trailing window
/// of warmup samples.
///
/// The caller drives warmup (sends requests until the wall-clock bound)
/// and supplies the accumulated samples + the wall-clock elapsed time.
/// This function is pure and unit-testable.
///
/// Per spec §4.3 (trailing-window policy), the comparison uses ONLY the
/// LATEST `cfg.max_messages` samples, split into two equal halves:
/// earlier samples fall out of scope once a full window of newer
/// samples exists. The caller evaluates exactly once, at the wall-clock
/// deadline:
///
/// - Complete window + halves within tolerance → `Stable`
///   (`elapsed_ns` is preserved verbatim, even past the configured
///   bound).
/// - Complete window + halves drifted → `TimeBoundUnconverged`.
/// - Fewer than `max_messages` samples → `InsufficientSamples`.
///
/// `MessageBoundUnconverged` is never returned: the message count is a
/// window size, not a termination bound.
pub fn check_warmup_stability(
    samples_ns: &[u64],
    elapsed_ns: u64,
    cfg: &WarmupConfig,
) -> WarmupOutcome {
    let n = samples_ns.len() as u32;
    let window_size = cfg.max_messages as usize;

    // Trailing window: the LATEST cfg.max_messages samples. Early drift
    // (process warmup) leaves the window as newer samples arrive.
    let window = if samples_ns.len() > window_size {
        &samples_ns[samples_ns.len() - window_size..]
    } else {
        samples_ns
    };

    // One complete comparison window needs all `max_messages` samples
    // (two halves of `max_messages / 2`). With fewer, no verdict is
    // possible: at the wall-clock deadline this is InsufficientSamples.
    if window.len() < window_size {
        return WarmupOutcome::FailedStability {
            p50_first_half_ns: median(window),
            p50_second_half_ns: None,
            messages_observed: n,
            elapsed_ns,
            reason: WarmupFailureReason::InsufficientSamples,
        };
    }

    let mid = window_size / 2;
    let first = &window[..mid];
    let second = &window[mid..];
    let p50_a = median(first).unwrap_or(0);
    let p50_b = median(second).unwrap_or(0);

    if within_tolerance(p50_a, p50_b, cfg.tolerance()) {
        return WarmupOutcome::Stable {
            p50_first_half_ns: p50_a,
            p50_second_half_ns: p50_b,
            messages_observed: n,
            elapsed_ns,
        };
    }

    // Complete window, unconverged. The driver evaluates exactly once,
    // at the wall-clock deadline, so an unconverged complete window is
    // always reported as TimeBoundUnconverged.
    WarmupOutcome::FailedStability {
        p50_first_half_ns: Some(p50_a),
        p50_second_half_ns: Some(p50_b),
        messages_observed: n,
        elapsed_ns,
        reason: WarmupFailureReason::TimeBoundUnconverged,
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
        // Complete trailing window: first half p50 = 1000ns, second half
        // p50 = 1200ns → 20% drift → unstable. The driver evaluates once
        // at the wall-clock bound, so an unconverged complete window is
        // TimeBoundUnconverged; Protocol A never emits
        // MessageBoundUnconverged (max_messages is the trailing-window
        // size, not a termination cap).
        let cfg = WarmupConfig::default();
        let mut samples = within(1000, 500);
        samples.extend(within(1200, 500));
        let out = check_warmup_stability(&samples, 1_000_000_000, &cfg);
        match out {
            WarmupOutcome::FailedStability { reason, .. } => {
                assert_eq!(reason, WarmupFailureReason::TimeBoundUnconverged);
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
    fn time_bound_insufficient_samples() {
        // Deadline arrives before one complete comparison window: only
        // 100 samples observed (< cfg.max_messages = 1000), so no
        // verdict is possible and the time bound reports
        // InsufficientSamples.
        let cfg = WarmupConfig::default();
        let samples = within(1000, 100);
        let out = check_warmup_stability(&samples, 30_000_000_000, &cfg);
        match out {
            WarmupOutcome::FailedStability {
                reason: WarmupFailureReason::InsufficientSamples,
                ..
            } => {}
            _ => panic!("expected InsufficientSamples"),
        }
    }

    #[test]
    fn time_bound_unconverged_trailing_window() {
        // One complete unstable trailing window evaluated at the
        // deadline: the verdict is TimeBoundUnconverged, never
        // MessageBoundUnconverged.
        let cfg = WarmupConfig::default();
        let mut samples = within(1000, 500);
        samples.extend(within(2000, 500)); // 100% drift
        let out = check_warmup_stability(&samples, 30_000_000_000, &cfg);
        match out {
            WarmupOutcome::FailedStability { reason, .. } => {
                assert_eq!(reason, WarmupFailureReason::TimeBoundUnconverged);
            }
            _ => panic!("expected FailedStability"),
        }
    }

    #[test]
    fn late_convergence_uses_trailing_window() {
        // 2,000 samples: the first 1,000 drift (early slow phase), the
        // latest 1,000 agree. The criterion must judge the LATEST
        // cfg.max_messages window, so late convergence is Stable and
        // the p50 fields are the latest-window halves. The pre-window
        // drift must not leak into the verdict.
        let cfg = WarmupConfig::default();
        let mut samples = within(1000, 500); // first window: half 1
        samples.extend(within(2000, 500)); // first window: half 2 → drifts
        samples.extend(within(1200, 1000)); // latest window: halves agree
        let out = check_warmup_stability(&samples, 30_000_000_000, &cfg);
        match out {
            WarmupOutcome::Stable {
                p50_first_half_ns,
                p50_second_half_ns,
                messages_observed,
                ..
            } => {
                assert_eq!(p50_first_half_ns, 1200);
                assert_eq!(p50_second_half_ns, 1200);
                assert_eq!(messages_observed, 2000);
            }
            _ => panic!("expected Stable from latest trailing window"),
        }
    }

    #[test]
    fn time_bound_stable_trailing_window() {
        // One complete stable trailing window evaluated past the
        // configured time bound: Stable, and elapsed_ns preserves the
        // ACTUAL elapsed time rather than the configured bound.
        let cfg = WarmupConfig::default();
        let mut samples = within(2000, 500);
        samples.extend(within(2050, 500)); // 2.5% drift → stable
        let out = check_warmup_stability(&samples, 31_500_000_000, &cfg);
        match out {
            WarmupOutcome::Stable { elapsed_ns, .. } => {
                assert_eq!(elapsed_ns, 31_500_000_000);
            }
            _ => panic!("expected Stable"),
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
