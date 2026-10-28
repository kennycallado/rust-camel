# Design: rhai-engine-reuse

## Approach

Binding ruling (e_gpt 2026-10-28, bd rc-rn6v1): try the low-risk fix first;
stop when the gate is met. This change implements ONLY step 1. Wording is
planned intent, not a claim of implementation.

**Construction vs installation.** rhai splits fixed engine setup into
one-time construction (building a module: registering functions, hashing,
`build_index()`) and per-engine installation (attaching a prebuilt
`Shared<Module>` via `register_global_module`, or attaching per-evaluation
closures). Step 1 hoists construction only; installation stays per evaluation
in the current order. Captured-map readers are per-evaluation state by design
and are NOT hoisted.

**Baseline first.** Task order puts `benches/eval_overhead.rs` (new,
non-ignored) on the UNMODIFIED tree; its run output is the recorded baseline
(full-path median + p95 per workload) with machine provenance
(host, CPU model, date, bench parameters) committed as
`benches/eval_overhead_baseline.txt` next to the bench. All relative
comparisons use this file.

**Step 1 — prebuild the fixed, stateless registrations (planned).**

- `create_base_engine` (lib.rs:477) today runs
  `StandardPackage::new().register_into_engine(&mut engine)`. In rhai 1.26
  `register_into_engine` is exactly
  `init_engine(engine); engine.register_global_module(self.as_shared_module())`,
  and for package structs `init_engine` is a no-op; all construction cost is
  `StandardPackage::new()` (module init over ~10 sub-packages + `build_index`).
- Replace with `static STANDARD_PACKAGE: OnceLock<Shared<Module>>` built once;
  per evaluation `engine.register_global_module(clone)`. A `OnceLock` is not
  thread-local state; the module is never mutated after `build_index()` and
  rhai treats registered global modules as read-only, so sharing across
  engines/threads is safe (the JSON host module already does this —
  `json::host::HOST`).
- Stream guards (`stream_body::register_stream_guards`) and the pure-`trim`
  shadow (`string_shadow::register`) are stateless closures over their own
  marker types; their registrations are likewise prebuilt once into one
  `Shared<Module>` (guards first, then shadow, in one module built in that
  order) and installed per evaluation. This removes the remaining
  construction work; it does not merge into the standard package module, so
  module-level precedence is unchanged.
- Registration ORDER is untouched: standard package (shared), JSON host module
  (already cached in `json/host.rs`), limits setters, `disable_symbol`, the
  stream+shadow shared module. Function-lookup precedence therefore stays
  identical (host JSON shadows stock; pure `trim` shadows the in-place one;
  guard refusals behave exactly as today).
- `create_eval_engine`/`register_snapshot_readers` keep building per-evaluation
  closures over the pre-run maps — the captured-map readers are NOT shared.
- No `Arc<Engine>`, no engine mutex, no limit mutation, no thread-local.

**Step 2 — owned immutable engine (DESIGNED SKETCH ONLY, out of scope here).**
RhaiLanguage holds one immutable `Arc<Engine>` per fixed `RhaiLimitsConfig`
(config change = new instance; compiled expressions retain the engine `Arc`
and their `Arc<AST>`). The per-eval snapshot installs inside `spawn_blocking`
via a private, non-Send, bounded, nesting-aware RAII guard that RESTORES the
previous frame on exit; readers validate engine identity and fail closed
without a frame; no `.await`/yield inside the boundary; timeout does not
cancel the blocking worker. This sketch SUPERSEDES this change's
per-evaluation-engine and captured-closure requirements and therefore cannot
land inside this change: if the step-1 gate is missed, this change parks its
result and a follow-up change (spec revision + fresh bless) implements it.

**Bench.** `benches/eval_overhead.rs` (hand-rolled like the existing
`property_scaling.rs`, no criterion dep). Sample = one COMPLETE public
evaluation call — engine build + exchange conversion + reader setup + AST
execution + result conversion (+ mutating write-back) — measured end-to-end;
phase breakdowns (construction / conversion / execution) are measured
separately and reported but never summed into the gate numbers. Workload
matrix (the regression set): trivial, predicate, 20 headers + 20 properties,
50 headers + 50 properties, mutating, 10-clause choice. Prints median + p95
per workload and per phase as machine-readable `overhead:` lines.

Gate contract (explicit thresholds, no silent passes):
- Gate A (absolute, always enforced): trivial full-path median <=
  `RHAI_EVAL_OVERHEAD_MAX_TRIVIAL_US` (default 100); choice full-path median
  <= `RHAI_EVAL_OVERHEAD_MAX_CHOICE_US` (default 250). Either breach fails.
- Gate B (relative, provenance-guarded): every workload p95 <= 110% of the
  committed baseline — checked only when `RHAI_EVAL_OVERHEAD_BASELINE` names a
  baseline file whose recorded CPU model matches the current machine;
  otherwise the bench prints `overhead: p95-check SKIPPED (<reason>)` and
  exits 0 on Gate B while Gate A still applies.
- ACCEPTANCE AUTHORITY: the binding p95 gate is satisfied ONLY by matched
  pre-change/post-change bench runs on the same machine (the committed
  baseline + the post-change run, both recorded in the mission park report).
  A CI run on a foreign machine that skips Gate B CANNOT satisfy the p95
  acceptance criterion — it is a smoke guard, not acceptance evidence.
- CI (`bench-smoke` step, no expected-fail flag) sets explicit runner-class
  values for the absolute gates and enables the baseline only if
  provenance matches (otherwise Gate A alone runs); exit 1 with the offending
  numbers printed.
- property_scaling coverage: the existing `property_scaling.rs` wide/deep
  read/mutating shapes (K=100/1000/5000) are NOT folded into the eval_overhead
  matrix (scope exception). `property_scaling.rs` is extended to print p95
  alongside its existing batch medians/ratios, and the acceptance evidence is
  a matched pre-change/post-change `property_scaling` run on the baseline
  machine with every shape/mode p95 <= 110% of its pre-change value, recorded
  in the mission park report; its own guard semantics and CI wiring continue
  unchanged.

**Security/semantics tests** (prove reuse leaks no per-eval state): a sentinel
value stored by a panicking script, a `?`/early-return script, a host-error
script, and a nested evaluation must all vanish for the next evaluation
(sentinel exchange); concurrent mixed-limit evaluations stay independent; a
timed-out evaluation does not corrupt the next one; stream refusal counters
start at zero per eval; the mutating write-back transaction commits/rolls back
as before.

## Affected crates

- `camel-language-rhai`: `create_base_engine` prebuild + bench + tests +
  Cargo bench registration. No public API change.
- `.github/workflows/ci.yml`: one bench-smoke step. No other crate changes.

## Architecture boundaries

Languages crate only. The Rhai boundary (no Rhai type in public signatures,
camel-language-api `RhaiLimitsConfig` in, SPI trait objects out) is unchanged;
ADR-0032 trust model (scripts trusted, exchange data untrusted) and the
ADR-0033 sandbox posture (`new_raw` + `StandardPackage`, `no_module`,
disabled symbols) are preserved line-for-line.

## Phases

Single-phase change: bench baseline -> prebuild -> tests -> CI wiring land as
one reviewed slice on one branch.
