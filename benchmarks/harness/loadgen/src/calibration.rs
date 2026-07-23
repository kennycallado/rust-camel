//! Calibration phase (spec §4.4) — Protocol A only.
//!
//! Send 200 requests total, doubling rate every 25 requests starting at
//! 10/sec → 20/sec → 40/sec → 80/sec → until 200 total OR backpressure.
//!
//! Backpressure definition (e_opus impl pin): HTTP 429/503 received,
//! connection refused, OR p99 > **100ms** (absolute threshold).
//!
//! Per-contender max sustainable rate is the last rate before the
//! threshold breach. The harness then takes **MIN across all 4
//! contenders** as the common measurement rate (fairness invariant).
//!
//! All calibration samples are DISCARDED — they do not contribute to
//! the per-round measurement statistics.

use crate::stats::nearest_rank_percentile;

/// Absolute p99 latency threshold for backpressure (spec §4.4).
///
/// 100ms in nanoseconds. If any 25-request batch exceeds this on p99,
/// the rate is considered unsustainable.
pub const BACKPRESSURE_P99_THRESHOLD_NS: u64 = 100_000_000;

/// Total calibration request budget (spec §4.4 verbatim).
pub const CALIBRATION_TOTAL_REQUESTS: u32 = 200;

/// Per-rate batch size: 25 requests per doubling step.
pub const CALIBRATION_BATCH_SIZE: u32 = 25;

/// Starting rate for the lowest step (requests/sec).
pub const CALIBRATION_START_RATE: u32 = 10;

/// Indicator that a sample in a calibration batch hit an HTTP 429/503
/// or connection failure. The [`calibration_phase`] function maps these
/// to [`Backpressure::HttpThrottled`] / [`Backpressure::ConnectionFailed`]
/// and the calibration driver in `cli_runtime::calibrate_async` `break`s
/// the rate-doubling loop on the first backpressure signal — per spec
/// §4.4, the max sustainable rate is the LAST rate that completed its
/// 25-request batch without backpressure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleStatus {
    /// 2xx response, latency recorded.
    Ok,
    /// 429 Too Many Requests OR 503 Service Unavailable — explicit
    /// server-side backpressure.
    Http429or503,
    /// Connection refused, reset, or other transport failure.
    ConnectionFailed,
}

/// A single calibration sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalibrationSample {
    pub latency_ns: u64,
    pub status: SampleStatus,
}

impl CalibrationSample {
    pub fn ok(latency_ns: u64) -> Self {
        Self {
            latency_ns,
            status: SampleStatus::Ok,
        }
    }
    pub fn http_throttled() -> Self {
        Self {
            latency_ns: 0,
            status: SampleStatus::Http429or503,
        }
    }
    pub fn conn_failed() -> Self {
        Self {
            latency_ns: 0,
            status: SampleStatus::ConnectionFailed,
        }
    }
}

/// How calibration detected backpressure (for diagnostics).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backpressure {
    /// p99 of the current batch exceeded 100ms.
    P99Threshold {
        rate_per_sec: u32,
        observed_p99_ns: u64,
    },
    /// HTTP 429/503 returned by the contender.
    HttpThrottled { rate_per_sec: u32 },
    /// Connection refused/reset.
    ConnectionFailed { rate_per_sec: u32 },
}

/// Outcome of a calibration phase run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalibrationOutcome {
    /// Last rate that completed its 25-request batch WITHOUT backpressure.
    /// `0` if even the starting rate (10/sec) could not sustain.
    pub max_sustainable_rate_per_sec: u32,
    /// Rate at which backpressure was first observed (None if all 200
    /// requests completed without backpressure).
    pub backpressure: Option<Backpressure>,
    /// Total requests actually sent (may be < 200 if backpressure hit
    /// mid-batch).
    pub total_requests_sent: u32,
    /// Per-rate-step batch p99s (for diagnostics + report).
    pub batch_p99s_ns: Vec<(u32, u64)>,
}

/// Decide the next rate step given the current one (spec §4.4: doubling).
///
/// 10 → 20 → 40 → 80 → 160 → 320 → ... This is rate doubling per 25
/// requests, but the total is capped at [`CALIBRATION_TOTAL_REQUESTS`].
pub fn next_rate(current: u32) -> u32 {
    current.saturating_mul(2)
}

