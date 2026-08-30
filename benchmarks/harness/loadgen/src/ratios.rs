//! Ratio-of-medians comparison between two M3 throughput cells
//! (`aggregate-ratios` subcommand, bench-missing-cells task 1.3).
//!
//! Reads two `m3-summary.json` files from the v4 published layout
//! `<run-root>/<cell-dir>/m3-summary.json`, validates comparability, and
//! reports `ratio = median(A per_round_means) / median(B per_round_means)`
//! with a 95% bootstrap confidence interval.
//!
//! ## Validation reasons
//!
//! Every rejection names exactly one reason token, surfaced by the CLI as
//! `ERROR: <reason>: …` on stderr with exit 2:
//!
//! - `metric` — file is not an m3-summary (no `per_round_means`; e.g. an
//!   m2-style summary), or JSON is unparseable / missing required keys.
//! - `means format` — `per_round_means` is empty or holds a non-numeric /
//!   non-positive / non-finite value.
//! - `round count` — `per_round_means.len() != rounds` within one file, or
//!   the two cells claim unequal round counts.
//! - `provenance` — no run root, no `measurement_order.json` at the run
//!   root, unparseable order file, a cell not listed at all, or (paired
//!   mode only) the two cells coming from different run roots.
//!   `--independent` skips ONLY the run-identity check.
//! - `round indices` — the order file disagrees with the summary's round
//!   count, or a cell does not appear exactly once in every round.
//!
//! ## What measurement_order.json encodes
//!
//! run.sh writes `{"seed": N, "order": [[cell, …], …]}` at the run root:
//! for each round (outer array position = round index 0..n−1), the seeded
//! Fisher-Yates order in which the cells of that round were measured. It
//! encodes order, not per-cell indices — so "indices 0..n−1 contiguous"
//! translates to: outer length == `rounds`, and every cell listed exactly
//! once per round (duplicates or skips in a round corrupt the round-index
//! mapping).
//!
//! ## CI method — percentile, deliberately not BCa
//!
//! [`bca`]'s acceleration term is jackknife-derived for a single-sample
//! statistic; for a ratio of two medians it is undefined. The interval is
//! therefore a plain percentile bootstrap on the resampled ratio
//! distribution (2.5th / 97.5th percentiles, linear interpolation).
//!
//! - **Paired** (default): one index vector is drawn per resample and
//!   applied to BOTH cells — preserves round coupling (same round =
//!   same machine conditions).
//! - **Independent** (`--independent`): each cell's indices are resampled
//!   separately; output is tagged `UNPAIRED`.
//!
//! Determinism: a single [`bca::SplitMix64`] stream seeded by `--seed`
//! (default 0); identical inputs + seed ⇒ identical output line.

use std::path::{Path, PathBuf};

use crate::bca::SplitMix64;

/// Default bootstrap resample count (`--bci-resamples`).
pub const DEFAULT_N_RESAMPLES: usize = 2000;

/// A parsed + individually validated m3-summary.json.
#[derive(Debug, Clone)]
pub struct M3Summary {
    /// `rounds` field; must equal `per_round_means.len()`.
    pub rounds: usize,
    /// Per-round mean msgs/sec, in measurement order.
    pub per_round_means: Vec<f64>,
    /// Run root: the summary file's parent's parent
    /// (`<run-root>/<cell-dir>/m3-summary.json`).
    pub run_root: PathBuf,
    /// `cell` field (slash form, e.g. `http-server/rust-camel-lib`);
    /// matched against measurement_order entries.
    pub cell_name: String,
    /// Cell directory name (slash-free); used in the output line.
    pub cell_dir: String,
}

/// Point estimate and 95% percentile-bootstrap CI of the ratio.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RatioOutcome {
    pub point: f64,
    pub lo: f64,
    pub hi: f64,
}

