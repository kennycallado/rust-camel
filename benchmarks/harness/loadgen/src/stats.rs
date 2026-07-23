//! Statistics helpers (spec §4.2).
//!
//! - [`percentile`] — linear-interpolation percentile (matches numpy's
//!   default `'linear'` method). Used for bootstrap-resampled distributions
//!   where interpolation is appropriate.
//! - [`nearest_rank_percentile`] — nearest-rank (ceil(p·n)), used for the
//!   raw per-round p50/p95/p99 to match the v1 harness's small-sample
//!   estimator choice (CONTEXT.md §2: "nearest-rank is the standard for
//!   small-sample percentiles").
//! - [`median`] — standard median.
//! - [`baseline_corrected_p50`] — first-order location correction
//!   (spec §4.4): subtract baseline p50 from contender p50. Per the §4.4
//!   authoritative version, this is the ONLY baseline-subtraction allowed;
//!   higher percentiles are published raw with baseline alongside.
//!
//! All helpers operate on `&[u64]` (nanosecond-resolution samples, the
//! canonical form recorded by the load generator and parsed from the
//! Protocol B tmpfs log). They do NOT mutate their input — callers sort
//! when needed.

/// Standard median. Empty input returns None.
///
/// For even n, returns the mean of the two middle elements (matches numpy
/// median). Half-integer results are rounded to the nearest representable
/// u64 — the consumer (BCa CI) treats the result as a point estimate and
/// resamples, so rounding bias is below noise.
pub fn median(samples: &[u64]) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted: Vec<u64> = samples.to_vec();
    sorted.sort_unstable();
    let n = sorted.len();
    let mid = n / 2;
    Some(if n % 2 == 1 {
        sorted[mid]
    } else {
        // (a + b) / 2 with rounding — both fit in u64, sum fits in u64.
        sorted[mid - 1].wrapping_add(sorted[mid]) / 2
    })
}

/// Nearest-rank percentile: index = ceil(p·n), 1-based, clamped to [1, n].
///
/// Matches v1 harness summarize() in benchmarks/harness/run.sh. For p=0.95,
/// n=50 → idx=48 (the 48th of 50 sorted samples). For n=10000, p=0.99 →
/// idx=9900.
pub fn nearest_rank_percentile(samples: &[u64], p: f64) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted: Vec<u64> = samples.to_vec();
    sorted.sort_unstable();
    nearest_rank_from_sorted(&sorted, p)
}

/// Same as [`nearest_rank_percentile`] but operates on a slice already
/// sorted ascending. Used internally to avoid re-sorting during bootstrap.
pub fn nearest_rank_from_sorted(sorted: &[u64], p: f64) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    let n = sorted.len();
    // ceil(p * n), 1-based; saturating sub at the end converts to 0-based.
    let idx_f = p * (n as f64);
    let mut idx = idx_f.ceil() as usize;
    if idx > n {
        idx = n;
    }
    if idx == 0 {
        idx = 1;
    }
    Some(sorted[idx - 1])
}

/// Linear-interpolation percentile (numpy `'linear'`).
///
/// Used inside the BCa bootstrap where we want a smooth statistic so the
/// resample distribution is well-behaved. For p50 this is identical to
/// [`median`].
pub fn percentile(samples: &[u64], p: f64) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted: Vec<u64> = samples.to_vec();
    sorted.sort_unstable();
    percentile_from_sorted(&sorted, p)
}

/// Linear-interpolation percentile on a pre-sorted slice.
pub fn percentile_from_sorted(sorted: &[u64], p: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let n = sorted.len();
    if n == 1 {
        return Some(sorted[0] as f64);
    }
    // Clamp p to [0, 1].
    let p = p.clamp(0.0, 1.0);
    let rank = p * (n as f64 - 1.0);
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        return Some(sorted[lo] as f64);
    }
    let frac = rank - (lo as f64);
    let v = (sorted[lo] as f64) * (1.0 - frac) + (sorted[hi] as f64) * frac;
    Some(v)
}

/// Headline aggregator: median of per-round p99 values (spec §4.2).
///
/// Preserves process-launch heterogeneity that pooled-samples would hide
/// (e_gpt blessing round 1). Returns the median of the input p99s.
pub fn median_of_round_p99s(round_p99s: &[u64]) -> Option<u64> {
    median(round_p99s)
}

