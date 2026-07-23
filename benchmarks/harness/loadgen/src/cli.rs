//! CLI dispatcher entry points for each bench-loadgen subcommand.
//!
//! Each `*_main` function is intentionally small: parse args, run the
//! async work, return an ExitCode. The novel logic they invoke lives in
//! the other library modules and is unit-tested there.
//!
//! This split keeps the CLI shell-thin (no statistical logic in main)
//! and makes it trivial to test the parsing + dispatch shape without a
//! network roundtrip.

use std::process::ExitCode;

use crate::bridge::{check_bridge_pid, PidStage};
use crate::protocol_b::{
    aggregate_bridge_tax, aggregate_per_cell, parse_protocol_b_log, CellSummary, ProtocolBOutcome,
    ProtocolBRoundStats,
};
use crate::rss::{aggregate_samples, sample_once, ProcessTreeSample};
use crate::warmup::WarmupConfig;

/// Parse `--key=value` style args from a slice into a key→value lookup.
fn parse_flags(args: &[String]) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for a in args {
        if let Some(rest) = a.strip_prefix("--") {
            if let Some(eq) = rest.find('=') {
                let (k, v) = rest.split_at(eq);
                map.insert(k.to_string(), v[1..].to_string());
            } else {
                map.insert(rest.to_string(), String::new());
            }
        }
    }
    map
}