/// Load both summaries, validate the pair, compute the ratio + CI, and
/// format the one-line report.
///
/// Returns `(line, outcome)`; the line is
/// `RATIO <A-cell-dir>/<B-cell-dir> point=… lo=… hi=…` plus ` UNPAIRED`
/// when `independent`.
pub fn compute_ratio(
    a_path: &Path,
    b_path: &Path,
    independent: bool,
    seed: u64,
    n_resamples: usize,
) -> Result<(String, RatioOutcome), String> {
    if n_resamples < 2 {
        return Err(format!(
            "bci-resamples must be >= 2 for a two-sided interval, got {n_resamples}"
        ));
    }
    let a = load_summary(a_path)?;
    let b = load_summary(b_path)?;
    validate_pair(&a, &b, independent)?;

    let ma = median_f64(&a.per_round_means).ok_or_else(|| {
        format!(
            "means format: {}: per_round_means is empty",
            a_path.display()
        )
    })?;
    let mb = median_f64(&b.per_round_means).ok_or_else(|| {
        format!(
            "means format: {}: per_round_means is empty",
            b_path.display()
        )
    })?;
    let point = ma / mb;

    let outcome = RatioOutcome {
        point,
        ..bootstrap_ratio(&a, &b, independent, seed, n_resamples)
    };

    let mut line = format!(
        "RATIO {}/{} point={:.4} lo={:.4} hi={:.4}",
        a.cell_dir, b.cell_dir, outcome.point, outcome.lo, outcome.hi
    );
    if independent {
        line.push_str(" UNPAIRED");
    }
    Ok((line, outcome))
}

/// Pair-level validation: equal round counts; paired mode additionally
/// requires both cells from the same run root.
pub fn validate_pair(a: &M3Summary, b: &M3Summary, independent: bool) -> Result<(), String> {
    if a.rounds != b.rounds || a.per_round_means.len() != b.per_round_means.len() {
        return Err(format!(
            "round count: {} has {} rounds, {} has {}",
            a.cell_dir, a.rounds, b.cell_dir, b.rounds
        ));
    }
    if !independent && a.run_root != b.run_root {
        return Err(format!(
            "provenance: paired cells from different run roots ({} vs {})",
            a.run_root.display(),
            b.run_root.display()
        ));
    }
    Ok(())
}

/// Parse and per-cell-validate one m3-summary.json (metric / means format /
/// round count / provenance / round indices checks from the module docs).
fn load_summary(path: &Path) -> Result<M3Summary, String> {
    let display = path.display();
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {display}: {e}"))?;
    let v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("metric: {display}: unparseable JSON: {e}"))?;

    let means_val = v
        .get("per_round_means")
        .ok_or_else(|| format!("metric: {display}: no per_round_means (m2-style summary?)"))?;
    let arr = means_val
        .as_array()
        .ok_or_else(|| format!("means format: {display}: per_round_means is not an array"))?;
    if arr.is_empty() {
        return Err(format!("means format: {display}: per_round_means is empty"));
    }
    let mut per_round_means = Vec::with_capacity(arr.len());
    for (i, el) in arr.iter().enumerate() {
        let x = el.as_f64().ok_or_else(|| {
            format!("means format: {display}: per_round_means[{i}] is not numeric")
        })?;
        if !x.is_finite() || x <= 0.0 {
            return Err(format!(
                "means format: {display}: per_round_means[{i}] is not a finite positive number"
            ));
        }
        per_round_means.push(x);
    }

    let rounds = v
        .get("rounds")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| format!("metric: {display}: missing or non-numeric rounds"))?
        as usize;
    if per_round_means.len() != rounds {
        return Err(format!(
            "round count: {display}: per_round_means has {} entries but rounds={rounds}",
            per_round_means.len()
        ));
    }

    let cell_name = v
        .get("cell")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("metric: {display}: missing cell"))?
        .to_string();
    let cell_dir = path
        .parent()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| format!("provenance: {display}: cannot derive cell dir"))?;
    let run_root = path
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| format!("provenance: {display}: no run root in path"))?
        .to_path_buf();
    if !run_root.is_dir() {
        return Err(format!(
            "provenance: {display}: run root {} does not exist",
            run_root.display()
        ));
    }
    // Canonicalize so lexical spellings (`./x` vs `x`) of the same run root
    // compare equal in the paired run-root check.
    let run_root = run_root
        .canonicalize()
        .map_err(|e| format!("provenance: {display}: cannot canonicalize run root: {e}"))?;

    // measurement_order.json at the run root must exist AND list this cell.
    let order_path = run_root.join("measurement_order.json");
    let order_display = order_path.display();
    let order_text = std::fs::read_to_string(&order_path).map_err(|_| {
        format!(
            "provenance: {display}: no measurement_order.json at run root {}",
            run_root.display()
        )
    })?;
    let order: serde_json::Value = serde_json::from_str(&order_text)
        .map_err(|e| format!("provenance: {order_display}: unparseable JSON: {e}"))?;
    let order_arr = order
        .get("order")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("provenance: {order_display}: no order array"))?;

    let listed_anywhere = order_arr.iter().any(|r| {
        r.as_array()
            .is_some_and(|cells| cells.iter().any(|c| c.as_str() == Some(cell_name.as_str())))
    });
    if !listed_anywhere {
        return Err(format!(
            "provenance: {order_display}: cell {cell_name} not listed"
        ));
    }
    if order_arr.len() != rounds {
        return Err(format!(
            "round indices: {order_display}: {} rounds listed, summary claims {rounds}",
            order_arr.len()
        ));
    }
    for (r, round_val) in order_arr.iter().enumerate() {
        let cells = round_val
            .as_array()
            .ok_or_else(|| format!("round indices: {order_display}: round {r} is not an array"))?;
        let count = cells
            .iter()
            .filter(|c| c.as_str() == Some(cell_name.as_str()))
            .count();
        if count != 1 {
            return Err(format!(
                "round indices: {order_display}: cell {cell_name} appears {count}x in round {r} (expected exactly once)"
            ));
        }
    }

    Ok(M3Summary {
        rounds,
        per_round_means,
        run_root,
        cell_name,
        cell_dir,
    })
}

