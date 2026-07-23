//! Protocol B parsing + aggregation (spec §4.1, §4.2, brief 2d).
//!
//! Protocol B = service-time measurement for timer-driven routes (T1,
//! T2, T4a, T4b). The contender emits per-tick latency records to a
//! tmpfs file (`/tmp/v3-protocol-b-<cell>.log`) in the format:
//!
//! ```text
//! BENCH_LATENCY <tick_id> <latency_ns>
//! ```
//!
//! The harness parses this file post-run (after the contender process
//! has exited and flushed), aggregates per-round and across rounds, and
//! reports median-of-per-round-p99 as the headline aggregator.
//!
//! **Timer period held constant at 10ms across all contenders** (spec
//! §4.4 authoritative version, contradicting spec §5 risk-table row 9).
//! Saturation escalates via `won't-measure`, not period adaptation.
//!
//! The Protocol B log records `<latency_ns>` in **nanoseconds** directly
//! (the fixture emits an absolute duration per tick; the parser applies
//! no rescaling). Any record that is non-integer, `<= 0`, or exceeds the
//! 30-second deadline (`> 30_000_000_000`) invalidates the whole round
//! (hard abort, no partial percentile computation).

use crate::stats::{median, nearest_rank_percentile};

/// Per-record parse error. `MissingTokens`, `InvalidInteger`, and
/// `OutOfBound` invalidate the round (the parser aborts); a non-BENCH_LATENCY
/// line (`NotALatencyRecord`) is ignored as irrelevant noise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// Line does not begin with `BENCH_LATENCY `.
    NotALatencyRecord { line: String },
    /// Missing tick_id or latency_ns token.
    MissingTokens { line: String },
    /// tick_id or latency_ns failed to parse as integer.
    InvalidInteger { line: String },
    /// latency_ns is `<= 0` or exceeds the 30-second deadline
    /// (`> 30_000_000_000`). `value` is the raw parsed i64.
    OutOfBound { value: i64, line: String },
}

/// Parsed Protocol B record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LatencyRecord {
    pub tick_id: u64,
    pub latency_ns: u64,
}

/// Per-round Protocol B aggregation result.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProtocolBRoundStats {
    pub n_samples: usize,
    pub p50_ns: u64,
    pub p90_ns: u64,
    pub p95_ns: u64,
    pub p99_ns: u64,
    /// True if this round was aborted by a malformed/out-of-bound record.
    pub is_invalidated: bool,
    /// Human-readable reason when `is_invalidated` is true (empty otherwise).
    pub invalidated_reason: String,
}

/// Aggregate outcome across all rounds (median-of-round-p99s headline).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProtocolBOutcome {
    /// Headline aggregator: median of per-round p99 (spec §4.2).
    pub median_p99_ns: u64,
    /// All 5 per-round p99 values (for dispersion reporting).
    pub round_p99s_ns: Vec<u64>,
    /// Per-round stats (n, p50, p95, p99).
    pub per_round: Vec<ProtocolBRoundStats>,
    /// Number of malformed/out-of-bound records encountered (0 or 1 — the
    /// parser aborts on the first such record).
    pub malformed_records: usize,
    /// Total samples parsed across rounds.
    pub total_samples: usize,
    /// True if ANY round was invalidated — consumers MUST discard the
    /// percentiles and treat the run as a hard failure.
    pub is_invalidated: bool,
    /// Reason for the first invalidation (empty if none).
    pub invalidated_reason: String,
}

/// Parse a single Protocol B log line.
///
/// Format: `BENCH_LATENCY <tick_id> <latency_ns>`. Field 2 is treated as
/// a duration in **nanoseconds** (no rescaling). The value is parsed as
/// `i64` (so negative values are detectable), bounds-checked against the
/// 30-second deadline, then cast to `u64`.
///
/// Bounds: `latency_ns` must satisfy `0 < latency_ns <= 30_000_000_000`.
/// Values `<= 0` or `> 30s` yield `ParseError::OutOfBound`.
#[allow(clippy::cast_possible_wrap)]
pub fn parse_line(line: &str) -> Result<LatencyRecord, ParseError> {
    let trimmed = line.trim();
    if !trimmed.starts_with("BENCH_LATENCY ") {
        return Err(ParseError::NotALatencyRecord {
            line: line.to_string(),
        });
    }
    let mut tokens = trimmed.split_whitespace();
    let _ = tokens.next(); // "BENCH_LATENCY"
    let tick_s = tokens.next().ok_or(ParseError::MissingTokens {
        line: line.to_string(),
    })?;
    let lat_s = tokens.next().ok_or(ParseError::MissingTokens {
        line: line.to_string(),
    })?;
    let tick_id: u64 = tick_s.parse().map_err(|_| ParseError::InvalidInteger {
        line: line.to_string(),
    })?;
    // Parse as i64 so negative durations are detectable (and rejected).
    let raw: i64 = lat_s.parse().map_err(|_| ParseError::InvalidInteger {
        line: line.to_string(),
    })?;
    const MAX_DURATION_NS: i64 = 30_000_000_000; // 30 seconds (spec deadline)
    if raw <= 0 || raw > MAX_DURATION_NS {
        return Err(ParseError::OutOfBound {
            value: raw,
            line: line.to_string(),
        });
    }
    // Safety of the cast: raw > 0 is guaranteed above, so it fits in u64.
    let latency_ns: u64 = raw as u64;
    Ok(LatencyRecord {
        tick_id,
        latency_ns,
    })
}