/// `devnull` subcommand: start the HTTP devnull baseline stub.
pub fn devnull_main(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    let port: u16 = flags
        .get("port")
        .and_then(|s| s.parse().ok())
        .unwrap_or(18080);
    match crate::cli_runtime::run_devnull(port) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("devnull: error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `calibrate` subcommand: Protocol A calibration phase.
pub fn calibrate_main(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    let url = match flags.get("url") {
        Some(u) => u.clone(),
        None => {
            eprintln!("calibrate: --url is required");
            return ExitCode::from(2);
        }
    };
    let timeout_secs: u64 = flags
        .get("timeout-secs")
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    match crate::cli_runtime::run_calibrate(&url, timeout_secs) {
        Ok(rate) => {
            // Print the max sustainable rate per second as a single integer
            // line; the harness captures this via $(...).
            println!("BENCH_CALIBRATION_RESULT max_sustainable_rate_per_sec={rate}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("calibrate: error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `measure-a` subcommand: Protocol A measurement.
pub fn measure_a_main(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    let url = match flags.get("url") {
        Some(u) => u.clone(),
        None => {
            eprintln!("measure-a: --url is required");
            return ExitCode::from(2);
        }
    };
    let rate: u32 = match flags.get("rate").and_then(|s| s.parse().ok()) {
        Some(r) => r,
        None => {
            eprintln!("measure-a: --rate=<positive-int> is required");
            return ExitCode::from(2);
        }
    };
    let samples_per_round: u32 = flags
        .get("samples")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_000);
    let rounds: u32 = flags
        .get("rounds")
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let warmup_msgs: u32 = flags
        .get("warmup-msgs")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);
    let warmup_time_secs: u32 = flags
        .get("warmup-time")
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    let bci_resamples: usize = flags
        .get("bci-resamples")
        .and_then(|s| s.parse().ok())
        .unwrap_or(2000);
    let bci_seed: u64 = flags
        .get("bci-seed")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let output_path = flags.get("out").cloned();
    let cfg = WarmupConfig {
        max_time_seconds: warmup_time_secs,
        max_messages: warmup_msgs,
        tolerance_ppth: 100,
    };
    match crate::cli_runtime::run_measure_a(
        &url,
        rate,
        samples_per_round,
        rounds,
        cfg,
        bci_resamples,
        bci_seed,
        output_path.as_deref(),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("measure-a: error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `measure-throughput` subcommand: sustained-throughput (M3) measurement.
///
/// Drives `workers` concurrent client loops against `--url` for the
/// configured duration (after `--warmup-secs` warmup), bucketizing 2xx
/// responses by wall-clock second. Reports mean / p50 / min / CV /
/// half-life ratio / error rate via `crate::throughput`.
pub fn measure_throughput_main(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    let url = match flags.get("url") {
        Some(u) => u.clone(),
        None => {
            eprintln!("measure-throughput: --url is required");
            return ExitCode::from(2);
        }
    };
    // --duration-secs: length of the measurement window (excludes warmup).
    // Total wall-clock = duration-secs + warmup-secs.
    let duration_secs: u64 = flags
        .get("duration-secs")
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    let warmup_secs: u64 = flags
        .get("warmup-secs")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let workers: usize = flags
        .get("workers")
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        });
    let output_path = flags.get("out").cloned();
    match crate::cli_runtime::run_measure_throughput(
        &url,
        duration_secs,
        warmup_secs,
        workers,
        output_path.as_deref(),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("measure-throughput: error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `parse-protocol-b` subcommand.
pub fn parse_protocol_b_main(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    let path = match flags.get("log") {
        Some(p) => p.clone(),
        None => {
            eprintln!("parse-protocol-b: --log=PATH is required");
            return ExitCode::from(2);
        }
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("parse-protocol-b: cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let outcome = parse_protocol_b_log(&content);
    if flags.contains_key("json") {
        // Programmatic output (consumed by the Task 2 aggregator). Emits the
        // full ProtocolBOutcome as a single JSON line on stdout.
        let json = serde_json::to_string(&outcome)
            .unwrap_or_else(|e| format!("{{\"error\":\"serialize failed: {e}\"}}"));
        println!("{json}");
    } else {
        // Legacy human-readable text output.
        println!(
            "BENCH_PROTOCOL_B_OUTCOME median_p99_ns={} total_samples={} malformed={} rounds={}",
            outcome.median_p99_ns,
            outcome.total_samples,
            outcome.malformed_records,
            outcome.per_round.len()
        );
        for (i, r) in outcome.per_round.iter().enumerate() {
            println!(
                "BENCH_PROTOCOL_B_ROUND idx={} n={} p50_ns={} p90_ns={} p95_ns={} p99_ns={}",
                i, r.n_samples, r.p50_ns, r.p90_ns, r.p95_ns, r.p99_ns
            );
        }
    }
    ExitCode::SUCCESS
}

/// `aggregate-bridge-tax` subcommand.
///
/// Walks `<input-dir>/**/m2-summary.json` (recursive, bounded depth 3),
/// parses each as a [`ProtocolBOutcome`], groups by `<scenario>/<contender>`
/// (derived from the file's parent + grandparent directory names), flattens
/// `per_round` across files in each group, then runs [`aggregate_per_cell`]
/// + [`aggregate_bridge_tax`].
///
/// Handles two on-disk layouts transparently (the cell identity is the
/// parent+grandparent dir pair in both):
/// - **per-round** (current harness, `run.sh` ~line 1571): files at
///   `<run>/m2-round-N/<scenario>/<contender>/m2-summary.json`, each with
///   `per_round.len() == 1`. Five files per cell flatten to a 5-element
///   Vec.
/// - **per-cell** (hypothetical consolidated): one file at
///   `<run>/<scenario>/<contender>/m2-summary.json` with
///   `per_round.len() == 5`.
pub fn aggregate_bridge_tax_main(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    let input_dir = match flags.get("input-dir") {
        Some(p) => std::path::Path::new(p).to_path_buf(),
        None => {
            eprintln!("aggregate-bridge-tax: --input-dir=PATH is required");
            return ExitCode::from(2);
        }
    };

    // Collect (path, cell_dir, ProtocolBOutcome) tuples by recursive walk,
    // then sort by path so rounds within a cell preserve deterministic
    // order (e.g. m2-round-0 < m2-round-1 < ... despite FS traversal
    // ordering being unspecified).
    let mut found: Vec<(std::path::PathBuf, String, ProtocolBOutcome)> = Vec::new();
    collect_m2_summaries(&input_dir, &input_dir, 0, &mut found);
    found.sort_by(|a, b| a.0.cmp(&b.0));

    if found.is_empty() {
        eprintln!(
            "aggregate-bridge-tax: no m2-summary.json files under {}",
            input_dir.display()
        );
        return ExitCode::from(2);
    }

    // Group by cell_dir (preserving path-sorted order within each group)
    // and flatten per_round across files in each group. cell_dir is the
    // canonical "<scenario>/<contender>" form produced by the walker.
    let mut by_cell: std::collections::BTreeMap<String, Vec<ProtocolBRoundStats>> =
        std::collections::BTreeMap::new();
    for (_path, cell_dir, outcome) in found.iter() {
        by_cell
            .entry(cell_dir.clone())
            .or_default()
            .extend(outcome.per_round.iter().cloned());
    }

    let mut summaries: Vec<CellSummary> = Vec::new();
    for (cell_dir, rounds) in by_cell.iter() {
        if rounds.is_empty() {
            eprintln!("aggregate-bridge-tax: cell {cell_dir} has no rounds; skipping");
            continue;
        }
        summaries.push(aggregate_per_cell(cell_dir, rounds));
    }

    let aggregated = aggregate_bridge_tax(&summaries);

    let json = serde_json::to_string_pretty(&aggregated)
        .unwrap_or_else(|e| format!("{{\"error\":\"serialize failed: {e}\"}}"));

    if let Some(out_path) = flags.get("output") {
        if let Err(e) = std::fs::write(out_path, &json) {
            eprintln!("aggregate-bridge-tax: cannot write {}: {out_path}", e);
            return ExitCode::from(3);
        }
    } else {
        println!("{json}");
    }
    ExitCode::SUCCESS
}

/// Recursive walker (bounded to depth 3). For each `m2-summary.json`
/// found, parse it as [`ProtocolBOutcome`] and push `(path, cell_dir,
/// outcome)` to `found`. The `cell_dir` is derived from the file's
/// parent (contender) + grandparent (scenario) directory names, yielding
/// the canonical `<scenario>/<contender>` form the aggregator expects.
///
/// Unreadable / unparseable files are warned about and skipped
/// (partial-failure tolerance, since the harness may emit
/// `status=not-measured` stub text files for cells that didn't run).
fn collect_m2_summaries(
    root: &std::path::Path,
    dir: &std::path::Path,
    depth: usize,
    found: &mut Vec<(std::path::PathBuf, String, ProtocolBOutcome)>,
) {
    if depth > 3 {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_m2_summaries(root, &path, depth + 1, found);
            continue;
        }
        let file_name = match path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };
        if file_name != "m2-summary.json" {
            continue;
        }
        // Derive canonical "<scenario>/<contender>" from path's parent
        // (contender) + grandparent (scenario). Works for both per-round
        // (`<run>/m2-round-N/<scenario>/<contender>/m2-summary.json`) and
        // per-cell (`<run>/<scenario>/<contender>/m2-summary.json`)
        // layouts — in both, the immediate parent is the contender dir
        // and its parent is the scenario dir.
        let parent = match path.parent() {
            Some(p) => p,
            None => continue,
        };
        let contender = match parent.file_name().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };
        let scenario = parent
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("_unknown_");
        let cell_dir = format!("{scenario}/{contender}");
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "aggregate-bridge-tax: cannot read {}: {e}",
                    path.strip_prefix(root).unwrap_or(&path).display()
                );
                continue;
            }
        };
        let outcome: ProtocolBOutcome = match serde_json::from_str(&content) {
            Ok(o) => o,
            Err(e) => {
                // Harness emits "status=not-measured ..." text stubs for
                // cells that produced no BENCH_LATENCY records. Skip them
                // silently unless JSON parse fails on something that looks
                // like JSON (heuristic: starts with `{`).
                if content.trim_start().starts_with('{') {
                    eprintln!(
                        "aggregate-bridge-tax: parse error in {}: {e}",
                        path.strip_prefix(root).unwrap_or(&path).display()
                    );
                }
                continue;
            }
        };
        found.push((path, cell_dir, outcome));
    }
}

/// `rss-sample` subcommand: sample process-tree RSS.
pub fn rss_sample_main(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    let pid: u32 = match flags.get("pid").and_then(|s| s.parse().ok()) {
        Some(p) => p,
        None => {
            eprintln!("rss-sample: --pid=<positive-int> is required");
            return ExitCode::from(2);
        }
    };
    let interval_ms: u64 = flags
        .get("interval-ms")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let duration_ms: u64 = flags
        .get("duration-ms")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);
    let json_output = flags.contains_key("json");
    let offsets = build_sample_offsets(interval_ms, duration_ms);
    #[cfg(target_os = "linux")]
    let reader = crate::rss::LinuxProcReader::new();
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        let _ = offsets;
        eprintln!("rss-sample: only supported on Linux (needs /proc)");
        return ExitCode::FAILURE;
    }
    #[cfg(target_os = "linux")]
    {
        let mut samples: Vec<ProcessTreeSample> = Vec::with_capacity(offsets.len());
        let start = std::time::Instant::now();
        let interval = std::time::Duration::from_millis(interval_ms);
        for offset_ns in &offsets {
            // Pace to the target offset.
            let target = std::time::Duration::from_nanos(*offset_ns);
            let now = start.elapsed();
            if target > now {
                std::thread::sleep(target - now);
            }
            samples.push(sample_once(&reader, pid, *offset_ns));
        }
        let _ = interval; // (kept for future use; current pacing uses target)
        let result = aggregate_samples(samples, duration_ms.saturating_mul(1_000_000));
        println!(
            "BENCH_RSS_OUTCOME max_summed_vmrss_kib={} max_summed_vmhwm_kib={} n_samples={} peak_process_count={} duration_ns={}",
            result.max_summed_vmrss_kib,
            result.max_summed_vmhwm_kib,
            result.n_samples,
            result.peak_process_count,
            result.duration_ns
        );
        if json_output {
            // Programmatic output: full per-sample trace as a JSON array.
            // The aggregate `BENCH_RSS_OUTCOME` line above is preserved
            // for backward compat with the harness; the JSON array lets
            // downstream tools plot / analyze the full sample stream.
            match serde_json::to_string(&result.samples) {
                Ok(json) => println!("BENCH_RSS_SAMPLES_JSON {json}"),
                Err(e) => eprintln!("rss-sample: json serialize failed: {e}"),
            }
        }
        ExitCode::SUCCESS
    }
}

/// `bridge-check` subcommand: compare bridge PID files.
pub fn bridge_check_main(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    let start_path = match flags.get("start") {
        Some(p) => p.clone(),
        None => {
            eprintln!("bridge-check: --start=PATH is required");
            return ExitCode::from(2);
        }
    };
    let end_path = match flags.get("end") {
        Some(p) => p.clone(),
        None => {
            eprintln!("bridge-check: --end=PATH is required");
            return ExitCode::from(2);
        }
    };
    let start = std::fs::read_to_string(&start_path).ok();
    let end = std::fs::read_to_string(&end_path).ok();
    let result = check_bridge_pid(start.as_deref(), end.as_deref());
    let health = match &result {
        crate::bridge::PidMismatch::Consistent { .. } => "consistent",
        crate::bridge::PidMismatch::Changed { .. } => "changed",
        crate::bridge::PidMismatch::Unreadable { stage } => match stage {
            PidStage::Start => "unreadable-start",
            PidStage::End => "unreadable-end",
        },
    };
    println!("BENCH_BRIDGE_CHECK result={health}");
    // Bridge health is separately checked by the harness via the
    // first-output timeout; expose the API here for parity.
    let _ = crate::bridge::BridgeHealth::Unhealthy;
    ExitCode::SUCCESS
}

/// Build the list of sample offsets (in ns) for the given cadence/duration.
fn build_sample_offsets(interval_ms: u64, duration_ms: u64) -> Vec<u64> {
    let step_ns = interval_ms.saturating_mul(1_000_000);
    let total_ns = duration_ms.saturating_mul(1_000_000);
    if step_ns == 0 {
        return Vec::new();
    }
    let mut offsets = Vec::new();
    let mut t = 0u64;
    while t <= total_ns {
        offsets.push(t);
        t = t.saturating_add(step_ns);
    }
    offsets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_flags_handles_long_and_short_values() {
        let args: Vec<String> = vec![
            "--url=http://x".to_string(),
            "--rate=42".to_string(),
            "--dry".to_string(),
        ];
        let m = parse_flags(&args);
        assert_eq!(m.get("url").unwrap(), "http://x");
        assert_eq!(m.get("rate").unwrap(), "42");
        assert_eq!(m.get("dry").unwrap(), "");
    }

    #[test]
    fn parse_flags_ignores_positional_args() {
        let args: Vec<String> = vec!["positional".to_string(), "--k=v".to_string()];
        let m = parse_flags(&args);
        assert_eq!(m.get("k").unwrap(), "v");
        assert!(!m.contains_key("positional"));
    }

    #[test]
    fn build_sample_offsets_inclusive_of_duration() {
        // 10ms interval, 100ms duration → offsets at 0,10,20,...,100.
        let offsets = build_sample_offsets(10, 100);
        assert_eq!(offsets.len(), 11);
        assert_eq!(offsets[0], 0);
        assert_eq!(offsets[10], 100_000_000);
    }

    #[test]
    fn build_sample_offsets_zero_interval_returns_empty() {
        assert!(build_sample_offsets(0, 100).is_empty());
    }
}
