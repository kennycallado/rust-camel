# Proposal: flakytests

## Why

Two wall-clock tests blocked the v0.67.0 release for hours across two
consecutive CI attempts (bd `rc-86u4n`, `rc-af1y3`). Both passed on other
attempts and runners: timing flakes, not product regressions. The release
gate now treats Full Tests as advisory (`release-gate-minimal`), but a
flaky test still hides real failures and burns runner time. Both flakes
are class `platform-timing` in the rc-99d5 taxonomy: OS-dependent timing
windows that fail on loaded runners without any code change.

## What Changes

- **Flake 1, in scope** (`crates/languages/camel-language-js/tests/js_engine_cache_amortization.rs`):
  replace the single mean over evals 2..=1000 with an explicit untimed
  warm-up window plus the median of 6 steady-state batches of 150 evals,
  asserted through a pure helper; a wall-clock-free negative test proves
  the helper still fails a simulated cache regression (synthetic
  regressed batch distribution). The ≥ 4x machine-independent ratio
  intent is preserved exactly. Because the spec scenario pins the
  current methodology (1000 evals, mean), this change carries a
  **narrow spec delta** on `js-engine-cache`
  (measurement methodology only) — the same amendment path the spec
  itself records for the 2026-09-12 recalibration.

- **Flake 2, OUT OF SCOPE — parked with evidence** (`crates/camel-cli/tests/job_signal_test.rs`,
  bd `rc-af1y3`): three expert review rounds converged on a prerequisite
  this mission cannot satisfy test-only: a deterministic second-signal
  test needs an observed production marker proving worker pipeline entry
  (the teardown-holding drain counts claimed exchanges; no production
  log line observes exchange claim or pipeline entry at any level —
  the delayer is silent, seda logs only consumer start/stop). Adding
  that marker is a ~3-line env-opt-in test seam mirroring the existing
  `CAMEL_JOB_SIGNAL_MARKER` / `CAMEL_JOB_SHUTDOWN_MARKER` precedent —
  production code, forbidden by this mission's constraints. The CLI
  test ships unchanged; `rc-af1y3` stays open carrying the analysis.
  Full design record: `design.md`, Flake 2 investigation.

## Acceptance criteria

- JS test passes ≥ 20/20 consecutive local runs unloaded and ≥ 20 runs
  under representative synthetic CPU load; both batch distributions are
  recorded in the test's doc comment; the loaded-run median stays ≥ 4x.
- The wall-clock-free negative test proves the helper fails a simulated
  cache regression.
- `cargo fmt --check`, clippy `-D warnings` on touched crates, and
  `bash scripts/gates-local.sh` all exit 0.
- Spec delta limited to the `js-engine-cache` amortization measurement
  methodology; every other surface is delta-free. The camel-cli test
  files are untouched by this change.

## Risk budget

Acceptable: measurement-methodology changes in the JS test; a bounded
+1-eval runtime increase (1001 vs 1000 evals). Out of bounds: production
code, deleting/skipping tests, retry-until-pass, widened assertions,
CLI test synchronization changes without the production seam.