/// Baseline-corrected p50 (spec §4.4).
///
/// Returns `contender_p50 - baseline_p50`, saturating at zero (a contender
/// faster than loopback baseline is reported as 0 + a flag in the report
/// narrative, not a negative number).
///
/// **Spec contradiction #1 applied here** (per plan §"Spec contradictions
/// already resolved"): higher percentiles are published raw with baseline
/// shown alongside, NOT subtracted percentile-by-percentile. Only p50
/// location uses this correction.
pub fn baseline_corrected_p50(contender_p50_ns: u64, baseline_p50_ns: u64) -> u64 {
    contender_p50_ns.saturating_sub(baseline_p50_ns)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_odd_returns_middle() {
        assert_eq!(median(&[3, 1, 2]), Some(2));
    }

    #[test]
    fn median_even_returns_mean_of_middle_pair() {
        // sorted: [1, 2, 3, 4] → (2+3)/2 = 2
        // (integer division of 5/2 = 2)
        assert_eq!(median(&[4, 1, 3, 2]), Some(2));
    }

    #[test]
    fn median_empty_returns_none() {
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn median_single_element() {
        assert_eq!(median(&[42]), Some(42));
    }

    #[test]
    fn nearest_rank_p95_n50_is_48th_sorted() {
        // v1 harness contract: n=50, p95 → idx=48
        let samples: Vec<u64> = (1..=50).collect();
        assert_eq!(nearest_rank_percentile(&samples, 0.95), Some(48));
    }

    #[test]
    fn nearest_rank_p99_n10000_is_9900th() {
        let samples: Vec<u64> = (1..=10000).collect();
        assert_eq!(nearest_rank_percentile(&samples, 0.99), Some(9900));
    }

    #[test]
    fn nearest_rank_p50_n4_is_2nd() {
        // ceil(0.5 * 4) = 2
        let samples: Vec<u64> = vec![10, 20, 30, 40];
        assert_eq!(nearest_rank_percentile(&samples, 0.50), Some(20));
    }

    #[test]
    fn nearest_rank_empty_returns_none() {
        assert_eq!(nearest_rank_percentile(&[], 0.95), None);
    }

    #[test]
    fn percentile_p50_matches_median() {
        let samples = vec![10, 20, 30, 40];
        let pct = percentile(&samples, 0.50).unwrap();
        // median of even-length: (20+30)/2 = 25.0
        assert_eq!(pct, 25.0);
        assert_eq!(median(&samples), Some(25));
    }

    #[test]
    fn percentile_p25_quartile_interpolates() {
        // numpy linear: p=0.25 on [10,20,30,40] → rank 0.75 → 10 + 0.75*(20-10) = 17.5
        let samples = vec![10, 20, 30, 40];
        let pct = percentile(&samples, 0.25).unwrap();
        assert!((pct - 17.5).abs() < 1e-9);
    }

    #[test]
    fn percentile_p0_is_min_p1_is_max() {
        let samples = vec![10, 50, 20, 40, 30];
        assert_eq!(percentile(&samples, 0.0).unwrap(), 10.0);
        assert_eq!(percentile(&samples, 1.0).unwrap(), 50.0);
    }

    #[test]
    fn baseline_corrected_p50_subtracts_loopback_overhead() {
        // Contender p50 = 2ms, baseline p50 = 0.5ms → corrected = 1.5ms
        let c = 2_000_000u64;
        let b = 500_000u64;
        assert_eq!(baseline_corrected_p50(c, b), 1_500_000);
    }

    #[test]
    fn baseline_corrected_p50_saturates_when_contender_faster_than_baseline() {
        // A contender faster than loopback is suspicious but reported as 0
        // (negative latency is meaningless).
        assert_eq!(baseline_corrected_p50(100, 500), 0);
    }

    #[test]
    fn median_of_round_p99s_picks_middle_of_5() {
        // 5 rounds → median = 3rd-highest sorted
        let p99s = vec![100, 300, 200, 500, 400];
        assert_eq!(median_of_round_p99s(&p99s), Some(300));
    }

    #[test]
    fn median_of_round_p99s_empty_returns_none() {
        assert_eq!(median_of_round_p99s(&[]), None);
    }
}
