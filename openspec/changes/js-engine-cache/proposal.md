# Proposal: js-engine-cache

## Why

Every exchange that hits a JS step builds a full Boa runtime: `boa.rs` calls
`Context::default()` per eval. A mission probe (release, worktree) decomposed
the cost: realm/intrinsics construction is ~99% (~390 µs), parse ~3% (~11 µs),
execution ~0.2%. Bench era-2 measured ~1.1 ms of the 1.73 ms t2-json tick for
this — the dominant CLI-vs-lib gap. bd: rc-i5pqu.

The owner signed Option A (fleet order 67, 2026-09-12): reuse one stable realm
on a dedicated worker. The original "JS semantics unchanged" seal is lifted and
replaced by an honest isolation contract; oracle conditions from the first
blessing review are mandatory design content.

## What Changes

Include:

- A dedicated `camel-js-worker` thread inside `BoaEngine` owning a persistent
  `Context` with one stable realm and an LRU cache (cap 256, full-source keys)
  of per-source wrapper scripts that invoke indirect `eval`.
- Protected invariants per eval: install-verify fresh `camel`, `console`, and a
  pristine `eval` before execution; any failure discards the realm.
- Cleanup on every exit path (success, error, panic, limit failure): remove
  configurable global additions added by the eval.
- Integrity-set recycling: after each eval, verify a named root set (own keys,
  symbol keys, descriptors, prototype identity, value identity). On drift,
  recycle the realm — never attempt restoration.
- Jobs carry a deadline; expired queued jobs are skipped, never executed.
- Error mapping (`eval` → Execution, `validate` → Parse) and all DoS limits
  unchanged. Custom `JsEngine` implementations bypass the worker.

Exclude: canonical bench re-run (sealed), multi-worker sharding, per-expression
context pooling (recorded as a future path in the design).

## Acceptance criteria

- Steady-state amortization: mean per-eval cost of evals 2..N is under 25% of
  eval 1 (≥ 4x; CI runs dev profile where the measured ratio is 5.5–6.2x;
  release measures 13.3–14.5x, ~93–100 µs/eval vs the pre-change ~390 µs).
  The original ≥ 20x / ≤ 25 µs estimate predated the per-eval verification
  work mandated by the oracle conditions; recalibrated by amendment, with
  both measured profiles documented in the test.
- Lexical declarations are fresh per eval; `camel`/`console` are fresh per
  eval; configurable global additions do not persist; integrity drift
  (poisoned `eval`, undeletable global, frozen prototype) triggers realm
  recycle and the next eval sees clean state.
- Expired queued jobs never execute.
- First and cached evals return structurally identical results; runtime errors
  repeat identically; error variants and messages keep today's mapping.
- All mission gates green in the worktree, including
  `cargo test -p camel-language-js`.

## Risk budget

Acceptable: shared-engine-state deltas as declared in the honest contract
(intrinsic state outside the integrity set, heap and engine state, RNG and GC
timing, promise-job retention); single-worker serialization with bounded
backpressure; realm recycle cost (~0.4–0.7 ms) paid on drift.

Out of bounds: relaxing any DoS cap; unbounded cache or queue growth; changing
the public `JsEngine` API; silent narrowing — every non-guarantee stays written
in the spec.
