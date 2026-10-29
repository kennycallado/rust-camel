//! Criterion bench and perf guard for `camel-language-simple`.
//!
//! Bench cases are measured by criterion; each measurement is recorded in
//! process immediately after `bench_function` returns by reading that
//! benchmark's freshly written `new/estimates.json`. The gate function then
//! inspects only the workloads measured in this process run, so filtered runs
//! never judge stale saved results. No separate timing mechanism is used.
//!
//! Gates:
//! - `SIMPLE_CHOICE_MAX_US` (default 9): `predicate_eval/choice` median.
//! - `SIMPLE_25STEP_MAX_US` (default 250): `expression_eval/compute_25` median.
//! - `SIMPLE_EVAL_BASELINE=<path>`: package-relative baseline file. The
//!   comparison uses the same statistic on both sides: criterion's typical
//!   estimate — the slope point estimate when the measurement carries a
//!   slope, the mean point estimate under flat sampling — i.e. the middle
//!   value of the printed `time: [lo est hi]` triple. Only workloads present
//!   in both the baseline and this run are compared. A provenance mismatch
//!   (host or cpu) only prints a SKIPPED line.
//!
//! Criterion test/list/profile modes take no measurements, so gate
//! enforcement is skipped there.
//!
//! A breached gate panics with the measured number, mirroring the hand-rolled
//! gate convention of the rhai `eval_overhead` bench.

use std::collections::HashMap;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use camel_language_api::{Exchange, Language, Message, Value};
use camel_language_simple::SimpleLanguage;
use criterion::{Criterion, criterion_group, criterion_main};
use tokio::runtime::Runtime;

const EXPRESSION_GROUP: &str = "language_simple/expression_eval";
const PREDICATE_GROUP: &str = "language_simple/predicate_eval";

const DEFAULT_MAX_CHOICE_US: f64 = 9.0;
const DEFAULT_MAX_25STEP_US: f64 = 250.0;
const BASELINE_REGRESSION_LIMIT: f64 = 1.10;

/// The 3-clause choice predicate from the corpus (a scalar-only slot).
const CHOICE_SCRIPT: &str = "default(header.at, '0') != 'null' && int(property.nowMs) >= int(header.at) && header.body != 'null'";

/// Compound boolean predicate restoring the old `complex` interpolation
/// workload: three template slots evaluated in one `&&` chain (no
/// short-circuit — every clause is true for the fixture).
const COMPLEX_PREDICATE_SCRIPT: &str =
    "${header.foo} == 'bar' && ${header.count} > 10 && ${body} contains 'world'";

/// 25 `int()` lookups combined by arithmetic/min/max: one whole-slot source.
const COMPUTE_25_SCRIPT: &str = "${int(header.k1) + int(header.k2) + int(header.k3) + int(header.k4) + int(header.k5) + min(int(header.k6), int(header.k7)) + max(int(header.k8), int(header.k9)) + int(header.k10) * 2 - int(header.k11) + int(header.k12) + int(header.k13) + int(header.k14) + int(header.k15) + int(header.k16) + int(header.k17) + min(int(header.k18), int(header.k19)) + int(header.k20) + int(header.k21) + int(header.k22) + int(header.k23) + int(header.k24) + int(header.k25)}";

/// Every workload that the guard inspects, as `(qualified_name, group, name)`.
/// Criterion stores the group name with `/` replaced by `_`.
const WORKLOADS: &[(&str, &str, &str)] = &[
    (
        "expression_eval/header_value",
        EXPRESSION_GROUP,
        "header_value",
    ),
    (
        "expression_eval/header_count",
        EXPRESSION_GROUP,
        "header_count",
    ),
    ("expression_eval/body_value", EXPRESSION_GROUP, "body_value"),
    ("expression_eval/complex", EXPRESSION_GROUP, "complex"),
    ("expression_eval/compute_25", EXPRESSION_GROUP, "compute_25"),
    (
        "predicate_eval/header_value",
        PREDICATE_GROUP,
        "header_value",
    ),
    ("predicate_eval/header_eq", PREDICATE_GROUP, "header_eq"),
    ("predicate_eval/header_gt", PREDICATE_GROUP, "header_gt"),
    (
        "predicate_eval/body_contains",
        PREDICATE_GROUP,
        "body_contains",
    ),
    ("predicate_eval/complex", PREDICATE_GROUP, "complex"),
    ("predicate_eval/choice", PREDICATE_GROUP, "choice"),
];

