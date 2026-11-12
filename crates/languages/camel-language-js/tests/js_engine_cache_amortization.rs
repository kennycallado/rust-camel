//! Amortization timing test for the js-engine-cache worker.
//!
//! The CI assertion is the machine-independent ratio only — the median
//! per-eval time across 6 steady-state batches (after a 100-eval untimed
//! warm-up) must be under 25% of eval 1 (≥ 4x amortization, dev-profile CI
//! with margin); there is no absolute-ms assertion (CI-stable principle).
//!
//! Measured on the dev box (2026-09-12, `cargo test -p camel-language-js
//! --test js_engine_cache_amortization`, stable across repeated runs):
//!
//! | Profile | eval 1 (cold worker) | steady-state per eval | ratio |
//! |---|---|---|---|
//! | dev | ~2.1 ms | ~350 µs | 5.5–6.2x |
//! | release | ~1.3 ms | ~93–100 µs | 13.3–14.5x |
//!
//! The original ≥ 20x / ≤ 25 µs estimate predated the per-eval integrity
//! verification work (~65 µs mandated by the protected-invariants and
//! integrity-set requirements) and was recalibrated by spec amendment
//! 2026-09-12 rather than by weakening the test.
//!
//! # Methodology (2026-11-12 amendment)
//!
//! The ratio is now built from three explicit phases on one `BoaEngine`:
//!
//! 1. **eval 1** — the first job into a freshly constructed engine, wall
//!    clock (cold worker: realm build + wrapper compile).
//! 2. **Warm-up window** — the next 100 evals, untimed.
//! 3. **Steady state** — 6 independent timed batches of 150 cache-hit evals
//!    each, on the same source.
//!
//! The assertion is the **median** per-eval time across the 6 batches under
//! 25% of eval 1 (≥ 4x amortization), computed by the pure helper
//! [`assert_amortization_ratio`]. The median replaces the old single mean
//! because a mean over one long window is dominated by scheduling noise: CI
//! run 43033768484 measured 514.6 µs/eval against ~350 µs on the dev box —
//! ratio 3.9x versus the 4x floor — while the cache itself was healthy.
//! Labeled hypothesis: contention inflates a minority of batches, and the
//! median discards that minority. The trade-off is accepted: sustained
//! contention across the whole window (most batches slow) still trips the
//! floor, which is the false-positive direction we prefer over masking a
//! real regression.
//!
//! ### Measured evidence (2026-11-12)
//!
//! 20 consecutive `--exact --nocapture` runs per condition (unloaded; loaded:
//! one spinner per CPU) of `amortization_ratio_at_least_4x` in the feature
//! worktree (`target/flakytests-evidence/*.log`, not committed). The two
//! conditions are separate sequences, with spinner setup and settle between
//! them. `b1`–`b6` are the six timed batch values in µs/eval.
//!
//! **Unloaded (no synthetic load)**
//!
//! | Run | eval1 µs | b1 | b2 | b3 | b4 | b5 | b6 | median µs | ratio |
//! |---|---|---|---|---|---|---|---|---|---|
//! | 1 | 2018.2 | 365.9 | 362.0 | 338.5 | 349.0 | 346.6 | 351.0 | 350.0 | 5.8x |
//! | 2 | 2032.1 | 364.8 | 356.7 | 337.6 | 346.6 | 352.5 | 354.0 | 353.3 | 5.8x |
//! | 3 | 1894.1 | 362.6 | 352.0 | 336.2 | 344.4 | 344.4 | 348.5 | 346.4 | 5.5x |
//! | 4 | 1940.3 | 359.6 | 350.1 | 340.3 | 345.7 | 344.1 | 347.8 | 346.7 | 5.6x |
//! | 5 | 1957.6 | 359.6 | 351.3 | 337.2 | 346.6 | 353.3 | 346.9 | 349.1 | 5.6x |
//! | 6 | 2030.4 | 366.0 | 355.7 | 336.9 | 349.3 | 345.9 | 346.9 | 348.1 | 5.8x |
//! | 7 | 1990.2 | 364.4 | 352.7 | 338.7 | 353.4 | 357.1 | 355.2 | 354.3 | 5.6x |
//! | 8 | 1857.6 | 391.5 | 378.9 | 352.1 | 367.3 | 366.9 | 363.4 | 367.1 | 5.1x |
//! | 9 | 2062.4 | 366.4 | 359.1 | 335.5 | 347.7 | 348.6 | 356.9 | 352.8 | 5.8x |
//! | 10 | 1982.4 | 360.3 | 357.7 | 336.2 | 347.2 | 348.5 | 348.1 | 348.3 | 5.7x |
//! | 11 | 2041.2 | 366.4 | 357.7 | 338.5 | 346.8 | 346.0 | 350.7 | 348.7 | 5.9x |
//! | 12 | 2062.2 | 359.2 | 349.9 | 334.9 | 350.0 | 351.0 | 348.1 | 349.9 | 5.9x |
//! | 13 | 2005.3 | 361.7 | 352.7 | 335.6 | 354.2 | 355.4 | 348.3 | 353.5 | 5.7x |
//! | 14 | 1968.5 | 371.8 | 351.2 | 334.4 | 350.4 | 360.4 | 359.3 | 355.3 | 5.5x |
//! | 15 | 1791.5 | 360.8 | 352.4 | 334.6 | 350.0 | 351.6 | 350.5 | 351.0 | 5.1x |
//! | 16 | 2056.3 | 363.0 | 356.3 | 335.6 | 353.1 | 351.4 | 351.4 | 352.2 | 5.8x |
//! | 17 | 1993.5 | 369.0 | 356.9 | 344.8 | 350.6 | 351.2 | 350.9 | 351.0 | 5.7x |
//! | 18 | 2049.7 | 362.6 | 355.6 | 336.1 | 358.0 | 352.9 | 356.5 | 356.1 | 5.8x |
//! | 19 | 2033.2 | 366.0 | 367.4 | 348.5 | 362.7 | 363.4 | 363.0 | 363.2 | 5.6x |
//! | 20 | 2022.1 | 368.8 | 353.6 | 336.5 | 353.3 | 352.7 | 352.1 | 353.0 | 5.7x |
//!
//! Summary: 20/20 passed; ratio min 5.1x / median 5.7x / max 5.9x;
//! per-batch µs/eval range 334.4–391.5; median µs/eval 346.4–367.1; eval1 1791.5–2062.4 µs.
//!
//! **Loaded (one busy-loop spinner per CPU, `nproc=12`)**
//!
//! | Run | eval1 µs | b1 | b2 | b3 | b4 | b5 | b6 | median µs | ratio |
//! |---|---|---|---|---|---|---|---|---|---|
//! | 1 | 5909.9 | 638.6 | 858.5 | 1716.1 | 836.1 | 626.7 | 770.0 | 803.0 | 7.4x |
//! | 2 | 5907.6 | 1361.9 | 625.1 | 628.3 | 806.8 | 624.0 | 623.0 | 626.7 | 9.4x |
//! | 3 | 5910.7 | 637.2 | 1148.2 | 599.2 | 618.5 | 619.6 | 617.7 | 619.0 | 9.5x |
//! | 4 | 7906.5 | 2020.0 | 959.7 | 603.2 | 619.6 | 653.8 | 623.4 | 638.6 | 12.4x |
//! | 5 | 6901.9 | 1426.0 | 623.8 | 601.0 | 622.3 | 623.7 | 620.2 | 623.0 | 11.1x |
//! | 6 | 6501.8 | 1304.0 | 1111.7 | 619.8 | 671.3 | 627.6 | 635.2 | 653.3 | 10.0x |
//! | 7 | 6905.3 | 1248.5 | 629.5 | 845.3 | 1313.3 | 868.9 | 626.0 | 857.1 | 8.1x |
//! | 8 | 6893.4 | 650.6 | 629.0 | 602.9 | 633.9 | 639.2 | 625.9 | 631.5 | 10.9x |
//! | 9 | 6900.8 | 1477.3 | 1488.0 | 1254.7 | 1470.6 | 760.3 | 670.4 | 1362.6 | 5.1x |
//! | 10 | 5436.4 | 639.7 | 623.7 | 601.3 | 617.8 | 632.9 | 623.3 | 623.5 | 8.7x |
//! | 11 | 5835.7 | 719.6 | 707.1 | 1026.4 | 620.3 | 1095.6 | 620.0 | 713.3 | 8.2x |
//! | 12 | 2903.1 | 640.7 | 949.8 | 603.2 | 1040.6 | 621.6 | 618.9 | 631.2 | 4.6x |
//! | 13 | 2914.3 | 637.1 | 623.7 | 602.9 | 651.4 | 673.5 | 618.8 | 630.4 | 4.6x |
//! | 14 | 6903.2 | 1464.0 | 663.6 | 601.1 | 619.2 | 618.3 | 621.7 | 620.4 | 11.1x |
//! | 15 | 5900.7 | 1216.5 | 628.4 | 603.8 | 621.7 | 624.6 | 620.9 | 623.1 | 9.5x |
//! | 16 | 2914.1 | 719.6 | 631.4 | 605.5 | 621.5 | 633.8 | 624.6 | 628.0 | 4.6x |
//! | 17 | 2909.9 | 639.1 | 634.9 | 602.8 | 606.5 | 623.3 | 623.0 | 623.2 | 4.7x |
//! | 18 | 6869.2 | 698.9 | 650.0 | 811.7 | 625.8 | 658.3 | 630.3 | 654.2 | 10.5x |
//! | 19 | 5909.6 | 1923.1 | 627.5 | 602.1 | 619.0 | 645.8 | 651.4 | 636.7 | 9.3x |
//! | 20 | 2902.7 | 639.7 | 942.7 | 702.9 | 832.8 | 617.5 | 621.5 | 671.3 | 4.3x |
//!
//! Summary: 20/20 passed; ratio min 4.3x / median 9.0x / max 12.4x;
//! per-batch µs/eval range 599.2–2020.0; median µs/eval 619.0–1362.6; eval1 2902.7–7906.5 µs.

