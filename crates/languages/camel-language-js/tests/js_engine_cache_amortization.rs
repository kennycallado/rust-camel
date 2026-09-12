//! Amortization timing test for the js-engine-cache worker.
//!
//! The CI assertion is the machine-independent ratio only — mean(evals
//! 2..=1000) must be under 25% of eval 1 (≥ 4x amortization, dev-profile CI
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

    // Evals 2..=1000: cache hits on the same source. A fresh JsExchange per
    // eval keeps results comparable (each eval sees `n` = "0", as in real
    // per-message usage); the assert is on time only.
    let t0 = Instant::now();
    for _ in 2..=1000 {
        engine.eval(SRC, make_exchange()).unwrap();
    }
    let steady = t0.elapsed();

    let eval1_us = eval1.as_secs_f64() * 1e6;
    let steady_us_per_eval = steady.as_secs_f64() * 1e6 / 999.0;
    let ratio = eval1_us / steady_us_per_eval;

    println!(
        "steady_state_us_per_eval = {steady_us_per_eval:.1} µs \
         (eval1 = {eval1_us:.1} µs, ratio = {ratio:.1}x)"
    );

    assert!(
        steady_us_per_eval < eval1_us / 4.0,
        "steady-state {steady_us_per_eval:.1} µs/eval must be under 25% of eval 1 \
         ({eval1_us:.1} µs); measured ratio {ratio:.1}x < 4x"
    );
}