/// Analyze one batch's samples and decide whether backpressure was hit.
///
/// A batch hits backpressure if ANY of:
/// - p99 of OK-sample latencies > [`BACKPRESSURE_P99_THRESHOLD_NS`]
/// - ANY sample is `Http429or503`
/// - ANY sample is `ConnectionFailed`
pub fn detect_backpressure(rate_per_sec: u32, batch: &[CalibrationSample]) -> Option<Backpressure> {
    // Transport-layer failures take precedence (most actionable signal).
    if batch
        .iter()
        .any(|s| s.status == SampleStatus::ConnectionFailed)
    {
        return Some(Backpressure::ConnectionFailed { rate_per_sec });
    }
    if batch.iter().any(|s| s.status == SampleStatus::Http429or503) {
        return Some(Backpressure::HttpThrottled { rate_per_sec });
    }
    // p99 on OK samples only.
    let ok_latencies: Vec<u64> = batch
        .iter()
        .filter_map(|s| {
            if s.status == SampleStatus::Ok {
                Some(s.latency_ns)
            } else {
                None
            }
        })
        .collect();
    if let Some(p99) = nearest_rank_percentile(&ok_latencies, 0.99) {
        if p99 > BACKPRESSURE_P99_THRESHOLD_NS {
            return Some(Backpressure::P99Threshold {
                rate_per_sec,
                observed_p99_ns: p99,
            });
        }
    }
    None
}

/// Pure reduction over a sequence of per-rate-step batches.
///
/// Inputs:
/// - `batches`: ordered list of (rate_per_sec, samples-in-that-batch).
///
/// Returns the [`CalibrationOutcome`] with max sustainable rate = the rate
/// of the last batch that did NOT trigger backpressure.
///
/// This function does NOT send any requests — it is pure and unit-
/// testable. The harness calls it after collecting batches.
pub fn calibration_phase(batches: &[(u32, Vec<CalibrationSample>)]) -> CalibrationOutcome {
    let mut max_sustainable: u32 = 0;
    let mut backpressure: Option<Backpressure> = None;
    let mut total_sent: u32 = 0;
    let mut p99s: Vec<(u32, u64)> = Vec::new();

    for &(rate, ref batch) in batches {
        total_sent = total_sent.saturating_add(batch.len() as u32);

        // Compute p99 for diagnostics, even on backpressure batches.
        let ok_latencies: Vec<u64> = batch
            .iter()
            .filter_map(|s| {
                if s.status == SampleStatus::Ok {
                    Some(s.latency_ns)
                } else {
                    None
                }
            })
            .collect();
        if let Some(p99) = nearest_rank_percentile(&ok_latencies, 0.99) {
            p99s.push((rate, p99));
        }

        if let Some(bp) = detect_backpressure(rate, batch) {
            backpressure = Some(bp);
            break;
        }
        max_sustainable = rate;
    }

    CalibrationOutcome {
        max_sustainable_rate_per_sec: max_sustainable,
        backpressure,
        total_requests_sent: total_sent,
        batch_p99s_ns: p99s,
    }
}