static MEASURED: Mutex<Vec<(&'static str, Estimate)>> = Mutex::new(Vec::new());
/// Workloads criterion selected and invoked the benchmark closure for.
static SELECTED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
static OUTPUT_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Criterion writes no estimates in test/list/profile modes
/// (`Mode::is_benchmark()` is false). Mirrors criterion's own flag handling so
/// those runs skip gate enforcement instead of reading stale files.
///
/// Value-taking flags accept both clap forms (`--profile-time 1` and
/// `--profile-time=1`). `--save-baseline[=<name>]` still writes estimates, so
/// it is not a skip mode and needs no detection here.
fn criterion_measurement_enabled() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| {
        let equals_form = format!("{flag}=");
        args.iter()
            .any(|arg| arg == flag || arg.starts_with(&equals_form))
    };
    let test_mode = match (has("--bench"), has("--test")) {
        (true, true) => true,
        (true, false) => false,
        (false, _) => true,
    };
    !test_mode && !has("--list") && !has("--profile-time")
}

fn qualified_name(group: &str, name: &str) -> &'static str {
    WORKLOADS
        .iter()
        .find(|(_, workload_group, workload_name)| {
            *workload_group == group && *workload_name == name
        })
        .map(|(qualified, _, _)| *qualified)
        .expect("workload must be listed in WORKLOADS")
}

fn output_dir() -> &'static Path {
    OUTPUT_DIR.get_or_init(criterion_output_dir)
}

/// Record that criterion selected this workload. The closure passed to
/// `bench_function` runs only for selected benchmarks, so this is the
/// distinction between "unselected (filtered out)" and "selected but the
/// estimate is missing" (a gate infrastructure failure).
fn mark_selected(qualified: &'static str) {
    if !criterion_measurement_enabled() {
        return;
    }
    SELECTED
        .lock()
        .expect("selected workloads mutex")
        .push(qualified);
}

/// Read the estimate for the measurement that just finished in this process.
/// A missing file means criterion filtered this workload out of the run; the
/// gate fails closed for selected workloads that produced no estimate.
fn record_estimate(group: &str, name: &str) {
    if !criterion_measurement_enabled() {
        return;
    }
    let path = estimate_path(output_dir(), group, name);
    if !path.exists() {
        return;
    }
    let qualified = qualified_name(group, name);
    let estimate = read_estimate(output_dir(), group, name)
        .unwrap_or_else(|err| panic!("perf gate: estimate for {qualified}: {err}"));
    MEASURED
        .lock()
        .expect("measured estimates mutex")
        .push((qualified, estimate));
}

/// Drop the previous run's transient `new/estimates.json` before measuring so
/// a filtered-out workload cannot be mistaken for a fresh measurement.
/// Criterion compares against `base`, so this deletion does not affect it.
fn clear_estimate(group: &str, name: &str) {
    if !criterion_measurement_enabled() {
        return;
    }
    let path = estimate_path(output_dir(), group, name);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => panic!("perf gate: cannot clear {}: {err}", path.display()),
    }
}