/// Percentile bootstrap of the ratio distribution (see module docs).
/// `point` is left as 0.0 — the caller fills the observed ratio.
fn bootstrap_ratio(
    a: &M3Summary,
    b: &M3Summary,
    independent: bool,
    seed: u64,
    n_resamples: usize,
) -> RatioOutcome {
    let n = a.per_round_means.len();
    let mut rng = SplitMix64::new(seed);
    let mut dist: Vec<f64> = Vec::with_capacity(n_resamples);
    for _ in 0..n_resamples {
        let ratio = if independent {
            let ia = draw_indices(&mut rng, n);
            let ib = draw_indices(&mut rng, n);
            median_at(&a.per_round_means, &ia) / median_at(&b.per_round_means, &ib)
        } else {
            // Paired: ONE index vector applied to both cells.
            let idx = draw_indices(&mut rng, n);
            median_at(&a.per_round_means, &idx) / median_at(&b.per_round_means, &idx)
        };
        dist.push(ratio);
    }
    dist.sort_unstable_by(|x, y| x.total_cmp(y));
    RatioOutcome {
        point: 0.0,
        lo: percentile_f64(&dist, 0.025),
        hi: percentile_f64(&dist, 0.975),
    }
}

/// Draw `n` indices (with replacement) from `0..n`.
fn draw_indices(rng: &mut SplitMix64, n: usize) -> Vec<usize> {
    (0..n).map(|_| (rng.next_u64() as usize) % n).collect()
}

/// Median of the values selected by `indices` from `values`.
fn median_at(values: &[f64], indices: &[usize]) -> f64 {
    let picked: Vec<f64> = indices.iter().map(|&i| values[i]).collect();
    // indices.len() == values.len() >= 1 — never None.
    median_f64(&picked).unwrap_or(f64::NAN)
}

/// Standard median on f64 (empty → None; even n → mean of middle pair).
fn median_f64(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable_by(|a, b| a.total_cmp(b));
    let n = sorted.len();
    let mid = n / 2;
    Some(if n % 2 == 1 {
        sorted[mid]
    } else {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    })
}

