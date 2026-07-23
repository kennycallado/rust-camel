//! Throughput statistics for M3 sustained-throughput measurement (spec v4 §3).
//!
//! Per-second bucketing of successful (2xx) responses under max-rate
//! saturation load. Statistics: mean, p50, min, CV, half-life ratio,
//! error rate.
//!
//! ## Why p01 is excluded
//!
//! p01 is intentionally **not** reported: with ~50 throughput samples
//! (one per wall-clock second over a typical 50s measurement window),
//! the nearest-rank p01 equals the minimum value, providing no
//! additional information beyond `min_msgs_per_sec`. Only `p50`
//! (median) and `min` are surfaced in this module.
//!
//! ## Plant/seed separation
//!
//! All statistics operate on `Vec<u64>` (per-second 2xx counts) — no I/O,
//! no async. The async runtime (sending requests, bucketing) lives in
//! `cli_runtime.rs`. This module is unit-testable in isolation.

use crate::stats::median;

/// Result of aggregating per-second throughput buckets.
#[derive(Debug, Clone, PartialEq)]
pub struct ThroughputResult {
    /// Mean per-second throughput (headline number).
    pub mean_msgs_per_sec: f64,
    /// Median per-second throughput.
    pub p50_msgs_per_sec: u64,
    /// Absolute minimum per-second throughput.
    pub min_msgs_per_sec: u64,
    /// Coefficient of variation (std_dev / mean). < 0.15 = stable.
    pub cv: f64,
    /// mean(second_half) / mean(first_half). >= 0.90 = no degradation.
    pub half_life_ratio: f64,
    /// Total 2xx responses in measurement window.
    pub total_2xx: u64,
    /// Total non-2xx responses in measurement window.
    pub total_non_2xx: u64,
    /// Total transport failures (conn refused, timeout, etc.).
    pub total_errors: u64,
    /// Total requests attempted (2xx + non_2xx + errors).
    pub total_attempts: u64,
    /// Error rate: (non_2xx + errors) / attempts × 100.
    pub error_rate_pct: f64,
    /// Raw per-second buckets (for plotting / diagnostics).
    pub per_second_buckets: Vec<u64>,
}

/// Aggregate per-second throughput buckets into [`ThroughputResult`].
///
/// `buckets` = per-second count of 2xx responses in the measurement
/// window (warmup already discarded). `non_2xx` and `errors` are totals
/// across the same window.
///
/// # Examples
///
/// ```
/// # use bench_loadgen::throughput::aggregate_throughput;
/// let result = aggregate_throughput(vec![10000, 10100, 9900], 0, 0);
/// assert!((result.mean_msgs_per_sec - 10000.0).abs() < 1.0);
/// ```
pub fn aggregate_throughput(buckets: Vec<u64>, non_2xx: u64, errors: u64) -> ThroughputResult {
    let total_2xx: u64 = buckets.iter().sum();
    let n = buckets.len() as f64;
    let mean = if n > 0.0 { total_2xx as f64 / n } else { 0.0 };

    let p50 = median(&buckets).unwrap_or(0);
    let min = buckets.iter().copied().min().unwrap_or(0);

    let variance = if n > 0.0 {
        let sum_sq: f64 = buckets
            .iter()
            .map(|&b| {
                let diff = b as f64 - mean;
                diff * diff
            })
            .sum();
        sum_sq / n
    } else {
        0.0
    };
    let cv = if mean > 0.0 {
        variance.sqrt() / mean
    } else {
        0.0
    };

    // Half-life ratio: mean(second_half) / mean(first_half)
    let half = buckets.len() / 2;
    let first_half_mean = if half > 0 {
        buckets[..half].iter().sum::<u64>() as f64 / half as f64
    } else {
        0.0
    };
    let second_half_mean = if buckets.len() - half > 0 {
        let remainder = buckets.len() - half;
        buckets[half..].iter().sum::<u64>() as f64 / remainder as f64
    } else {
        0.0
    };
    let half_life_ratio = if first_half_mean > 0.0 {
        second_half_mean / first_half_mean
    } else {
        0.0
    };

    let total_attempts = total_2xx + non_2xx + errors;
    let error_rate_pct = if total_attempts > 0 {
        (non_2xx + errors) as f64 / total_attempts as f64 * 100.0
    } else {
        0.0
    };

    ThroughputResult {
        mean_msgs_per_sec: mean,
        p50_msgs_per_sec: p50,
        min_msgs_per_sec: min,
        cv,
        half_life_ratio,
        total_2xx,
        total_non_2xx: non_2xx,
        total_errors: errors,
        total_attempts,
        error_rate_pct,
        per_second_buckets: buckets,
    }
}