fn bench_expression_eval(c: &mut Criterion) {
    let mut group = c.benchmark_group(EXPRESSION_GROUP);

    let lang = SimpleLanguage::new();
    let rt = Runtime::new().expect("tokio runtime");

    let mut ex = Exchange::new(Message::new("hello world"));
    ex.input.set_header("foo", Value::String("bar".to_string()));
    ex.input.set_header("count", Value::Number(42.into()));

    // Dedicated fixture for the 25-step workload: k1..k25 = "1".
    let mut compute_ex = Exchange::new(Message::new("hello world"));
    for i in 1..=25 {
        compute_ex
            .input
            .set_header(format!("k{i}"), Value::String("1".to_string()));
    }

    // Value templates and one whole-slot source. Comparison scripts live in
    // the predicate bench; expression evaluation is measured on slots/raw text.
    let expressions = [
        ("header_value", "${header.foo}".to_string()),
        ("header_count", "${header.count}".to_string()),
        ("body_value", "${body}".to_string()),
        (
            "complex",
            "route-${header.foo}-${header.count}-payload:${body}".to_string(),
        ),
        ("compute_25", COMPUTE_25_SCRIPT.to_string()),
    ];

    for (name, script) in &expressions {
        let exchange = if *name == "compute_25" {
            &compute_ex
        } else {
            &ex
        };
        let expr = lang
            .create_expression(script)
            .expect("expression should parse");
        if *name == "compute_25" {
            // Correctness assert once, outside the timed loop.
            let value = rt
                .block_on(expr.evaluate(exchange))
                .expect("compute_25 evaluation should succeed");
            assert_eq!(
                value,
                Value::Number(21.into()),
                "compute_25 must evaluate to 21"
            );
        }
        clear_estimate(EXPRESSION_GROUP, name);
        group.bench_function(*name, |b| {
            mark_selected(qualified_name(EXPRESSION_GROUP, name));
            b.iter(|| {
                let value = rt
                    .block_on(expr.evaluate(black_box(exchange)))
                    .expect("expression evaluation should succeed");
                black_box(value);
            })
        });
        record_estimate(EXPRESSION_GROUP, name);
    }

    group.finish();
}

fn bench_predicate_eval(c: &mut Criterion) {
    let mut group = c.benchmark_group(PREDICATE_GROUP);

    let lang = SimpleLanguage::new();
    let rt = Runtime::new().expect("tokio runtime");

    let mut ex = Exchange::new(Message::new("hello world"));
    ex.input.set_header("foo", Value::String("bar".to_string()));
    ex.input.set_header("count", Value::Number(42.into()));

    let mut choice_ex = Exchange::new(Message::new("hello world"));
    choice_ex
        .input
        .set_header("at", Value::String("1749999999999".to_string()));
    choice_ex
        .input
        .set_header("body", Value::String("x".to_string()));
    choice_ex.set_property("nowMs", Value::String("1750000000000".to_string()));

    let predicates = [
        ("header_value", "${header.foo} == 'bar'".to_string()),
        ("header_eq", "${header.foo} == 'bar'".to_string()),
        ("header_gt", "${header.count} > 10".to_string()),
        ("body_contains", "${body} contains 'world'".to_string()),
        ("complex", COMPLEX_PREDICATE_SCRIPT.to_string()),
        ("choice", CHOICE_SCRIPT.to_string()),
    ];

    for (name, script) in &predicates {
        let predicate = lang
            .create_predicate(script)
            .expect("predicate should parse");
        let exchange = if *name == "choice" { &choice_ex } else { &ex };
        clear_estimate(PREDICATE_GROUP, name);
        group.bench_function(*name, |b| {
            mark_selected(qualified_name(PREDICATE_GROUP, name));
            b.iter(|| {
                let matched = rt
                    .block_on(predicate.matches(black_box(exchange)))
                    .expect("predicate evaluation should succeed");
                black_box(matched);
            })
        });
        record_estimate(PREDICATE_GROUP, name);
    }

    group.finish();
}

/// Criterion's output directory resolution (mirrors `default_output_directory`
/// in criterion): `CRITERION_HOME`, then `CARGO_TARGET_DIR`, then the target
/// directory reported by `cargo metadata`, then `target/criterion`.
fn criterion_output_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("CRITERION_HOME") {
        return PathBuf::from(home);
    }
    if let Some(target) = std::env::var_os("CARGO_TARGET_DIR") {
        return PathBuf::from(target).join("criterion");
    }
    cargo_metadata_target_dir()
        .map(|target| target.join("criterion"))
        .unwrap_or_else(|| PathBuf::from("target/criterion"))
}

fn cargo_metadata_target_dir() -> Option<PathBuf> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let marker = "\"target_directory\":\"";
    let start = text.find(marker)? + marker.len();
    let value = &text[start..];
    let end = value.find('"')?;
    Some(PathBuf::from(&value[..end]))
}