/// Linear-interpolation percentile (numpy `'linear'`) on an ascending slice.
fn percentile_f64(sorted: &[f64], p: f64) -> f64 {
    let n = sorted.len();
    if n == 1 {
        return sorted[0];
    }
    let rank = p * (n - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = (lo + 1).min(n - 1);
    let frac = rank - lo as f64;
    sorted[lo] * (1.0 - frac) + sorted[hi] * frac
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique temp root per test (tag) — created fresh, removed at end.
    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(tag: &str) -> Self {
            let d = std::env::temp_dir()
                .join(format!("bench-loadgen-ratios-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).expect("create temp root");
            Self(d)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Write `<root>/<cell_dir>/m3-summary.json` with `means_json` (a JSON
    /// array literal) and `rounds`, under cell field `cell` (slash form).
    fn write_summary(
        root: &Path,
        cell_dir: &str,
        cell: &str,
        means_json: &str,
        rounds: usize,
    ) -> PathBuf {
        let dir = root.join(cell_dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m3-summary.json");
        let body = format!(
            "{{\"cell\":\"{cell}\",\"status\":\"ok\",\"median_mean_msgs_per_sec\":1.0,\
             \"min_mean\":1.0,\"max_mean\":1.0,\"per_round_means\":{means_json},\
             \"rounds\":{rounds},\"duration_secs\":50,\"warmup_secs\":10}}"
        );
        std::fs::write(&path, body).unwrap();
        path
    }

    /// Write a valid measurement_order.json listing `cells` in every one of
    /// `rounds` rounds.
    fn write_uniform_order(root: &Path, cells: &[&str], rounds: usize) {
        let round = serde_json::json!(cells);
        let order = serde_json::json!({ "seed": 7, "order": vec![round; rounds] });
        std::fs::write(root.join("measurement_order.json"), order.to_string()).unwrap();
    }

    #[test]
    fn ratio_point_and_ci_synthetic() {
        let root = TempRoot::new("synthetic");
        let pa = write_summary(
            root.path(),
            "cell-a",
            "t/cell-a",
            "[200.0, 200.0, 200.0, 200.0, 200.0]",
            5,
        );
        let pb = write_summary(
            root.path(),
            "cell-b",
            "t/cell-b",
            "[100.0, 100.0, 100.0, 100.0, 100.0]",
            5,
        );
        write_uniform_order(root.path(), &["t/cell-a", "t/cell-b"], 5);

        let (line, out) = compute_ratio(&pa, &pb, false, 0, 2000).unwrap();
        assert!((out.point - 2.0).abs() < 1e-9, "point {out:?}");
        assert!((out.lo - 2.0).abs() <= 0.05, "lo {out:?}");
        assert!((out.hi - 2.0).abs() <= 0.05, "hi {out:?}");
        assert!(line.starts_with("RATIO cell-a/cell-b "), "line {line}");
        assert!(!line.contains("UNPAIRED"), "line {line}");
    }

    #[test]
    fn ratio_deterministic_same_seed() {
        let root = TempRoot::new("deterministic");
        let pa = write_summary(
            root.path(),
            "cell-a",
            "t/cell-a",
            "[190.0, 200.0, 210.0, 205.0, 195.0]",
            5,
        );
        let pb = write_summary(
            root.path(),
            "cell-b",
            "t/cell-b",
            "[95.0, 105.0, 100.0, 102.0, 98.0]",
            5,
        );
        write_uniform_order(root.path(), &["t/cell-a", "t/cell-b"], 5);

        let (line1, out1) = compute_ratio(&pa, &pb, false, 0, 2000).unwrap();
        let (line2, out2) = compute_ratio(&pa, &pb, false, 0, 2000).unwrap();
        assert_eq!(line1, line2);
        assert_eq!(out1, out2);
    }

    #[test]
    fn ratio_rejects_cross_run_paired() {
        let root1 = TempRoot::new("crossrun1");
        let root2 = TempRoot::new("crossrun2");
        let pa = write_summary(
            root1.path(),
            "cell-a",
            "t/cell-a",
            "[200.0, 200.0, 200.0, 200.0, 200.0]",
            5,
        );
        write_uniform_order(root1.path(), &["t/cell-a"], 5);
        let pb = write_summary(
            root2.path(),
            "cell-b",
            "t/cell-b",
            "[100.0, 100.0, 100.0, 100.0, 100.0]",
            5,
        );
        write_uniform_order(root2.path(), &["t/cell-b"], 5);

        let paired = compute_ratio(&pa, &pb, false, 0, 2000);
        assert!(paired.is_err());
        let err = paired.unwrap_err();
        assert!(err.contains("provenance"), "err {err}");

        let indep = compute_ratio(&pa, &pb, true, 0, 2000).unwrap();
        assert!(indep.0.ends_with(" UNPAIRED"), "line {}", indep.0);
    }

    #[test]
    fn ratio_rejects_malformed() {
        // (a) m2-style summary: no per_round_means -> metric.
        {
            let root = TempRoot::new("malform-a");
            let dir = root.path().join("cell-a");
            std::fs::create_dir_all(&dir).unwrap();
            let pa = dir.join("m3-summary.json");
            std::fs::write(
                &pa,
                "{\"cell\":\"t/cell-a\",\"per_round\":[{\"p50_ns\":100}],\"rounds\":5}",
            )
            .unwrap();
            let pb = write_summary(
                root.path(),
                "cell-b",
                "t/cell-b",
                "[100.0, 100.0, 100.0, 100.0, 100.0]",
                5,
            );
            write_uniform_order(root.path(), &["t/cell-a", "t/cell-b"], 5);
            let err = compute_ratio(&pa, &pb, false, 0, 2000).unwrap_err();
            assert!(err.contains("metric"), "err {err}");
        }
        // (b) missing measurement_order.json -> provenance.
        {
            let root = TempRoot::new("malform-b");
            let pa = write_summary(
                root.path(),
                "cell-a",
                "t/cell-a",
                "[200.0, 200.0, 200.0, 200.0, 200.0]",
                5,
            );
            let pb = write_summary(
                root.path(),
                "cell-b",
                "t/cell-b",
                "[100.0, 100.0, 100.0, 100.0, 100.0]",
                5,
            );
            // No order file written.
            let err = compute_ratio(&pa, &pb, false, 0, 2000).unwrap_err();
            assert!(err.contains("provenance"), "err {err}");
        }
        // (c) means len 3 vs 5 -> round count.
        {
            let root = TempRoot::new("malform-c");
            let pa = write_summary(
                root.path(),
                "cell-a",
                "t/cell-a",
                "[200.0, 200.0, 210.0]",
                5,
            );
            let pb = write_summary(
                root.path(),
                "cell-b",
                "t/cell-b",
                "[100.0, 100.0, 100.0, 100.0, 100.0]",
                5,
            );
            write_uniform_order(root.path(), &["t/cell-a", "t/cell-b"], 5);
            let err = compute_ratio(&pa, &pb, false, 0, 2000).unwrap_err();
            assert!(err.contains("round count"), "err {err}");
        }
        // (d) order-file duplicate/skip in one round -> round indices.
        {
            let root = TempRoot::new("malform-d");
            let pa = write_summary(
                root.path(),
                "cell-a",
                "t/cell-a",
                "[200.0, 200.0, 200.0, 200.0, 200.0]",
                5,
            );
            let pb = write_summary(
                root.path(),
                "cell-b",
                "t/cell-b",
                "[100.0, 100.0, 100.0, 100.0, 100.0]",
                5,
            );
            // Round 2 lists cell-a twice and skips cell-b.
            let order = serde_json::json!({
                "seed": 7,
                "order": [
                    ["t/cell-a", "t/cell-b"],
                    ["t/cell-a", "t/cell-b"],
                    ["t/cell-a", "t/cell-a"],
                    ["t/cell-a", "t/cell-b"],
                    ["t/cell-a", "t/cell-b"],
                ]
            });
            std::fs::write(
                root.path().join("measurement_order.json"),
                order.to_string(),
            )
            .unwrap();
            let err = compute_ratio(&pa, &pb, false, 0, 2000).unwrap_err();
            assert!(err.contains("round indices"), "err {err}");
        }
        // (e) non-numeric mean and empty means -> means format.
        {
            let root = TempRoot::new("malform-e");
            let pa = write_summary(
                root.path(),
                "cell-a",
                "t/cell-a",
                "[\"lots\", 200.0, 200.0, 200.0, 200.0]",
                5,
            );
            let pb = write_summary(
                root.path(),
                "cell-b",
                "t/cell-b",
                "[100.0, 100.0, 100.0, 100.0, 100.0]",
                5,
            );
            write_uniform_order(root.path(), &["t/cell-a", "t/cell-b"], 5);
            let err = compute_ratio(&pa, &pb, false, 0, 2000).unwrap_err();
            assert!(err.contains("means format"), "err {err}");
        }
        {
            let root = TempRoot::new("malform-e2");
            let pa = write_summary(root.path(), "cell-a", "t/cell-a", "[]", 5);
            let pb = write_summary(
                root.path(),
                "cell-b",
                "t/cell-b",
                "[100.0, 100.0, 100.0, 100.0, 100.0]",
                5,
            );
            write_uniform_order(root.path(), &["t/cell-a", "t/cell-b"], 5);
            let err = compute_ratio(&pa, &pb, false, 0, 2000).unwrap_err();
            assert!(err.contains("means format"), "err {err}");
        }
    }
}
