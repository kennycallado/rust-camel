//! Perf guard bench: per-evaluation Rhai overhead (change `rhai-engine-reuse`).
//!
//! Hand-rolled (no criterion). Workloads, phases, and gates follow the blessed
//! plan of `rhai-engine-reuse`; this bench records the pre-change baseline and
//! gates the post-change engine work.
//!
//! Output contract (machine-readable):
//! - `overhead: <workload> <phase> median_us=<m> p95_us=<p>` for every
//!   workload x {full, construction, conversion, execution}.
//! - `full` is the gate number: one complete public evaluation call per
//!   sample. The three phase proxies are rhai-direct and reported only.
//! - `construction`/`conversion`/`execution` are rhai-direct proxies for the
//!   private `create_base_engine` core, the `Scope` value construction, and
//!   the AST execution respectively. Never summed into a gate.
//!
//! Gates (record mode skips all of them):
//! - `RHAI_EVAL_OVERHEAD_MAX_TRIVIAL_US` (default 100): trivial full median.
//! - `RHAI_EVAL_OVERHEAD_MAX_CHOICE_US` (default 250): choice10 full median.
//! - `RHAI_EVAL_OVERHEAD_BASELINE=<path>`: optional relative check; a
//!   provenance mismatch (host or cpu) only prints a SKIPPED line and does
//!   not satisfy binding local acceptance.
//! - `RHAI_EVAL_OVERHEAD_RECORD_BASELINE=1`: print provenance header plus all
//!   overhead lines, exit 0.
//!
//! Percentiles are nearest-rank over the per-sample vector: p95 is the
//! `ceil(0.95 * n)`-th smallest sample, median the 50th.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use camel_language_api::{
    Body, Exchange, Expression, Language, Message, MutatingExpression, Predicate, RhaiLimitsConfig,
    Value,
};
use camel_language_rhai::RhaiLanguage;
use rhai::packages::{Package, StandardPackage};
use rhai::{Dynamic, Engine, Map, Scope};
use tokio::runtime::Runtime;

const WARMUP: usize = 50;
const SAMPLES_FULL: usize = 400;
const SAMPLES_PHASE: usize = 200;
const P95_RATIO_LIMIT: f64 = 1.10;

const DEFAULT_MAX_TRIVIAL_US: f64 = 100.0;
const DEFAULT_MAX_CHOICE_US: f64 = 250.0;

const TRIVIAL_SCRIPT: &str = "";
const PREDICATE_SCRIPT: &str = r#"header("h1") == "v1" && body == "hi""#;
const P20_SCRIPT: &str = r#"property("k19")"#;
const P50_SCRIPT: &str = r#"property("k49")"#;
const MUTATING_SCRIPT: &str =
    r#"let x = header("h0"); properties["out"] = x; headers["h1"] = "m";"#;

#[derive(Clone, Copy, Debug)]
enum Kind {
    Expression,
    Predicate,
    Mutating,
}

struct Workload {
    name: &'static str,
    kind: Kind,
    script: String,
    build_exchange: fn() -> Exchange,
}

fn trivial_exchange() -> Exchange {
    let mut msg = Message::new("hi");
    msg.set_header("h1", "v1");
    let mut ex = Exchange::new(msg);
    ex.set_property("p1", "v1");
    ex
}

fn exchange_with_entries(headers: usize, properties: usize) -> Exchange {
    let mut msg = Message::new("hi");
    for i in 0..headers {
        msg.set_header(format!("h{i}"), format!("v{i}"));
    }
    let mut ex = Exchange::new(msg);
    for i in 0..properties {
        ex.set_property(format!("k{i}"), format!("v{i}"));
    }
    ex
}