#[derive(Clone, Copy)]
struct Estimate {
    /// Criterion's typical estimate: slope when present, mean for flat
    /// sampling. This is the statistic printed as `time: [lo est hi]`.
    typical_ns: f64,
    median_ns: f64,
}

fn estimate_path(output_dir: &Path, group: &str, name: &str) -> PathBuf {
    output_dir
        .join(group.replace('/', "_"))
        .join(name)
        .join("new")
        .join("estimates.json")
}

fn read_estimate(output_dir: &Path, group: &str, name: &str) -> Result<Estimate, String> {
    let path = estimate_path(output_dir, group, name);
    let text = std::fs::read_to_string(&path)
        .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|err| format!("invalid {}: {err}", path.display()))?;
    let point_estimate = |stat: &str| -> Option<f64> {
        json.get(stat)
            .and_then(|value| value.get("point_estimate"))
            .and_then(serde_json::Value::as_f64)
    };
    // `typical() == slope.unwrap_or(mean)`: the same value criterion prints
    // between the `time:` interval bounds.
    let typical_ns = point_estimate("slope")
        .or_else(|| point_estimate("mean"))
        .ok_or_else(|| format!("{} missing slope/mean point_estimate", path.display()))?;
    let median_ns = point_estimate("median")
        .ok_or_else(|| format!("{} missing median.point_estimate", path.display()))?;
    Ok(Estimate {
        typical_ns,
        median_ns,
    })
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn current_host() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|host| host.trim().to_string())
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

