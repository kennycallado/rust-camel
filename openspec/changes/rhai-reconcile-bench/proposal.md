# Proposal: rhai-reconcile-bench

## Why

`camel-cache` measured that 0.66.0 does not improve their reconcile loop, and gave us
the workload and the numbers (5001 members, same machine/file/tree):

| tag       | wall clock        |
| --------- | ----------------- |
| v0.56.0   | 8.3 s             |
| v0.57.0   | 29.2 s            |
| v0.61.0   | > 30 s (`direct:` cut) |
| v0.66.0   | > 30 s (`direct:` cut) |

We closed bd rc-m01r9 on a ratio proxy benchmark; this change makes the workload shape
the customer described reproducible in-repo so the next fix decision is measured, not
argued. **This change reproduces; it does not fix.** The loop is a DSL `loop:`/`while:` with one rhai script
per iteration (~10 phases × ⌈N/128⌉ chunks ≈ 400 iterations at N=5001); each iteration
reads and writes ~11 large exchange properties, so every evaluation converts all of
them — the shape bd rc-m01r9 blames for the 2.7–3.5× regression.

## What Changes

Additive test material only, under `crates/camel-integration-test/`:

- Fixtures under `tests/fixtures/reconcile/`: `reconcile.yaml` (inner reconcile loop
  route), `reconcile-budget-fixture.yaml` (generator → reconcile → oracle route with
  per-phase log markers), `stable-singletons-1200.test.yaml` (CI control) and
  `stable-singletons-5001.test.yaml` (bench document). Synthetic data only
  (`viirs:<n>`, `graph:<n>`, `historic:<n>`); relative `routeFiles` resolve from the
  fixtures directory; the inner `direct:` call carries a raised `timeout_ms` so the
  30 s default cut cannot masquerade as a result.
- `tests/reconcile_budget_test.rs`:
  - CI test: drives the **1200 control** through `run_scenario_document` (the
    `wasm_boot_test.rs` pattern) and asserts the reply shape
    `{ members: 1200, clusters: 1200, retiredIds: 0, errors: 0 }`; stays well inside
    the job budget.
  - `#[ignore]` bench: the **5001 document** in release, no wall-clock assertion;
    prints total elapsed, the phase breakdown (generator / reconcile / oracle from
    log-capture `Instant` timestamps), and the reply. Comment names the exact command
    and the historical-run recipe (one worktree per tag, fixture copied unchanged).
- One feature line in `crates/camel-integration-test/Cargo.toml`:
  `lang-rhai = ["camel-core/lang-rhai"]` — the crate's established shadow-feature
  pattern (`wasm`, `sql`, `surreal`, `redis`); without it rhai is not on the
  integration-test boot path and no scenario can evaluate a rhai step. The test
  file is gated `#![cfg(feature = "lang-rhai")]` so default-feature builds compile
  it out rather than fail.
- No `crates/**/src/**`, no `bridges/**`, no dependency version changes.

**Deviation to disclose (conductor gate):** the customer's verbatim files are not
present in this environment — `camel-cache` is the customer's private repo (its
`src/nodes/incidents/resolve/reconcile.yaml` is referenced in bd rc-m01r9 but not
reachable), and no copy exists on disk or in git history. The fixtures are faithful
reconstructions of the workload the customer described (loop structure, 128-key
chunking, ~10 phases, 11 large properties read+written per iteration,
oracle-maps-as-text, synthetic ids), not byte-for-byte copies. Landing therefore
requires the master conductor, at the merge gate, to either (a) supply the verbatim
originals as a drop-in fixture replacement, or (b) accept this reconstruction as an
amended deliverable. The park is that request; until the conductor rules, this change
must not merge.

## Acceptance criteria

- `cargo test -p camel-integration-test --features lang-rhai --test
  reconcile_budget_test` green (1200 control); its **measured debug wall clock** is
  recorded in the park. If that clock exceeds the CI job budget, the park is filed
  `blocked: budget` — no workload rewrites, no timing assertions added to make it
  pass. (The feature flag is mandatory: a default-feature run compiles the gated
  test out and would be a vacuous green.)
