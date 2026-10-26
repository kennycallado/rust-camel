//! Perf guard bench: Rhai property conversion scaling (mission 416 / bd rc-m01r9).
//!
//! This bench reproduces the current O(K) property-conversion cost as a
//! machine-readable guard. It is guard-first: on master the gated ratios are
//! expected to FAIL (read R ~ 8, mutating R ~ 24). The phase-2 fix is expected
//! to flip them to passing without changing this bench.
//!
//! Envelopes come from bd rc-m01r9 / mission 416. The mutating envelope
//! (R <= 3.0) requires removing the serde clone and the full write-back diff.
//! TODO(rc-m01r9): if the phase-2 e_gpt ruling rules out nested laziness, the
//! D-mutating gate becomes a slope/constant-factor check instead of a ratio.
//!
//! Measurement protocol:
//! - K = number of map entries, each `"k{i}" -> "v{i}"`.
//! - Shapes: W = K top-level properties `k{i}`, each a one-entry map; D = one
//!   `buffer` property holding a K-entry map. Both carry a constant 128-element
//!   `chunk` property.
//! - Warmup: 20 evals per K before sampling.
//! - Sampling: 15 samples, each sample = total time of a 20-eval batch; the
//!   reported ns/eval is `median(batch_ns) / 20` (batch-median variant).
//! - K=100 and K=5000 batches are interleaved (100, 5000, 100, 5000, ...) so
//!   thermal/load drift hits both Ks equally.
//! - Metric: R = median_ns(K=5000) / median_ns(K=100).
//! - Up to 3 rounds per gated case; a case PASSES if any round meets its
//!   envelope. The lines printed for a case come from its best (lowest-R)
//!   round, so each (shape, mode, K) appears exactly once.
//! - K=100 lines report `R=1.00` literally (ratio against itself). K=1000 is
//!   printed-only and never gated; its R is relative to the same case's K=100
//!   median.
//!
//! Exit-code contract (mission-approved env-flag form):
//! - Default: exit 0 iff ALL gated cases pass, exit 1 otherwise.
//! - `RHAI_GUARD_EXPECT_FAIL=1` (CI on master): exit 0 iff ALL gated cases
//!   FAIL. Any passing case is unexpected -> exit 1 naming that case.
//!
//! Runs on a current-thread Tokio runtime, no tracing init, no file I/O.

use std::time::Instant;

use camel_api::{Exchange, Message, Value};
use camel_language_api::{Expression, Language, MutatingExpression, RhaiLimitsConfig};
use camel_language_rhai::RhaiLanguage;
use tokio::runtime::Runtime;

const K_LO: usize = 100;
const K_HI: usize = 5_000;
const K_PRINT_ONLY: usize = 1_000;
const BATCH: usize = 20;
const SAMPLES: usize = 15;
const WARMUP_EVALS: usize = 20;
const MAX_ROUNDS: usize = 3;
const CONTROL_SCRIPT: &str = r#"property("buffer").len()"#;

#[derive(Clone, Copy, Debug)]
enum Shape {
    Wide,
    Deep,
}

impl Shape {
    fn label(self) -> &'static str {
        match self {
            Shape::Wide => "W",
            Shape::Deep => "D",
        }
    }

    fn read_script(self) -> &'static str {
        match self {
            Shape::Wide => r#"property("k0")"#,
            Shape::Deep => r#"property("buffer")["k0"]"#,
        }
    }

    fn mutating_script(self) -> &'static str {
        match self {
            Shape::Wide => r#"let x = property("k0"); properties["out"] = x;"#,
            Shape::Deep => r#"let x = property("buffer")["k0"]; properties["out"] = x;"#,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Mode {
    Read,
    Mutating,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Mode::Read => "read",
            Mode::Mutating => "mutating",
        }
    }

    fn envelope(self) -> f64 {
        match self {
            Mode::Read => 2.0,
            // TODO(rc-m01r9): if the phase-2 e_gpt ruling rules out nested
            // laziness, replace the D-mutating ratio gate with a
            // slope/constant-factor check at this constant.
            Mode::Mutating => 3.0,
        }
    }
}

fn chunk_value() -> Value {
    Value::Array((0..128).map(Value::from).collect())
}