/// Parse a full Protocol B log (multiple rounds, each terminated by a
/// blank line OR end-of-file).
///
/// Round boundaries: a blank line in the log file marks the end of a
/// round. If the file has no blank-line separators, the entire file is
/// treated as a single round.
///
/// **Invalidation contract (spec F1):** any record that is non-integer,
/// `<= 0`, or `> 30s` (`MissingTokens` / `InvalidInteger` / `OutOfBound`)
/// invalidates the round — the parser records the reason on the current
/// round and ABORTS further processing (no partial percentile
/// computation). Non-`BENCH_LATENCY` lines (`NotALatencyRecord`) are
/// ignored as irrelevant noise. The outcome-level `is_invalidated` flag
/// is set if any round was invalidated.
pub fn parse_protocol_b_log(content: &str) -> ProtocolBOutcome {
    /// Per-round collector during parsing.
    #[derive(Default)]
    struct RoundCollector {
        samples: Vec<u64>,
        is_invalidated: bool,
        invalidated_reason: String,
    }
    impl RoundCollector {
        fn new() -> Self {
            RoundCollector {
                samples: Vec::new(),
                is_invalidated: false,
                invalidated_reason: String::new(),
            }
        }
    }

    let mut rounds: Vec<RoundCollector> = Vec::new();
    let mut current = RoundCollector::new();
    let mut malformed = 0usize;
    let mut total = 0usize;
    let mut outcome_invalidated = false;
    let mut outcome_reason = String::new();

    'outer: for line in content.lines() {
        if line.trim().is_empty() {
            // Blank-line separator: close current round if it has data or
            // was invalidated.
            if !current.samples.is_empty() || current.is_invalidated {
                rounds.push(std::mem::take(&mut current));
            }
            continue;
        }
        match parse_line(line) {
            Ok(rec) => {
                current.samples.push(rec.latency_ns);
                total += 1;
            }
            Err(ParseError::NotALatencyRecord { .. }) => {
                // Non-BENCH_LATENCY line (other markers/comments): ignore.
            }
            Err(e) => {
                // MissingTokens | InvalidInteger | OutOfBound → invalidate.
                malformed += 1;
                let reason = format!("malformed/out-of-bound record: {e:?}");
                current.is_invalidated = true;
                current.invalidated_reason = reason.clone();
                outcome_invalidated = true;
                outcome_reason = reason;
                break 'outer;
            }
        }
    }
    // Flush a trailing round (no blank-line terminator).
    if !current.samples.is_empty() || current.is_invalidated {
        rounds.push(current);
    }

    let mut per_round: Vec<ProtocolBRoundStats> = Vec::new();
    let mut round_p99s: Vec<u64> = Vec::new();
    for r in &rounds {
        if r.is_invalidated {
            // No partial percentile computation on an invalidated round.
            per_round.push(ProtocolBRoundStats {
                n_samples: r.samples.len(),
                p50_ns: 0,
                p90_ns: 0,
                p95_ns: 0,
                p99_ns: 0,
                is_invalidated: true,
                invalidated_reason: r.invalidated_reason.clone(),
            });
            continue;
        }
        let p50 = nearest_rank_percentile(&r.samples, 0.50).unwrap_or(0);
        let p90 = nearest_rank_percentile(&r.samples, 0.90).unwrap_or(0);
        let p95 = nearest_rank_percentile(&r.samples, 0.95).unwrap_or(0);
        let p99 = nearest_rank_percentile(&r.samples, 0.99).unwrap_or(0);
        per_round.push(ProtocolBRoundStats {
            n_samples: r.samples.len(),
            p50_ns: p50,
            p90_ns: p90,
            p95_ns: p95,
            p99_ns: p99,
            is_invalidated: false,
            invalidated_reason: String::new(),
        });
        round_p99s.push(p99);
    }

    let median_p99 = median(&round_p99s).unwrap_or(0);

    ProtocolBOutcome {
        median_p99_ns: median_p99,
        round_p99s_ns: round_p99s,
        per_round,
        malformed_records: malformed,
        total_samples: total,
        is_invalidated: outcome_invalidated,
        invalidated_reason: outcome_reason,
    }
}