use std::time::Instant;

use camel_language_js::{BoaEngine, JsEngine, JsExchange};
use serde_json::json;

/// Representative transform: increments header `n` (parseInt with `||0` guard)
/// and returns the body. Exercises the `camel` global, header get/set, and body
/// access — the shape of a real route step.
const SRC: &str = "camel.headers.set('n', (parseInt(camel.headers.get('n'))||0)+1); camel.body";

/// Small exchange: one header `n` = `"0"`, body a JSON string.
fn make_exchange() -> JsExchange {
    JsExchange::from_headers_body_properties(
        [("n".to_string(), json!("0"))].into_iter().collect(),
        json!("hello"),
        Default::default(),
    )
}

/// Median of a batch of per-eval times (µs). Sorts a copy; an even count takes
/// the mean of the two middle elements — the steady-state batches are 6, so
/// `sorted[n / 2 - 1]` and `sorted[n / 2]`.
fn batch_median(batches: &[f64]) -> f64 {
    let mut sorted = batches.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    if n.is_multiple_of(2) {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    } else {
        sorted[n / 2]
    }
}

/// Asserts the median steady-state per-eval time is under 25% of eval 1
/// (≥ 4x amortization) and returns the measured ratio `eval1_us / median`.
fn assert_amortization_ratio(eval1_us: f64, batches: &[f64]) -> f64 {
    let median = batch_median(batches);
    let ratio = eval1_us / median;
    assert!(
        median < eval1_us / 4.0,
        "median steady-state {median:.2} µs/eval must be under 25% of eval 1 \
         ({eval1_us:.2} µs) — the 4x amortization floor; measured ratio \
         {ratio:.2}x. batches (µs/eval): {batches:?}"
    );
    ratio
}