fn current_cpu() -> String {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix("model name"))
                .and_then(|rest| rest.split_once(':'))
                .map(|(_, model)| model.trim().to_string())
        })
        .filter(|model| !model.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

struct Baseline {
    host: String,
    cpu: String,
    estimates: HashMap<String, f64>,
}

fn parse_baseline(text: &str) -> Result<Baseline, String> {
    let mut host = None;
    let mut cpu = None;
    let mut estimates = HashMap::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("# host:") {
            host = Some(rest.trim().to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix("# cpu:") {
            cpu = Some(rest.trim().to_string());
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let name = parts
            .next()
            .ok_or_else(|| format!("malformed baseline line: {line}"))?;
        let estimate = parts
            .next()
            .ok_or_else(|| format!("malformed baseline line: {line}"))?
            .parse::<f64>()
            .map_err(|_| format!("malformed baseline estimate: {line}"))?;
        let line_host = parts
            .next()
            .ok_or_else(|| format!("baseline line missing host: {line}"))?;
        let line_cpu = parts.collect::<Vec<_>>().join(" ");
        if line_cpu.is_empty() {
            return Err(format!("baseline line missing cpu: {line}"));
        }
        host.get_or_insert_with(|| line_host.to_string());
        cpu.get_or_insert(line_cpu);
        estimates.insert(name.to_string(), estimate);
    }
    Ok(Baseline {
        host: host.ok_or_else(|| "baseline missing provenance".to_string())?,
        cpu: cpu.ok_or_else(|| "baseline missing provenance".to_string())?,
        estimates,
    })
}

fn check_baseline(path: &str, host: &str, cpu: &str, measured: &HashMap<&'static str, Estimate>) {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("baseline gate: cannot read {path}: {err}"));
    let baseline =
        parse_baseline(&text).unwrap_or_else(|err| panic!("baseline gate: invalid {path}: {err}"));

    let mut mismatched = Vec::new();
    if baseline.host != host {
        mismatched.push(format!("host baseline={} current={host}", baseline.host));
    }
    if baseline.cpu != cpu {
        mismatched.push(format!("cpu baseline={} current={cpu}", baseline.cpu));
    }
    if !mismatched.is_empty() {
        println!(
            "baseline: SKIPPED (provenance mismatch: {})",
            mismatched.join("; ")
        );
        return;
    }

    let mut checked = 0usize;
    let mut max_ratio = 0.0_f64;
    let mut breaches = Vec::new();
    for (qualified, _group, _name) in WORKLOADS {
        let Some(estimate) = measured.get(*qualified) else {
            continue;
        };
        let Some(base_ns) = baseline.estimates.get(*qualified) else {
            println!("baseline: {qualified} absent from {path} (new workload) — skipped");
            continue;
        };
        let current_ns = estimate.typical_ns;
        let ratio = current_ns / base_ns;
        max_ratio = max_ratio.max(ratio);
        let verdict = if ratio > BASELINE_REGRESSION_LIMIT {
            breaches.push(format!(
                "{qualified} baseline_est_ns={base_ns:.2} current_est_ns={current_ns:.2} \
                 ratio={ratio:.3}"
            ));
            "FAIL"
        } else {
            "pass"
        };
        println!(
            "baseline: {qualified} baseline_est_ns={base_ns:.2} current_est_ns={current_ns:.2} \
             ratio={ratio:.3} <= {BASELINE_REGRESSION_LIMIT:.2} {verdict}"
        );
        checked += 1;
    }

    // Baseline workloads not measured in this run are skipped, never judged
    // against stale saved results.
    let not_measured = baseline
        .estimates
        .keys()
        .filter(|name| !measured.contains_key(name.as_str()))
        .count();
    if not_measured > 0 {
        println!(
            "baseline: {not_measured} baseline workload(s) not measured in this run — skipped"
        );
    }

    if checked == 0 {
        println!("baseline: no shared workloads measured in this run — skipped");
        return;
    }

    if !breaches.is_empty() {
        panic!(
            "baseline gate FAIL: {} workload(s) exceed {BASELINE_REGRESSION_LIMIT:.2}x baseline: {}",
            breaches.len(),
            breaches.join("; ")
        );
    }

    println!(
        "baseline: PASS ({checked} shared workloads <= {BASELINE_REGRESSION_LIMIT:.2}x baseline, \
         max_ratio={max_ratio:.3})"
    );
}

fn check_median_gate(
    measured: &HashMap<&'static str, Estimate>,
    qualified: &str,
    label: &str,
    max_us: f64,
) {
    let Some(estimate) = measured.get(qualified) else {
        println!("gate: {qualified} not measured in this run — skipped");
        return;
    };
    let median_us = estimate.median_ns / 1000.0;
    if median_us > max_us {
        panic!("{label} gate FAIL: median_us={median_us:.2} exceeds max_us={max_us:.2}");
    }
    println!("gate: {qualified} median_us={median_us:.2} <= max_us={max_us:.2}");
}

/// Runs after both measurement functions, enforcing gates only on the
/// workloads those functions recorded in this process.
///
/// Three states per workload:
/// - selected and measured: gated below;
/// - selected but missing: PANIC (fail closed — e.g. `--discard-baseline` or
///   an estimate write failure);
/// - unselected (filtered out): legitimately absent, skipped.
fn bench_gates(_c: &mut Criterion) {
    if !criterion_measurement_enabled() {
        println!("bench gates skipped (test mode)");
        return;
    }
    let mut selected = SELECTED.lock().expect("selected workloads mutex").clone();
    selected.sort_unstable();
    selected.dedup();
    if selected.is_empty() {
        println!("bench gates skipped (no benchmarks selected)");
        return;
    }
    let measured: HashMap<&'static str, Estimate> = MEASURED
        .lock()
        .expect("measured estimates mutex")
        .iter()
        .copied()
        .collect();
    for qualified in &selected {
        if !measured.contains_key(*qualified) {
            panic!(
                "gate infrastructure failure: no measurement for {qualified} \
                 (check for --discard-baseline / estimate write failures)"
            );
        }
    }

    let max_choice_us = env_f64("SIMPLE_CHOICE_MAX_US", DEFAULT_MAX_CHOICE_US);
    let max_25step_us = env_f64("SIMPLE_25STEP_MAX_US", DEFAULT_MAX_25STEP_US);

    check_median_gate(&measured, "predicate_eval/choice", "choice", max_choice_us);
    check_median_gate(
        &measured,
        "expression_eval/compute_25",
        "25-step",
        max_25step_us,
    );

    if let Ok(path) = std::env::var("SIMPLE_EVAL_BASELINE") {
        check_baseline(&path, &current_host(), &current_cpu(), &measured);
    }
}

criterion_group!(
    benches,
    bench_expression_eval,
    bench_predicate_eval,
    bench_gates
);
criterion_main!(benches);