/// Per-cell summary (aggregated across N rounds).
///
/// Derived from one or more [`ProtocolBRoundStats`] (typically 5, one per
/// M2 round). Per-round percentiles are reduced to a median across rounds
/// (the v3.5 headline methodology); `round_p99s_ns` preserves the original
/// per-round distribution for dispersion disclosure (spec F1).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CellSummary {
    pub cell_name: String,
    pub p50_ns: u64,
    pub p90_ns: u64,
    pub p95_ns: u64,
    pub p99_ns: u64,
    /// All per-round p99 values, in original order (not sorted).
    pub round_p99s_ns: Vec<u64>,
    pub n_rounds: u32,
    pub n_samples_per_round: u64,
    /// True if ANY contributing round was invalidated.
    pub is_invalidated: bool,
}

/// Aggregates N [`ProtocolBRoundStats`] (one per round) into one
/// [`CellSummary`]. Per-round percentiles → median across rounds; the
/// `round_p99s_ns` Vec is preserved in original order for distribution
/// disclosure.
///
/// Panics on empty input — callers must skip cells with no rounds.
pub fn aggregate_per_cell(cell_name: &str, rounds: &[ProtocolBRoundStats]) -> CellSummary {
    assert!(!rounds.is_empty(), "aggregate_per_cell: empty rounds");
    let n = rounds.len();

    let mut p50s: Vec<u64> = rounds.iter().map(|r| r.p50_ns).collect();
    let mut p90s: Vec<u64> = rounds.iter().map(|r| r.p90_ns).collect();
    let mut p95s: Vec<u64> = rounds.iter().map(|r| r.p95_ns).collect();
    let p99s_original: Vec<u64> = rounds.iter().map(|r| r.p99_ns).collect();

    p50s.sort_unstable();
    p90s.sort_unstable();
    p95s.sort_unstable();
    let mut p99s_sorted = p99s_original.clone();
    p99s_sorted.sort_unstable();

    // Median of a non-empty sorted slice. For odd n, the middle element;
    // for even n, the upper-middle (matches the v3.5 "n=5 → 3rd of 5"
    // convention; rounds-per-cell is always odd in the current harness).
    let median = |v: &[u64]| v[v.len() / 2];

    CellSummary {
        cell_name: cell_name.into(),
        p50_ns: median(&p50s),
        p90_ns: median(&p90s),
        p95_ns: median(&p95s),
        p99_ns: median(&p99s_sorted),
        round_p99s_ns: p99s_original,
        n_rounds: n as u32,
        n_samples_per_round: rounds[0].n_samples as u64,
        is_invalidated: rounds.iter().any(|r| r.is_invalidated),
    }
}

/// One rust-vs-java pair within a scenario, with full distribution
/// disclosure on both sides plus pairwise deltas.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BridgeTaxPair {
    pub scenario: String,
    pub rust_cell: String,
    pub java_cell: String,
    pub rust_p50_ns: u64,
    pub rust_p90_ns: u64,
    pub rust_p95_ns: u64,
    pub rust_p99_ns: u64,
    pub rust_round_p99s_ns: Vec<u64>,
    /// (max, min) of `rust_round_p99s_ns`. Serializes as JSON array
    /// `[max_ns, min_ns]`. Order is max-first to match the natural
    /// "range = max - min" intuition.
    pub rust_p99_range_ns: (u64, u64),
    pub java_p50_ns: u64,
    pub java_p90_ns: u64,
    pub java_p95_ns: u64,
    pub java_p99_ns: u64,
    pub java_round_p99s_ns: Vec<u64>,
    /// (max, min) of `java_round_p99s_ns`. Serializes as JSON array
    /// `[max_ns, min_ns]` (same convention as `rust_p99_range_ns`).
    pub java_p99_range_ns: (u64, u64),
    /// Location estimate of bridge tax: rust p50 − java p50.
    pub p50_delta_ns: i64,
    /// Tail delta: rust p99 − java p99.
    pub p99_delta_ns: i64,
    /// rust/java p99 ratio (NaN when java p99 is zero).
    pub p99_ratio: f64,
    pub caveat: String,
}

/// A pair that could not be formed or was invalidated; reported alongside
/// successful pairs so the consumer can detect missing data.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DroppedPair {
    pub rust_cell: String,
    pub java_cell: String,
    pub reason: String,
}

/// Output of [`aggregate_bridge_tax`]: pairs + drops + a single
/// uncertainty statement that consumers attach once to any report.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AggregatedBridgeTax {
    pub pairs: Vec<BridgeTaxPair>,
    pub dropped_pairs: Vec<DroppedPair>,
    pub uncertainty_statement: String,
}