#[test]
fn amortization_ratio_at_least_4x() {
    // One cold BoaEngine: the first eval pays thread spawn + realm build +
    // baseline capture + wrapper compile.
    let engine = BoaEngine::default();

    // Eval 1 wall-clock (cold worker).
    let t0 = Instant::now();
    let r1 = engine.eval(SRC, make_exchange()).unwrap();
    let eval1 = t0.elapsed();
    // Sanity: the representative transform actually ran (timing is the assert,
    // but a silently-broken eval path must not produce a misleading ratio).
    // The completion value is `camel.body` — asserting it proves the JS
    // evaluated (the body getter ran), not just that the exchange passed
    // through untouched.
    assert_eq!(r1.return_value.as_str().unwrap(), "hello");
    assert_eq!(r1.body.as_str().unwrap(), "hello");

    // Explicit untimed warm-up window: 100 evals absorb any residual one-off
    // costs (source-site registration, allocator growth) before timing.
    for _ in 0..100 {
        engine.eval(SRC, make_exchange()).unwrap();
    }

    // Steady state: 6 independent timed batches of 150 cache-hit evals on the
    // same source. A fresh JsExchange per eval keeps results comparable (each
    // eval sees `n` = "0", as in real per-message usage); the assert is on
    // time only.
    const BATCHES: usize = 6;
    const EVALS_PER_BATCH: u32 = 150;
    let mut batches = Vec::with_capacity(BATCHES);
    for _ in 0..BATCHES {
        let t = Instant::now();
        for _ in 0..EVALS_PER_BATCH {
            engine.eval(SRC, make_exchange()).unwrap();
        }
        batches.push(t.elapsed().as_secs_f64() * 1e6 / f64::from(EVALS_PER_BATCH));
    }

    let eval1_us = eval1.as_secs_f64() * 1e6;
    let ratio = assert_amortization_ratio(eval1_us, &batches);
    let median = batch_median(&batches);

    for (i, batch_us) in batches.iter().enumerate() {
        println!("batch {} = {batch_us:.1} µs/eval", i + 1);
    }
    println!("eval1 = {eval1_us:.1} µs, median = {median:.1} µs/eval, ratio = {ratio:.1}x");
}