fn single_entry_map(i: usize) -> Value {
    Value::Object(
        [(format!("k{i}"), Value::String(format!("v{i}")))]
            .into_iter()
            .collect(),
    )
}

fn deep_map(k: usize) -> Value {
    Value::Object(
        (0..k)
            .map(|i| (format!("k{i}"), Value::String(format!("v{i}"))))
            .collect(),
    )
}

fn exchange(shape: Shape, k: usize) -> Exchange {
    let mut ex = Exchange::new(Message::new(""));
    match shape {
        Shape::Wide => {
            for i in 0..k {
                ex.set_property(format!("k{i}"), single_entry_map(i));
            }
        }
        Shape::Deep => {
            ex.set_property("buffer", deep_map(k));
        }
    }
    ex.set_property("chunk", chunk_value());
    ex
}

fn median(mut samples: Vec<u128>) -> u128 {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn time_read_batch(rt: &Runtime, expr: &dyn Expression, ex: &Exchange, evals: usize) -> u128 {
    let start = Instant::now();
    rt.block_on(async {
        for _ in 0..evals {
            expr.evaluate(ex).await.expect("read evaluate must succeed");
        }
    });
    start.elapsed().as_nanos()
}

fn time_mutating_batch(
    rt: &Runtime,
    expr: &dyn MutatingExpression,
    ex: &mut Exchange,
    evals: usize,
) -> u128 {
    let start = Instant::now();
    rt.block_on(async {
        for _ in 0..evals {
            expr.evaluate(ex)
                .await
                .expect("mutating evaluate must succeed");
        }
    });
    start.elapsed().as_nanos()
}

struct Round {
    m100: u128,
    m5000: u128,
    ratio: f64,
}

fn measure_read_round(rt: &Runtime, lang: &RhaiLanguage, shape: Shape) -> Round {
    let expr = lang
        .create_expression(shape.read_script())
        .expect("read script must compile");
    let lo = exchange(shape, K_LO);
    let hi = exchange(shape, K_HI);

    time_read_batch(rt, expr.as_ref(), &lo, WARMUP_EVALS);
    time_read_batch(rt, expr.as_ref(), &hi, WARMUP_EVALS);

    let mut lo_samples = Vec::with_capacity(SAMPLES);
    let mut hi_samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        lo_samples.push(time_read_batch(rt, expr.as_ref(), &lo, BATCH));
        hi_samples.push(time_read_batch(rt, expr.as_ref(), &hi, BATCH));
    }

    let m100 = median(lo_samples) / BATCH as u128;
    let m5000 = median(hi_samples) / BATCH as u128;
    Round {
        m100,
        m5000,
        ratio: m5000 as f64 / m100 as f64,
    }
}

fn measure_mutating_round(rt: &Runtime, lang: &RhaiLanguage, shape: Shape) -> Round {
    let expr = lang
        .create_mutating_expression(shape.mutating_script())
        .expect("mutating script must compile");
    let mut lo = exchange(shape, K_LO);
    let mut hi = exchange(shape, K_HI);

    time_mutating_batch(rt, expr.as_ref(), &mut lo, WARMUP_EVALS);
    time_mutating_batch(rt, expr.as_ref(), &mut hi, WARMUP_EVALS);

    let mut lo_samples = Vec::with_capacity(SAMPLES);
    let mut hi_samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        lo_samples.push(time_mutating_batch(rt, expr.as_ref(), &mut lo, BATCH));
        hi_samples.push(time_mutating_batch(rt, expr.as_ref(), &mut hi, BATCH));
    }

    let m100 = median(lo_samples) / BATCH as u128;
    let m5000 = median(hi_samples) / BATCH as u128;
    Round {
        m100,
        m5000,
        ratio: m5000 as f64 / m100 as f64,
    }
}

fn measure_read_at(rt: &Runtime, lang: &RhaiLanguage, script: &str, ex: &Exchange) -> u128 {
    let expr = lang
        .create_expression(script)
        .expect("read script must compile");
    time_read_batch(rt, expr.as_ref(), ex, WARMUP_EVALS);
    let samples = (0..SAMPLES)
        .map(|_| time_read_batch(rt, expr.as_ref(), ex, BATCH))
        .collect();
    median(samples) / BATCH as u128
}