fn choice_script() -> String {
    (0..10)
        .map(|i| format!(r#"header("h{i}") == "v{i}" && property("k{i}") == "v{i}""#))
        .collect::<Vec<_>>()
        .join(" && ")
}

fn workloads() -> Vec<Workload> {
    vec![
        Workload {
            name: "trivial",
            kind: Kind::Expression,
            script: TRIVIAL_SCRIPT.to_string(),
            build_exchange: trivial_exchange,
        },
        Workload {
            name: "predicate",
            kind: Kind::Predicate,
            script: PREDICATE_SCRIPT.to_string(),
            build_exchange: trivial_exchange,
        },
        Workload {
            name: "20h20p",
            kind: Kind::Expression,
            script: P20_SCRIPT.to_string(),
            build_exchange: || exchange_with_entries(20, 20),
        },
        Workload {
            name: "50h50p",
            kind: Kind::Expression,
            script: P50_SCRIPT.to_string(),
            build_exchange: || exchange_with_entries(50, 50),
        },
        Workload {
            name: "mutating",
            kind: Kind::Mutating,
            script: MUTATING_SCRIPT.to_string(),
            build_exchange: || exchange_with_entries(20, 20),
        },
        Workload {
            name: "choice10",
            kind: Kind::Predicate,
            script: choice_script(),
            build_exchange: || exchange_with_entries(20, 20),
        },
    ]
}

fn measure_full_expression(rt: &Runtime, expr: &dyn Expression, ex: &Exchange) -> Vec<f64> {
    for _ in 0..WARMUP {
        rt.block_on(expr.evaluate(ex))
            .expect("expression eval must succeed");
    }
    let mut samples = Vec::with_capacity(SAMPLES_FULL);
    for _ in 0..SAMPLES_FULL {
        let start = Instant::now();
        rt.block_on(expr.evaluate(ex))
            .expect("expression eval must succeed");
        samples.push(start.elapsed().as_secs_f64() * 1e6);
    }
    samples
}

fn measure_full_predicate(rt: &Runtime, pred: &dyn Predicate, ex: &Exchange) -> Vec<f64> {
    for _ in 0..WARMUP {
        rt.block_on(pred.matches(ex))
            .expect("predicate eval must succeed");
    }
    let mut samples = Vec::with_capacity(SAMPLES_FULL);
    for _ in 0..SAMPLES_FULL {
        let start = Instant::now();
        rt.block_on(pred.matches(ex))
            .expect("predicate eval must succeed");
        samples.push(start.elapsed().as_secs_f64() * 1e6);
    }
    samples
}

fn measure_full_mutating(
    rt: &Runtime,
    expr: &dyn MutatingExpression,
    build_exchange: fn() -> Exchange,
) -> Vec<f64> {
    // A no-change transaction commits no entries, so every timed sample needs
    // a fixture whose `h1` is still `v1` and whose `out` property is absent.
    // The rebuild happens outside the timed region.
    let mut ex = build_exchange();
    for _ in 0..WARMUP {
        ex = build_exchange();
        rt.block_on(expr.evaluate(&mut ex))
            .expect("mutating eval must succeed");
    }
    let mut samples = Vec::with_capacity(SAMPLES_FULL);
    for _ in 0..SAMPLES_FULL {
        ex = build_exchange();
        let start = Instant::now();
        rt.block_on(expr.evaluate(&mut ex))
            .expect("mutating eval must succeed");
        samples.push(start.elapsed().as_secs_f64() * 1e6);
    }
    // Guard against silently timing an eval that never commits: the last
    // sample must carry both scripted writes.
    assert_eq!(
        ex.input.headers.get("h1"),
        Some(&Value::String("m".to_string())),
        "mutating workload must commit headers[\"h1\"]"
    );
    assert!(
        ex.properties.contains_key("out"),
        "mutating workload must commit properties[\"out\"]"
    );
    samples
}

fn measure_full(rt: &Runtime, lang: &RhaiLanguage, workload: &Workload) -> Vec<f64> {
    match workload.kind {
        Kind::Expression => {
            let expr = lang
                .create_expression(&workload.script)
                .expect("expression must compile");
            measure_full_expression(rt, expr.as_ref(), &(workload.build_exchange)())
        }
        Kind::Predicate => {
            let pred = lang
                .create_predicate(&workload.script)
                .expect("predicate must compile");
            measure_full_predicate(rt, pred.as_ref(), &(workload.build_exchange)())
        }
        Kind::Mutating => {
            let expr = lang
                .create_mutating_expression(&workload.script)
                .expect("mutating expression must compile");
            measure_full_mutating(rt, expr.as_ref(), workload.build_exchange)
        }
    }
}

/// rhai-direct proxy for the `Scope` value construction: body `Dynamic` plus
/// one `rhai::Map` each for headers and properties. The fixtures only hold
/// strings, matching the workload shapes.
fn build_scope_values(ex: &Exchange) -> (Dynamic, Map, Map) {
    let body = match &ex.input.body {
        Body::Text(s) | Body::Xml(s) => Dynamic::from(s.clone()),
        Body::Empty => Dynamic::UNIT,
        Body::Bytes(b) => Dynamic::from(rhai::Blob::from(b.as_ref())),
        _ => Dynamic::UNIT,
    };
    let mut headers = Map::new();
    for (k, v) in &ex.input.headers {
        if let Value::String(s) = v {
            headers.insert(k.clone().into(), Dynamic::from(s.clone()));
        }
    }
    let mut properties = Map::new();
    for (k, v) in &ex.properties {
        if let Value::String(s) = v {
            properties.insert(k.clone().into(), Dynamic::from(s.clone()));
        }
    }
    (body, headers, properties)
}

/// rhai-direct proxy for the private `create_base_engine` core plus the
/// snapshot-reader registration: `Engine::new_raw` + `StandardPackage` +
/// snapshot-shaped `header`/`property` reader closures.
fn build_proxy_engine(headers: &Map, properties: &Map) -> Engine {
    let mut engine = Engine::new_raw();
    StandardPackage::new().register_into_engine(&mut engine);
    let header_snapshot = headers.clone();
    let property_snapshot = properties.clone();
    engine.register_fn("header", move |name: String| -> Dynamic {
        header_snapshot
            .get(name.as_str())
            .cloned()
            .unwrap_or(Dynamic::UNIT)
    });
    engine.register_fn("property", move |name: String| -> Dynamic {
        property_snapshot
            .get(name.as_str())
            .cloned()
            .unwrap_or(Dynamic::UNIT)
    });
    engine
}

fn measure_construction(headers: &Map, properties: &Map) -> Vec<f64> {
    for _ in 0..WARMUP {
        std::hint::black_box(build_proxy_engine(headers, properties));
    }
    let mut samples = Vec::with_capacity(SAMPLES_PHASE);
    for _ in 0..SAMPLES_PHASE {
        let start = Instant::now();
        let engine = build_proxy_engine(headers, properties);
        std::hint::black_box(&engine);
        samples.push(start.elapsed().as_secs_f64() * 1e6);
    }
    samples
}

fn measure_conversion(ex: &Exchange) -> Vec<f64> {
    for _ in 0..WARMUP {
        let _ = std::hint::black_box(build_scope_values(ex));
    }
    let mut samples = Vec::with_capacity(SAMPLES_PHASE);
    for _ in 0..SAMPLES_PHASE {
        let start = Instant::now();
        let values = build_scope_values(ex);
        samples.push(start.elapsed().as_secs_f64() * 1e6);
        let _ = std::hint::black_box(values);
    }
    samples
}

fn measure_execution(script: &str, ex: &Exchange) -> Vec<f64> {
    let (body, headers, properties) = build_scope_values(ex);
    let engine = build_proxy_engine(&headers, &properties);
    let ast = engine.compile(script).expect("proxy AST must compile");
    let mut template = Scope::new();
    template.push("body", body);
    template.push("headers", headers);
    template.push("properties", properties);

    for _ in 0..WARMUP {
        let mut scope = template.clone();
        let _ = std::hint::black_box(
            engine
                .eval_ast_with_scope::<Dynamic>(&mut scope, &ast)
                .expect("proxy eval must succeed"),
        );
    }
    let mut samples = Vec::with_capacity(SAMPLES_FULL);
    for _ in 0..SAMPLES_FULL {
        // Clone the template scope outside the timer: only the AST execution
        // is measured as the execution phase.
        let mut scope = template.clone();
        let start = Instant::now();
        let value = engine
            .eval_ast_with_scope::<Dynamic>(&mut scope, &ast)
            .expect("proxy eval must succeed");
        samples.push(start.elapsed().as_secs_f64() * 1e6);
        drop(scope);
        let _ = std::hint::black_box(value);
    }
    samples
}

#[derive(Clone, Copy)]
struct Stat {
    median_us: f64,
    p95_us: f64,
}

fn nearest_rank(sorted: &[f64], pct: f64) -> f64 {
    let n = sorted.len();
    let rank = ((pct / 100.0) * n as f64).ceil() as usize;
    sorted[rank.clamp(1, n) - 1]
}

fn stat(mut samples: Vec<f64>) -> Stat {
    samples.sort_by(f64::total_cmp);
    Stat {
        median_us: nearest_rank(&samples, 50.0),
        p95_us: nearest_rank(&samples, 95.0),
    }
}

struct WorkloadResult {
    name: &'static str,
    full: Stat,
    construction: Stat,
    conversion: Stat,
    execution: Stat,
}

impl WorkloadResult {
    fn phases(&self) -> [(&'static str, Stat); 4] {
        [
            ("full", self.full),
            ("construction", self.construction),
            ("conversion", self.conversion),
            ("execution", self.execution),
        ]
    }
}

fn env_flag(key: &str) -> bool {
    matches!(std::env::var(key), Ok(v) if v == "1")
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn current_host() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
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

/// UTC date (`YYYY-MM-DD`) from the unix epoch without a date-crate
/// dependency (Howard Hinnant's civil-from-days algorithm).
fn utc_date() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 {
        yoe + era * 400 + 1
    } else {
        yoe + era * 400
    };
    format!("{year:04}-{month:02}-{day:02}")
}

struct Baseline {
    host: String,
    cpu: String,
    full_p95: HashMap<String, f64>,
}

fn parse_baseline(text: &str) -> Result<Baseline, String> {
    let mut host = None;
    let mut cpu = None;
    let mut full_p95 = HashMap::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# baseline:").map(str::trim) {
            let host_start = rest
                .find("host=")
                .map(|i| i + "host=".len())
                .ok_or_else(|| "provenance header missing host=".to_string())?;
            let cpu_marker = rest
                .find(" cpu=")
                .ok_or_else(|| "provenance header missing cpu=".to_string())?;
            if cpu_marker < host_start {
                return Err("provenance header fields out of order".to_string());
            }
            host = Some(rest[host_start..cpu_marker].to_string());
            let cpu_start = cpu_marker + " cpu=".len();
            let date_marker = rest[cpu_start..]
                .find(" date=")
                .map(|i| cpu_start + i)
                .ok_or_else(|| "provenance header missing date=".to_string())?;
            cpu = Some(rest[cpu_start..date_marker].to_string());
            continue;
        }
        let Some(rest) = line.strip_prefix("overhead: ") else {
            continue;
        };
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() < 4 || parts[1] != "full" {
            continue;
        }
        let p95 = parts[3]
            .strip_prefix("p95_us=")
            .ok_or_else(|| format!("malformed overhead line: {line}"))?
            .parse::<f64>()
            .map_err(|_| format!("malformed p95 value: {line}"))?;
        full_p95.insert(parts[0].to_string(), p95);
    }
    Ok(Baseline {
        host: host.ok_or_else(|| "baseline missing provenance header".to_string())?,
        cpu: cpu.ok_or_else(|| "baseline missing provenance header".to_string())?,
        full_p95,
    })
}

/// Returns true when the relative check ran and found no breach; false when it
/// breached. A provenance mismatch prints SKIPPED and is not a breach.
fn check_baseline(path: &str, host: &str, cpu: &str, results: &[WorkloadResult]) -> bool {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("overhead: p95-check FAILED (cannot read baseline {path}: {err})");
            return false;
        }
    };
    let baseline = match parse_baseline(&text) {
        Ok(baseline) => baseline,
        Err(err) => {
            eprintln!("overhead: p95-check FAILED (invalid baseline {path}: {err})");
            return false;
        }
    };

    let mut mismatched = String::new();
    if baseline.host != host {
        let _ = write!(
            mismatched,
            "host baseline={} current={}",
            baseline.host, host
        );
    }
    if baseline.cpu != cpu {
        if !mismatched.is_empty() {
            mismatched.push_str("; ");
        }
        let _ = write!(mismatched, "cpu baseline={} current={}", baseline.cpu, cpu);
    }
    if !mismatched.is_empty() {
        println!("overhead: p95-check SKIPPED (provenance mismatch: {mismatched})");
        return true;
    }

    let mut breached = false;
    for result in results {
        let Some(base_p95) = baseline.full_p95.get(result.name) else {
            eprintln!(
                "overhead: p95-check BREACH workload={} missing from baseline",
                result.name
            );
            breached = true;
            continue;
        };
        let limit = base_p95 * P95_RATIO_LIMIT;
        if result.full.p95_us > limit {
            eprintln!(
                "overhead: p95-check BREACH workload={} baseline_p95_us={base_p95:.2} limit_us={limit:.2} current_p95_us={:.2}",
                result.name, result.full.p95_us
            );
            breached = true;
        }
    }
    if breached {
        return false;
    }
    println!("overhead: p95-check PASS (all workloads full p95 <= {P95_RATIO_LIMIT:.2}x baseline)");
    true
}