const BRIDGE_TAX_CAVEAT: &str = "These deltas describe two independent \
    distributions. The bridge tax per request is not directly observable; \
    p50_delta is the location estimate of the bridge tax distribution under \
    the assumption that per-request rust and java latencies would pair \
    monotonically.";

const UNCERTAINTY_STATEMENT: &str = "Descriptive statistics only. No \
    inferential CI or significance test is claimed. Per-cell headline is \
    median-of-5-round-p99; the round_p99s_ns array and range disclose \
    round-to-round variability. Five process launches is insufficient for \
    process-level uncertainty estimation.";

/// Pairs `rust-camel-lib` vs `{camel-standalone-dsl, camel-quarkus-dsl-native}`
/// per scenario, computed from a flat list of [`CellSummary`] values.
///
/// Cells are grouped by the path segment before the first `/` in
/// `cell_name` (the scenario); the segment after is the contender key.
/// Cells whose `cell_name` has no `/` are grouped under `_unknown_`.
///
/// Pairs are dropped (not omitted silently) when one side is missing or
/// either cell is invalidated — the consumer MUST see the drop to avoid
/// silently publishing an incomplete comparison.
pub fn aggregate_bridge_tax(summaries: &[CellSummary]) -> AggregatedBridgeTax {
    use std::collections::HashMap;

    let mut by_scenario: HashMap<String, HashMap<&str, &CellSummary>> = HashMap::new();
    for s in summaries {
        let (scenario, cell) = match s.cell_name.split_once('/') {
            Some((s, c)) => (s.to_string(), c),
            None => (s.cell_name.clone(), "_unknown_"),
        };
        by_scenario.entry(scenario).or_default().insert(cell, s);
    }

    let mut pairs = Vec::new();
    let mut dropped = Vec::new();

    let rust_keys = ["rust-camel-lib"];
    let java_keys = ["camel-standalone-dsl", "camel-quarkus-dsl-native"];

    for (scenario, cells) in by_scenario.iter() {
        for rust_key in rust_keys {
            for java_key in java_keys {
                let rust = cells.get(rust_key);
                let java = cells.get(java_key);
                match (rust, java) {
                    (Some(r), Some(j)) => {
                        if r.is_invalidated || j.is_invalidated {
                            dropped.push(DroppedPair {
                                rust_cell: format!("{scenario}/{rust_key}"),
                                java_cell: format!("{scenario}/{java_key}"),
                                reason: "one or both cells invalidated".into(),
                            });
                        } else {
                            let p99_ratio = if j.p99_ns == 0 {
                                f64::NAN
                            } else {
                                r.p99_ns as f64 / j.p99_ns as f64
                            };
                            let rust_range = (
                                *r.round_p99s_ns.iter().max().unwrap_or(&0),
                                *r.round_p99s_ns.iter().min().unwrap_or(&0),
                            );
                            let java_range = (
                                *j.round_p99s_ns.iter().max().unwrap_or(&0),
                                *j.round_p99s_ns.iter().min().unwrap_or(&0),
                            );
                            pairs.push(BridgeTaxPair {
                                scenario: scenario.clone(),
                                rust_cell: r.cell_name.clone(),
                                java_cell: j.cell_name.clone(),
                                rust_p50_ns: r.p50_ns,
                                rust_p90_ns: r.p90_ns,
                                rust_p95_ns: r.p95_ns,
                                rust_p99_ns: r.p99_ns,
                                rust_round_p99s_ns: r.round_p99s_ns.clone(),
                                rust_p99_range_ns: rust_range,
                                java_p50_ns: j.p50_ns,
                                java_p90_ns: j.p90_ns,
                                java_p95_ns: j.p95_ns,
                                java_p99_ns: j.p99_ns,
                                java_round_p99s_ns: j.round_p99s_ns.clone(),
                                java_p99_range_ns: java_range,
                                p50_delta_ns: r.p50_ns as i64 - j.p50_ns as i64,
                                p99_delta_ns: r.p99_ns as i64 - j.p99_ns as i64,
                                p99_ratio,
                                caveat: BRIDGE_TAX_CAVEAT.into(),
                            });
                        }
                    }
                    _ => {
                        // One side absent entirely (e.g. partial run where
                        // camel-quarkus-dsl-native was not measured). Push
                        // a DroppedPair so the publication gate
                        // (`dropped_pairs.len() == 0`) catches the missing
                        // data instead of silently passing. Missing cells
                        // MUST be visible in the output.
                        dropped.push(DroppedPair {
                            rust_cell: format!("{scenario}/{rust_key}"),
                            java_cell: format!("{scenario}/{java_key}"),
                            reason: "missing cell on one side".into(),
                        });
                    }
                }
            }
        }
    }

    AggregatedBridgeTax {
        pairs,
        dropped_pairs: dropped,
        uncertainty_statement: UNCERTAINTY_STATEMENT.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_line_valid_record() {
        let rec = parse_line("BENCH_LATENCY 42 15000000").unwrap();
        assert_eq!(rec.tick_id, 42);
        assert_eq!(rec.latency_ns, 15_000_000);
    }

    #[test]
    fn parse_line_handles_leading_trailing_whitespace() {
        let rec = parse_line("  BENCH_LATENCY 7 3000000  \n").unwrap();
        assert_eq!(rec.tick_id, 7);
        assert_eq!(rec.latency_ns, 3_000_000);
    }

    #[test]
    fn parse_line_rejects_non_latency_marker() {
        let err = parse_line("BENCH_ROUTE_READY 12345").unwrap_err();
        assert!(matches!(err, ParseError::NotALatencyRecord { .. }));
    }

    #[test]
    fn parse_line_rejects_missing_latency() {
        let err = parse_line("BENCH_LATENCY 42").unwrap_err();
        assert!(matches!(err, ParseError::MissingTokens { .. }));
    }

    #[test]
    fn parse_line_rejects_non_integer_tick() {
        let err = parse_line("BENCH_LATENCY abc 15").unwrap_err();
        assert!(matches!(err, ParseError::InvalidInteger { .. }));
    }

    #[test]
    fn parse_line_rejects_non_integer_latency() {
        let err = parse_line("BENCH_LATENCY 42 xyz").unwrap_err();
        assert!(matches!(err, ParseError::InvalidInteger { .. }));
    }

    #[test]
    fn parse_log_single_round() {
        // 5 records, no blank lines → 1 round. Field 2 is ns (direct).
        let log = "BENCH_LATENCY 0 10000000\n\
                   BENCH_LATENCY 1 10000000\n\
                   BENCH_LATENCY 2 10000000\n\
                   BENCH_LATENCY 3 10000000\n\
                   BENCH_LATENCY 4 100000000\n";
        let out = parse_protocol_b_log(log);
        assert_eq!(out.per_round.len(), 1);
        assert_eq!(out.per_round[0].n_samples, 5);
        // p99 = highest of 5 samples = 100_000_000 ns (nearest-rank idx=5).
        assert_eq!(out.per_round[0].p99_ns, 100_000_000);
        assert_eq!(out.median_p99_ns, 100_000_000);
        assert_eq!(out.malformed_records, 0);
    }

    #[test]
    fn parse_log_multi_round_with_blank_separators() {
        let log = "BENCH_LATENCY 0 10000000\n\
                   BENCH_LATENCY 1 10000000\n\
                   BENCH_LATENCY 2 10000000\n\
                   BENCH_LATENCY 3 10000000\n\
                   BENCH_LATENCY 4 10000000\n\
                   \n\
                   BENCH_LATENCY 0 20000000\n\
                   BENCH_LATENCY 1 20000000\n\
                   BENCH_LATENCY 2 20000000\n\
                   BENCH_LATENCY 3 20000000\n\
                   BENCH_LATENCY 4 20000000\n";
        let out = parse_protocol_b_log(log);
        assert_eq!(out.per_round.len(), 2);
        assert_eq!(out.per_round[0].p99_ns, 10_000_000);
        assert_eq!(out.per_round[1].p99_ns, 20_000_000);
        // median of [10, 20] ms-equivalent ns = 15_000_000 ns
        assert_eq!(out.median_p99_ns, 15_000_000);
        assert_eq!(out.round_p99s_ns, vec![10_000_000, 20_000_000]);
    }

    #[test]
    fn parse_log_five_rounds_median_picks_third() {
        let mut log = String::new();
        for round_idx in 0..5 {
            for tick in 0..10 {
                // 10, 20, 30, 40, 50 ms — emitted directly as ns.
                let lat = (round_idx + 1) * 10_000_000;
                log.push_str(&format!("BENCH_LATENCY {tick} {lat}\n"));
            }
            log.push('\n'); // round separator
        }
        let out = parse_protocol_b_log(&log);
        assert_eq!(out.per_round.len(), 5);
        // Per-round p99 is the highest sample in each round → 10,20,30,40,50 ms.
        // Median of [10, 20, 30, 40, 50] = 30 ms.
        assert_eq!(out.median_p99_ns, 30_000_000);
    }

    #[test]
    fn parse_log_empty_content_returns_empty_outcome() {
        let out = parse_protocol_b_log("");
        assert_eq!(out.per_round.len(), 0);
        assert_eq!(out.median_p99_ns, 0);
        assert_eq!(out.total_samples, 0);
    }

    #[test]
    fn parse_log_only_blank_lines_returns_empty() {
        let out = parse_protocol_b_log("\n\n\n");
        assert_eq!(out.per_round.len(), 0);
        assert_eq!(out.total_samples, 0);
    }

    #[test]
    fn parse_log_trailing_blank_does_not_create_empty_round() {
        let log = "BENCH_LATENCY 0 10\n\n";
        let out = parse_protocol_b_log(log);
        assert_eq!(out.per_round.len(), 1);
    }

    #[test]
    fn protocol_b_round_stats_p50_p90_p95_p99_disjoint_for_known_distribution() {
        // 20 samples: 1, 2, 3, ..., 20 ms — emitted directly as ns.
        let mut log = String::new();
        for tick in 1..=20 {
            let lat_ns = tick * 1_000_000;
            log.push_str(&format!("BENCH_LATENCY {tick} {lat_ns}\n"));
        }
        let out = parse_protocol_b_log(&log);
        let s = &out.per_round[0];
        // nearest-rank: p50 = ceil(0.5 * 20) = 10th value = 10ms
        assert_eq!(s.p50_ns, 10_000_000);
        // p90 = ceil(0.90 * 20) = 18th value = 18ms
        assert_eq!(s.p90_ns, 18_000_000);
        // p95 = ceil(0.95 * 20) = 19th value = 19ms
        assert_eq!(s.p95_ns, 19_000_000);
        // p99 = ceil(0.99 * 20) = 20th value = 20ms
        assert_eq!(s.p99_ns, 20_000_000);
    }

    #[test]
    fn aggregate_bridge_tax_pairs_rust_vs_java_correctly() {
        use crate::protocol_b::{aggregate_bridge_tax, CellSummary};

        let rust = CellSummary {
            cell_name: "xslt-bridge/rust-camel-lib".into(),
            p50_ns: 800_000,
            p90_ns: 1_200_000,
            p95_ns: 1_800_000,
            p99_ns: 2_500_000,
            round_p99s_ns: vec![2_400_000, 2_500_000, 2_600_000, 2_500_000, 2_500_000],
            n_rounds: 5,
            n_samples_per_round: 10_000,
            is_invalidated: false,
        };
        let java_standalone = CellSummary {
            cell_name: "xslt-bridge/camel-standalone-dsl".into(),
            p50_ns: 300_000,
            p90_ns: 500_000,
            p95_ns: 700_000,
            p99_ns: 1_000_000,
            round_p99s_ns: vec![950_000, 1_000_000, 1_050_000, 1_000_000, 1_000_000],
            n_rounds: 5,
            n_samples_per_round: 10_000,
            is_invalidated: false,
        };
        let java_quarkus = CellSummary {
            cell_name: "xslt-bridge/camel-quarkus-dsl-native".into(),
            p50_ns: 250_000,
            p90_ns: 450_000,
            p95_ns: 650_000,
            p99_ns: 900_000,
            round_p99s_ns: vec![850_000, 900_000, 950_000, 900_000, 900_000],
            n_rounds: 5,
            n_samples_per_round: 10_000,
            is_invalidated: false,
        };

        let result = aggregate_bridge_tax(&[rust, java_standalone, java_quarkus]);
        assert_eq!(result.pairs.len(), 2, "2 pairs: standalone + quarkus");
        assert_eq!(
            result.dropped_pairs.len(),
            0,
            "no drops when all cells present"
        );

        // Verify the standalone pair (deterministic order: java_keys is
        // iterated as ["camel-standalone-dsl", "camel-quarkus-dsl-native"],
        // so standalone always comes before quarkus).
        let p = &result.pairs[0];
        assert_eq!(p.rust_cell, "xslt-bridge/rust-camel-lib");
        assert_eq!(p.java_cell, "xslt-bridge/camel-standalone-dsl");
        assert_eq!(p.p50_delta_ns, 500_000);
        assert_eq!(p.p99_delta_ns, 1_500_000);
        assert!((p.p99_ratio - 2.5).abs() < 0.001);
        assert_eq!(p.rust_round_p99s_ns.len(), 5);
        assert_eq!(p.java_round_p99s_ns.len(), 5);
        assert_eq!(p.rust_p99_range_ns, (2_600_000u64, 2_400_000u64));
        assert_eq!(p.java_p99_range_ns, (1_050_000u64, 950_000u64));

        // Verify the quarkus pair.
        let q = &result.pairs[1];
        assert_eq!(q.java_cell, "xslt-bridge/camel-quarkus-dsl-native");
        assert_eq!(q.p99_delta_ns, 1_600_000);
    }

    #[test]
    fn aggregate_bridge_tax_drops_pairs_with_invalidated_cells() {
        use crate::protocol_b::{aggregate_bridge_tax, CellSummary};

        let rust = CellSummary {
            cell_name: "xsd-validation-bridge/rust-camel-lib".into(),
            p50_ns: 80_000,
            p90_ns: 120_000,
            p95_ns: 180_000,
            p99_ns: 250_000,
            round_p99s_ns: vec![240_000, 250_000, 260_000, 250_000, 250_000],
            n_rounds: 5,
            n_samples_per_round: 10_000,
            is_invalidated: false,
        };
        let java_invalid = CellSummary {
            cell_name: "xsd-validation-bridge/camel-standalone-dsl".into(),
            p50_ns: 30_000,
            p90_ns: 50_000,
            p95_ns: 70_000,
            p99_ns: 100_000,
            round_p99s_ns: vec![95_000, 100_000, 105_000, 100_000, 100_000],
            n_rounds: 5,
            n_samples_per_round: 9_500,
            is_invalidated: true,
        };
        // Present + valid quarkus pair, so the only drop is the invalidated
        // standalone above (not a missing-cell drop from quarkus absence).
        let java_quarkus_ok = CellSummary {
            cell_name: "xsd-validation-bridge/camel-quarkus-dsl-native".into(),
            p50_ns: 25_000,
            p90_ns: 45_000,
            p95_ns: 65_000,
            p99_ns: 90_000,
            round_p99s_ns: vec![85_000, 90_000, 95_000, 90_000, 90_000],
            n_rounds: 5,
            n_samples_per_round: 10_000,
            is_invalidated: false,
        };

        let result = aggregate_bridge_tax(&[rust, java_invalid, java_quarkus_ok]);
        assert_eq!(result.pairs.len(), 1, "quarkus pair should be in pairs");
        assert_eq!(
            result.dropped_pairs.len(),
            1,
            "invalidated standalone pair dropped"
        );
        assert_eq!(
            result.dropped_pairs[0].rust_cell,
            "xsd-validation-bridge/rust-camel-lib"
        );
        assert_eq!(
            result.dropped_pairs[0].java_cell,
            "xsd-validation-bridge/camel-standalone-dsl"
        );
        assert!(result.dropped_pairs[0].reason.contains("invalidated"));
    }

    #[test]
    fn aggregate_bridge_tax_drops_pair_when_java_baseline_missing() {
        use crate::protocol_b::{aggregate_bridge_tax, CellSummary};

        let rust = CellSummary {
            cell_name: "xslt-bridge/rust-camel-lib".into(),
            p50_ns: 800_000,
            p90_ns: 1_200_000,
            p95_ns: 1_800_000,
            p99_ns: 2_500_000,
            round_p99s_ns: vec![2_500_000],
            n_rounds: 1,
            n_samples_per_round: 10,
            is_invalidated: false,
        };
        let java_standalone = CellSummary {
            cell_name: "xslt-bridge/camel-standalone-dsl".into(),
            p50_ns: 300_000,
            p90_ns: 500_000,
            p95_ns: 700_000,
            p99_ns: 1_000_000,
            round_p99s_ns: vec![1_000_000],
            n_rounds: 1,
            n_samples_per_round: 10,
            is_invalidated: false,
        };
        // NOTE: camel-quarkus-dsl-native is intentionally ABSENT —
        // simulates a partial run where the quarkus cell wasn't measured.

        let result = aggregate_bridge_tax(&[rust, java_standalone]);
        assert_eq!(result.pairs.len(), 1, "the present pair should be in pairs");
        assert_eq!(
            result.dropped_pairs.len(),
            1,
            "missing quarkus pair must be dropped (not silently omitted)"
        );
        let drop = &result.dropped_pairs[0];
        assert_eq!(drop.rust_cell, "xslt-bridge/rust-camel-lib");
        assert_eq!(drop.java_cell, "xslt-bridge/camel-quarkus-dsl-native");
        assert!(
            drop.reason.contains("missing"),
            "reason must explain the drop, got: {}",
            drop.reason
        );
    }

    #[test]
    fn aggregate_bridge_tax_p99_ratio_handles_zero_java_p99() {
        use crate::protocol_b::{aggregate_bridge_tax, CellSummary};
        let rust = CellSummary {
            cell_name: "xslt-bridge/rust-camel-lib".into(),
            p50_ns: 800_000,
            p90_ns: 1_200_000,
            p95_ns: 1_800_000,
            p99_ns: 2_500_000,
            round_p99s_ns: vec![2_500_000],
            n_rounds: 1,
            n_samples_per_round: 10,
            is_invalidated: false,
        };
        let java_zero = CellSummary {
            cell_name: "xslt-bridge/camel-standalone-dsl".into(),
            p50_ns: 0,
            p90_ns: 0,
            p95_ns: 0,
            p99_ns: 0,
            round_p99s_ns: vec![0],
            n_rounds: 1,
            n_samples_per_round: 10,
            is_invalidated: false,
        };
        let result = aggregate_bridge_tax(&[rust, java_zero]);
        assert_eq!(result.pairs.len(), 1);
        assert!(
            result.pairs[0].p99_ratio.is_nan(),
            "zero java_p99 must yield NaN ratio, not panic"
        );
    }

    #[test]
    fn aggregate_per_cell_picks_median_round_p99() {
        use crate::protocol_b::{aggregate_per_cell, ProtocolBRoundStats};
        // 5 rounds with p99s [10, 30, 50, 70, 90] → median = 50
        let rounds: Vec<ProtocolBRoundStats> = (0..5)
            .map(|i| ProtocolBRoundStats {
                n_samples: 10_000,
                p50_ns: 100 + i * 10,
                p90_ns: 150 + i * 15,
                p95_ns: 200 + i * 20,
                p99_ns: 10 + i * 20, // [10, 30, 50, 70, 90]
                is_invalidated: false,
                invalidated_reason: String::new(),
            })
            .collect();
        let cell = aggregate_per_cell("test-scenario/test-cell", &rounds);
        assert_eq!(cell.p99_ns, 50, "median of [10,30,50,70,90] is 50");
        assert_eq!(cell.round_p99s_ns, vec![10, 30, 50, 70, 90]);
        assert_eq!(cell.n_rounds, 5);
        assert_eq!(cell.n_samples_per_round, 10_000);
        assert_eq!(cell.p90_ns, 150 + 2 * 15); // median of [150,165,180,195,210] = 180
    }

    #[test]
    fn parse_line_treats_field2_as_nanoseconds_not_millis() {
        let rec = parse_line("BENCH_LATENCY 42 500000").unwrap();
        assert_eq!(rec.tick_id, 42);
        assert_eq!(rec.latency_ns, 500_000);
    }

    #[test]
    fn parse_line_rejects_zero_duration() {
        let err = parse_line("BENCH_LATENCY 42 0").unwrap_err();
        assert!(matches!(err, ParseError::OutOfBound { .. }));
    }

    #[test]
    fn parse_line_rejects_negative_duration() {
        let err = parse_line("BENCH_LATENCY 42 -100").unwrap_err();
        assert!(matches!(err, ParseError::OutOfBound { .. }));
    }

    #[test]
    fn parse_line_rejects_duration_above_30s_deadline() {
        let err = parse_line("BENCH_LATENCY 42 30000000001").unwrap_err();
        assert!(matches!(err, ParseError::OutOfBound { .. }));
    }

    #[test]
    fn parse_line_accepts_duration_at_30s_boundary() {
        let rec = parse_line("BENCH_LATENCY 42 30000000000").unwrap();
        assert_eq!(rec.latency_ns, 30_000_000_000);
    }

    #[test]
    fn parse_log_aborts_on_any_malformed_record() {
        let log = "BENCH_LATENCY 0 100\n\
                   BENCH_LATENCY 1 200\n\
                   BENCH_LATENCY 2 -5\n\
                   BENCH_LATENCY 3 400\n";
        let result = parse_protocol_b_log(log);
        assert!(result.is_invalidated, "round must be invalidated");
        assert!(result.invalidated_reason.contains("malformed"));
    }

    #[test]
    fn parse_log_aborts_on_any_out_of_bound_record() {
        let log = "BENCH_LATENCY 0 100\n\
                   BENCH_LATENCY 1 200\n\
                   BENCH_LATENCY 2 99000000000000\n";
        let result = parse_protocol_b_log(log);
        assert!(result.is_invalidated);
        assert!(result.invalidated_reason.contains("out-of-bound"));
    }

    #[test]
    fn parse_log_aborts_remaining_rounds_after_malformed() {
        // Multi-round log where round 3 (of 5) contains an out-of-bound record.
        // File-level abort (`break 'outer`) must stop processing ALL subsequent
        // rounds, not just the current one. Per-round `break` would continue
        // parsing rounds 4 and 5.
        let log = "BENCH_LATENCY 0 1000\n\
                   \n\
                   BENCH_LATENCY 10 2000\n\
                   \n\
                   BENCH_LATENCY 20 99000000000000\n\
                   \n\
                   BENCH_LATENCY 30 4000\n\
                   \n\
                   BENCH_LATENCY 40 5000\n";
        let result = parse_protocol_b_log(log);
        assert!(result.is_invalidated, "outcome must be invalidated");
        assert!(
            result.per_round.len() < 5,
            "file-level abort must not process all 5 rounds; got {}",
            result.per_round.len()
        );
        assert!(
            result.invalidated_reason.contains("out-of-bound"),
            "reason must mention out-of-bound, got: {}",
            result.invalidated_reason
        );
    }
}