/// Compute the common measurement rate: MIN across contenders'
/// max-sustainable rates (spec §4.4 fairness invariant).
///
/// Returns `0` if any contender failed at even the starting rate (10/sec)
/// — the harness escalates such a cell to `won't-measure`.
pub fn common_measurement_rate(contender_rates: &[u32]) -> u32 {
    contender_rates.iter().copied().min().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_batch(rate: u32, count: usize, latency_ns: u64) -> (u32, Vec<CalibrationSample>) {
        (
            rate,
            (0..count)
                .map(|_| CalibrationSample::ok(latency_ns))
                .collect(),
        )
    }

    #[test]
    fn next_rate_doubles_starting_at_10() {
        assert_eq!(next_rate(10), 20);
        assert_eq!(next_rate(20), 40);
        assert_eq!(next_rate(40), 80);
        assert_eq!(next_rate(80), 160);
    }

    #[test]
    fn detect_backpressure_returns_none_for_fast_batch() {
        let batch: Vec<CalibrationSample> = (0..25)
            .map(|_| CalibrationSample::ok(5_000_000)) // 5ms each
            .collect();
        assert!(detect_backpressure(10, &batch).is_none());
    }

    #[test]
    fn detect_backpressure_at_p99_threshold_100ms() {
        // 25 samples, p99 = 24th-largest. If we make 24 of them 5ms and 1
        // of them 200ms, p99 = 200ms (the 24th largest after sort).
        let mut batch: Vec<CalibrationSample> =
            (0..24).map(|_| CalibrationSample::ok(5_000_000)).collect();
        batch.push(CalibrationSample::ok(200_000_000)); // 200ms
        let bp = detect_backpressure(40, &batch).unwrap();
        match bp {
            Backpressure::P99Threshold {
                rate_per_sec,
                observed_p99_ns,
            } => {
                assert_eq!(rate_per_sec, 40);
                assert_eq!(observed_p99_ns, 200_000_000);
            }
            _ => panic!("expected P99Threshold"),
        }
    }

    #[test]
    fn detect_backpressure_just_under_threshold_no_trigger() {
        // p99 = 95ms — under 100ms threshold.
        let mut batch: Vec<CalibrationSample> =
            (0..24).map(|_| CalibrationSample::ok(5_000_000)).collect();
        batch.push(CalibrationSample::ok(95_000_000)); // 95ms
        assert!(detect_backpressure(80, &batch).is_none());
    }

    #[test]
    fn detect_backpressure_http_429_triggers_immediately() {
        let batch = vec![
            CalibrationSample::ok(5_000_000),
            CalibrationSample::http_throttled(),
        ];
        let bp = detect_backpressure(20, &batch).unwrap();
        assert!(matches!(
            bp,
            Backpressure::HttpThrottled { rate_per_sec: 20 }
        ));
    }

    #[test]
    fn detect_backpressure_conn_failure_takes_precedence_over_http() {
        let batch = vec![
            CalibrationSample::http_throttled(),
            CalibrationSample::conn_failed(),
        ];
        let bp = detect_backpressure(40, &batch).unwrap();
        assert!(matches!(
            bp,
            Backpressure::ConnectionFailed { rate_per_sec: 40 }
        ));
    }

    #[test]
    fn calibration_phase_picks_max_sustainable_rate() {
        // Three batches: 10 (ok), 20 (ok), 40 (backpressure).
        let batches = vec![ok_batch(10, 25, 5_000_000), ok_batch(20, 25, 5_000_000), {
            // 40/sec batch with p99 = 200ms
            let mut b: Vec<CalibrationSample> =
                (0..24).map(|_| CalibrationSample::ok(5_000_000)).collect();
            b.push(CalibrationSample::ok(200_000_000));
            (40, b)
        }];
        let outcome = calibration_phase(&batches);
        assert_eq!(outcome.max_sustainable_rate_per_sec, 20);
        assert!(outcome.backpressure.is_some());
        assert_eq!(outcome.total_requests_sent, 75);
        assert_eq!(outcome.batch_p99s_ns.len(), 3);
    }

    #[test]
    fn calibration_phase_no_backpressure_returns_max_rate() {
        // All 200 requests across 8 batches of 25, no backpressure.
        let batches: Vec<(u32, Vec<CalibrationSample>)> = [10, 20, 40, 80, 160, 320, 640, 1280]
            .iter()
            .map(|&r| ok_batch(r, 25, 5_000_000))
            .collect();
        let outcome = calibration_phase(&batches);
        assert_eq!(outcome.max_sustainable_rate_per_sec, 1280);
        assert!(outcome.backpressure.is_none());
        assert_eq!(outcome.total_requests_sent, 200);
    }

    #[test]
    fn calibration_phase_first_batch_backpressure_yields_zero_sustainable() {
        let batches = vec![{
            let mut b: Vec<CalibrationSample> =
                (0..24).map(|_| CalibrationSample::ok(5_000_000)).collect();
            b.push(CalibrationSample::ok(200_000_000));
            (10, b)
        }];
        let outcome = calibration_phase(&batches);
        assert_eq!(outcome.max_sustainable_rate_per_sec, 0);
        assert!(matches!(
            outcome.backpressure,
            Some(Backpressure::P99Threshold { .. })
        ));
    }

    #[test]
    fn common_measurement_rate_is_min_across_contenders() {
        let rates = vec![640, 320, 160, 80];
        assert_eq!(common_measurement_rate(&rates), 80);
    }

    #[test]
    fn common_measurement_rate_zero_if_any_contender_zero() {
        // Fairness invariant: if ANY contender can't sustain even 10/sec,
        // the cell is escalated to won't-measure.
        let rates = vec![640, 320, 0, 80];
        assert_eq!(common_measurement_rate(&rates), 0);
    }

    #[test]
    fn common_measurement_rate_empty_is_zero() {
        assert_eq!(common_measurement_rate(&[]), 0);
    }

    #[test]
    fn calibration_threshold_is_exactly_100ms() {
        // Self-check on the constant — spec §4.4 verbatim.
        assert_eq!(BACKPRESSURE_P99_THRESHOLD_NS, 100_000_000);
    }

    #[test]
    fn calibration_total_request_budget_is_200() {
        assert_eq!(CALIBRATION_TOTAL_REQUESTS, 200);
    }

    #[test]
    fn calibration_batch_size_is_25() {
        assert_eq!(CALIBRATION_BATCH_SIZE, 25);
    }

    #[test]
    fn calibration_start_rate_is_10() {
        assert_eq!(CALIBRATION_START_RATE, 10);
    }
}