fn main() {
    let record = env_flag("RHAI_EVAL_OVERHEAD_RECORD_BASELINE");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("current-thread runtime must build");
    let lang = RhaiLanguage::with_limits(RhaiLimitsConfig {
        execution_timeout_ms: Some(60_000),
        ..Default::default()
    });

    let host = current_host();
    let cpu = current_cpu();

    let mut results = Vec::new();
    for workload in workloads() {
        let ex = (workload.build_exchange)();
        let full = stat(measure_full(&rt, &lang, &workload));
        let (_, headers, properties) = build_scope_values(&ex);
        results.push(WorkloadResult {
            name: workload.name,
            full,
            construction: stat(measure_construction(&headers, &properties)),
            conversion: stat(measure_conversion(&ex)),
            execution: stat(measure_execution(&workload.script, &ex)),
        });
    }

    if record {
        println!(
            "# baseline: host={host} cpu={cpu} date={} samples={SAMPLES_FULL}",
            utc_date()
        );
    }
    for result in &results {
        for (phase, stat) in result.phases() {
            println!(
                "overhead: {} {} median_us={:.2} p95_us={:.2}",
                result.name, phase, stat.median_us, stat.p95_us
            );
        }
    }

    if record {
        std::process::exit(0);
    }

    let max_trivial = env_f64("RHAI_EVAL_OVERHEAD_MAX_TRIVIAL_US", DEFAULT_MAX_TRIVIAL_US);
    let max_choice = env_f64("RHAI_EVAL_OVERHEAD_MAX_CHOICE_US", DEFAULT_MAX_CHOICE_US);

    let mut failed = false;
    for result in &results {
        let (limit, gate) = match result.name {
            "trivial" => (max_trivial, "GATE-A trivial"),
            "choice10" => (max_choice, "GATE-A choice10"),
            _ => continue,
        };
        if result.full.median_us > limit {
            eprintln!(
                "{gate} FAIL: full median_us={:.2} exceeds max_us={limit:.2}",
                result.full.median_us
            );
            failed = true;
        } else {
            println!(
                "{gate} pass: full median_us={:.2} <= max_us={limit:.2}",
                result.full.median_us
            );
        }
    }

    if let Ok(path) = std::env::var("RHAI_EVAL_OVERHEAD_BASELINE")
        && !check_baseline(&path, &host, &cpu, &results)
    {
        failed = true;
    }

    std::process::exit(if failed { 1 } else { 0 });
}
