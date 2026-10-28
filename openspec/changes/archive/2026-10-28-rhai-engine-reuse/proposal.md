# Proposal: rhai-engine-reuse

## Why

Every rhai `script:`/predicate evaluation builds a fresh Rhai engine
(`create_base_engine`, `lib.rs:477-535`). External profile (camel-112, 0.61.1)
and the local probe (reference/rhai-probe-2026-10-27.rs) show a trivial
evaluation costs ~231 us vs ~8 us for an equivalent Simple choice, and
~153 us (66%) of that is `Engine::new_raw()` + `StandardPackage` construction
and registration. At 9 workers this forced an idle-CPU move to Simple
(0.40 -> 0.03 cores) instead of rhai. The AST is already cached; the engine
build is the remaining fixed cost.

## What Changes

Step 1 only (binding e_gpt ruling 2026-10-28, bd rc-rn6v1) — planned intent:

- Hoist the CONSTRUCTION of the fixed, stateless engine registrations out of
  the per-evaluation path: the `StandardPackage` merged module, and the
  stream-guard + pure-`trim` shadow closures, are prebuilt once per process
  into shared modules (`OnceLock<Shared<Module>>`, no thread-local state).
  Per evaluation the prebuilt shared modules are INSTALLED via
  `register_global_module` — the exact operation
  `Package::register_into_engine` performs after construction — in the
  current registration order. The JSON host module is already process-cached
  (`json::host::HOST`).
- Captured-map snapshot readers (`header()`/`property()`) remain per-evaluation
  closures over the pre-run maps; they are per-engine state by design and are
  not hoisted.
- Add a non-ignored overhead bench (`benches/eval_overhead.rs`) measuring the
  COMPLETE public evaluation path per workload (engine build + exchange
  conversion + reader setup + AST execution + result conversion + mutating
  write-back where applicable) as one sample, plus separate construction /
  conversion / execution phase breakdowns; prints median + p95. Workload
  matrix: trivial, predicate, 20 headers + 20 properties, 50 headers + 50
  properties, mutating, 10-clause choice. Wired into the `bench-smoke` CI job
  with explicit thresholds.
- Add security/semantics tests: deliberate panic, `?`/early return, host error,
  nested evaluation leave no snapshot residue (sentinel exchange); concurrent
  mixed-limit evaluations; timeout does not corrupt the next eval; stream
  refusal counters fresh per eval; transactional mutating write-back unchanged.

Step 2 (one immutable `Arc<Engine>` per fixed `RhaiLimitsConfig` with a
snapshot RAII frame inside `spawn_blocking`) is NOT part of this change and is
INCOMPATIBLE with this change's per-evaluation-engine requirement. If the
step-1 gate is missed, this change stops and a SEPARATE change (spec revision
+ fresh bless) implements step 2 per the binding ruling. Choice batching stays
out of scope (rc-1y2qs).

## Acceptance criteria

- Baseline medians/p95 recorded from `benches/eval_overhead.rs` on the
  UNMODIFIED tree, with machine provenance (host, CPU model, date) committed
  next to the bench, BEFORE any engine change.
- After step 1, on the same machine as the baseline: trivial full-path median
  <= 100 us (stretch 25 us); 10-clause choice <= 250 us; every workload in the
  matrix above p95 <= 110% of its pre-change baseline.
- The bench always prints all medians/p95; it hard-fails on the absolute
  thresholds (defaults 100 us trivial median, 250 us choice median); the
  relative p95 check runs only against a baseline whose recorded CPU model
  matches the current machine, else reports SKIP with the reason (never a
  silent pass).
- New bench is non-ignored, runs in `bench-smoke` with explicit CI thresholds,
  and fails CI when they are exceeded.
- All security/semantics tests pass; no semantic change to evaluation results,
  error classes, sandbox posture, or the mutation transaction.
- `cargo test -p camel-language-rhai -p camel-core`, fmt, clippy
  (`-p camel-language-rhai -p camel-core --all-targets -- -D warnings`),
  `lint-unwrap`, `lint-test-sleep`, `lint-unbounded-wait`, rustdoc
  `-p camel-language-rhai`, `openspec validate --all --strict` all green.

## Risk budget

- Acceptable: process-wide immutable shared modules (registration is
  read-only after `build_index()`); bench noise on foreign machines (relative
  check skips with reason when provenance does not match).
- Out of bounds: semantic changes, new language features, thread-local state,
  hoisting per-evaluation captured-map readers, limit mutation, engine mutex,
  process-wide mutable registry, workspace/bridge bumps, `#[deprecated]`.