- **Historical comparison is executed, not just prescribed**: the 5001 bench runs on
  current `main` (release) and on `v0.56.0` (one worktree per tag, the fixture copied
  unchanged, release build), and both measurements — total elapsed plus phase
  breakdown — are reported in the park. The recipe comment names all four comparison
  tags (`v0.56.0`, `v0.57.0`, `v0.61.0`, `v0.66.0`) so the remaining points are one
  worktree each; running all four is optional, time permitting.
  **Outcome (2026-11-09): main measured in full; the v0.56 point landed
  `blocked`** — the v0.56 language boundary cannot hold 5001-entry maps across
  script evaluations (textified properties; era `to_json` is map-only and
  size-capped ≲1000 entries; probe logs in the park), so no shape-preserving
  adaptation of the committed fixture can execute there. Per the plan's own
  acceptance, the comparison is labeled `blocked`, not verified; unblocking
  requires the customer's original fixture files (conductor-gate hold 1).
- The 5001 bench runs behind `#[ignore]` only; no wall-clock assertion anywhere.
- Per-phase elapsed printed in the bench (generator / reconcile / oracle); rhai
  test-only counters are `#[cfg(test)] pub(crate)` — unreachable from an integration
  test, so the park says so plainly and the bench prints per-phase elapsed instead.
- `bash scripts/gates-local.sh` exit 0; diff = fixtures dir + one test file + the
  one-line `Cargo.toml` feature (anything else fails).

## Risk budget

- Acceptable: fixture scripts look "unpolished" (they mirror the described workload;
  readability is not the value, fidelity is). Debug-profile control runtime may
  exceed the release numbers by a large factor — the control must stay green with its
  clock recorded; a clock outside the job budget blocks, it does not bend the AC.
- Out of bounds: optimizing the rhai binding, the fixture scripts, or the loops;
  adding the 5001 document to the default test set; any product behavior change;
  raising rhai's `max_operations` limit in the fixture (the 100k default is part of
  the customer environment; the generator is split across script steps instead).
  A clear defect found by the reproduction is described with numbers in the park;
  the fix is a separate change.

Affected crates: `camel-integration-test` (tests + fixtures + one feature line
only). Bd: rc-m01r9.

## Conductor-gate deviations (landing requires a ruling on each)

1. **Fixture reconstruction, not verbatim copies** (see disclosure above).
2. **`lang-rhai` shadow feature** — one feature line in
   `crates/camel-integration-test/Cargo.toml`; the mission's "no gate changes" hard
   limit plausibly forbids it, but without it no integration test can boot rhai.
   CI wiring (`integration-rhai.yml`) is deliberately NOT in this change; the park
   proposes it as a follow-up with the measured debug clock attached.
3. **Post-bless conductor-ruled fixture deltas** (execution findings, each with
   probe evidence; blessed-plan intent preserved):
   - **Generator cap-gap splits**: the 100k ops default tripped at N=5001
     (measured: five scripts + the oracle exceeded). Generator scripts split into
     g1–g12 under the blessed remedy (halve the range, first-half-only map init,
     payload shape preserved).
   - **Oracle two-step split**: the oracle script itself needed 150k ops — the
     blessed remedy text scoped splits to generator scripts; the conductor
     extended the same shape-preserving remedy to the oracle (oc* counts computed
     in oracle-1, reply assembled in oracle-2).
   - **Body coercion shim + quoted send bodies**: v0.56 probe evidence — a
     numeric send body reaches the v0.56 script scope as an empty string, and
     `type_of` returns `"string"` (the `&str` imprint) — so both documents send
     quoted bodies (`"1200"`/`"5001"`) and every generator script coerces via
     `type_of(body) == "String" || type_of(body) == "string"` → `parse_int`.
     Main-side semantics unchanged (the else-branch passes the INT through).
   - **`sendDeadline: 600s` on both documents**: the binding 30 s cut is the
     scenario runner's `SEND_DEADLINE` (`runner.rs:122-131`), not the endpoint's
     `timeout_ms`; without the override a >30 s synchronous send is killed by the
     harness before the endpoint timeout could matter — the same raise-the-cut
     intent the plan prescribed via `timeout_ms`.
   - **`#[ignore]` reason vocabulary**: lint-ignore (ADR-0054) rejected the first
     wording; the reason now uses the `slow test:` prefix (recipe text unchanged).