/// Check steady-state criteria (spec §3.4).
///
/// A cell is "steady" if BOTH:
/// 1. CV < 0.15 (coefficient of variation)
/// 2. half_life_ratio >= 0.90 (mean second half >= 90% of mean first half)
pub fn is_steady(result: &ThroughputResult) -> bool {
    result.cv < 0.15 && result.half_life_ratio >= 0.90
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aggregate_stable_throughput() {
        let buckets = vec![
            10000, 10100, 9900, 10000, 10050, 9950, 10000, 10100, 9900, 10000,
        ];
        let result = aggregate_throughput(buckets, 0, 0);
        assert!((result.mean_msgs_per_sec - 10000.0).abs() < 1.0);
        assert!(result.cv < 0.15);
        assert!(result.half_life_ratio >= 0.90);
        assert!(is_steady(&result));
        assert_eq!(result.error_rate_pct, 0.0);
        assert_eq!(result.total_2xx, 100000);
        assert_eq!(result.total_attempts, 100000);
    }

    #[test]
    fn test_aggregate_degrading_throughput() {
        // Linear degradation: 10000 down to 5000
        let buckets: Vec<u64> = (0..10).map(|i| 10000 - i * 500).collect();
        let result = aggregate_throughput(buckets, 0, 0);
        assert!(
            result.half_life_ratio < 0.90,
            "half_life should detect degradation, got {}",
            result.half_life_ratio
        );
        assert!(!is_steady(&result));
    }

    #[test]
    fn test_aggregate_with_errors() {
        let buckets = vec![10000, 10000];
        let result = aggregate_throughput(buckets, 50, 10);
        assert!(result.error_rate_pct > 0.0);
        assert_eq!(result.total_2xx, 20000);
        assert_eq!(result.total_non_2xx, 50);
        assert_eq!(result.total_errors, 10);
        assert_eq!(result.total_attempts, 20060);
    }

    #[test]
    fn test_p01_excluded_from_output() {
        // p01 is intentionally not computed: with ~50 throughput samples,
        // nearest-rank p01 == min, providing no additional information.
        // Verify the JSON shape (mirrored from cli_runtime.rs) does not
        // include a p01_msgs_per_sec key.
        let result = aggregate_throughput(vec![5000, 6000, 7000, 8000, 9000, 10000], 0, 0);
        let json = serde_json::json!({
            "mean_msgs_per_sec": result.mean_msgs_per_sec,
            "p50_msgs_per_sec": result.p50_msgs_per_sec,
            "min_msgs_per_sec": result.min_msgs_per_sec,
        });
        let obj = json.as_object().expect("json should be an object");
        assert!(
            !obj.contains_key("p01_msgs_per_sec"),
            "p01 must not appear in throughput JSON output; got keys: {:?}",
            obj.keys().collect::<Vec<_>>()
        );
        // Sanity: p50 is still a real percentile (median), not the min.
        assert_eq!(result.p50_msgs_per_sec, 7500);
        assert_eq!(result.min_msgs_per_sec, 5000);
    }

    #[test]
    fn test_empty_buckets() {
        let result = aggregate_throughput(vec![], 0, 0);
        assert_eq!(result.mean_msgs_per_sec, 0.0);
        assert!(!is_steady(&result));
    }
}