fn measure_mutating_at(rt: &Runtime, lang: &RhaiLanguage, script: &str, ex: &mut Exchange) -> u128 {
    let expr = lang
        .create_mutating_expression(script)
        .expect("mutating script must compile");
    time_mutating_batch(rt, expr.as_ref(), ex, WARMUP_EVALS);
    let samples = (0..SAMPLES)
        .map(|_| time_mutating_batch(rt, expr.as_ref(), ex, BATCH))
        .collect();
    median(samples) / BATCH as u128
}

struct CaseReport {
    shape: Shape,
    mode: Mode,
    ratio: f64,
    pass: bool,
}

fn main() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("current-thread runtime must build");
    let lang = RhaiLanguage::with_limits(RhaiLimitsConfig {
        execution_timeout_ms: Some(60_000),
        ..Default::default()
    });

    let cases = [
        (Shape::Wide, Mode::Read),
        (Shape::Deep, Mode::Read),
        (Shape::Wide, Mode::Mutating),
        (Shape::Deep, Mode::Mutating),
    ];

    let mut reports = Vec::with_capacity(cases.len());
    for (shape, mode) in cases {
        let envelope = mode.envelope();
        let mut best: Option<Round> = None;
        for _ in 0..MAX_ROUNDS {
            let round = match mode {
                Mode::Read => measure_read_round(&rt, &lang, shape),
                Mode::Mutating => measure_mutating_round(&rt, &lang, shape),
            };
            if best.as_ref().is_none_or(|b| round.ratio < b.ratio) {
                best = Some(round);
            }
            if best.as_ref().is_some_and(|b| b.ratio <= envelope) {
                break;
            }
        }
        let best = best.expect("at least one round must run");

        let m1000 = match mode {
            Mode::Read => measure_read_at(
                &rt,
                &lang,
                shape.read_script(),
                &exchange(shape, K_PRINT_ONLY),
            ),
            Mode::Mutating => {
                let mut ex = exchange(shape, K_PRINT_ONLY);
                measure_mutating_at(&rt, &lang, shape.mutating_script(), &mut ex)
            }
        };
        let ratio1000 = m1000 as f64 / best.m100 as f64;
        let pass = best.ratio <= envelope;

        println!(
            "GUARD {} {} {} {} 1.00",
            shape.label(),
            mode.label(),
            K_LO,
            best.m100
        );
        println!(
            "GUARD {} {} {} {} {:.2}",
            shape.label(),
            mode.label(),
            K_HI,
            best.m5000,
            best.ratio
        );
        println!(
            "GUARD {} {} {} {} {:.2}",
            shape.label(),
            mode.label(),
            K_PRINT_ONLY,
            m1000,
            ratio1000
        );
        println!(
            "GUARD-VERDICT {} {} R={:.2} envelope={:.1} pass={}",
            shape.label(),
            mode.label(),
            best.ratio,
            envelope,
            pass
        );
        reports.push(CaseReport {
            shape,
            mode,
            ratio: best.ratio,
            pass,
        });
    }

    let control_ex = exchange(Shape::Deep, K_PRINT_ONLY);
    let control_ns = measure_read_at(&rt, &lang, CONTROL_SCRIPT, &control_ex);
    println!("GUARD D read-control {} {} -", K_PRINT_ONLY, control_ns);

    let expect_fail = matches!(
        std::env::var("RHAI_GUARD_EXPECT_FAIL"),
        Ok(v) if v == "1"
    );
    let all_pass = reports.iter().all(|r| r.pass);
    let all_fail = reports.iter().all(|r| !r.pass);

    if expect_fail {
        if all_fail {
            println!("GUARD-EXPECT-FAIL ok: all gated cases failed as expected");
            std::process::exit(0);
        }
        let unexpectedly_passing: Vec<String> = reports
            .iter()
            .filter(|r| r.pass)
            .map(|r| format!("{} {} R={:.2}", r.shape.label(), r.mode.label(), r.ratio))
            .collect();
        eprintln!(
            "GUARD-EXPECT-FAIL violated: case unexpectedly passed: {}",
            unexpectedly_passing.join(", ")
        );
        std::process::exit(1);
    }

    std::process::exit(if all_pass { 0 } else { 1 });
}