#[test]
fn assertion_fails_on_regressed_steady_state() {
    // A regressed (uncached) steady state sits at eval 1's own cost: the floor
    // must reject it and the panic must name the 4x contract.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_amortization_ratio(2000.0, &[1900.0, 1950.0, 2000.0, 2050.0, 2100.0, 2150.0])
    }));

    let payload = outcome.expect_err("regressed steady state must trip the 4x floor");
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .expect("panic payload must be a string");
    assert!(
        message.contains("4x"),
        "panic message must name the 4x floor; got: {message}"
    );

    // Strict boundary: median (500.0) == eval 1 / 4 (500.0) exactly must fail —
    // the floor is `<`, so equality is not amortized enough.
    let boundary = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_amortization_ratio(2000.0, &[500.0, 500.0, 500.0, 500.0, 500.0, 500.0])
    }));
    let boundary_payload = boundary.expect_err("median at exactly eval1/4 must trip the 4x floor");
    let boundary_message = boundary_payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| boundary_payload.downcast_ref::<&str>().copied())
        .expect("panic payload must be a string");
    assert!(
        boundary_message.contains("4x"),
        "boundary panic message must name the 4x floor; got: {boundary_message}"
    );

    // Minority-outlier tolerance: one inflated batch among six must not fail —
    // median = 350.0 < 500.0, so the helper passes and reports ~5.7x.
    let ratio = assert_amortization_ratio(2000.0, &[350.0, 350.0, 350.0, 350.0, 350.0, 5000.0]);
    assert!(
        ratio > 4.0,
        "one inflated batch must not drag the ratio under 4x; got {ratio:.2}x"
    );
}
