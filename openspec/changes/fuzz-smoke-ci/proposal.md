# Proposal: fuzz-smoke-ci

## Why

Phase 1 (`fuzzing-mutation-tooling`, merged 2026-09-01) landed the
`fuzz/` crate, the `dsl_yaml` target, the committed seed corpus, and the
`cargo xtask fuzz` wrapper — but nothing executes fuzzing anywhere. The
Phase 1 `verification.md` records four runtime drills as
`integration-verification-deferred-to-CI`, explicitly stating they close
"when the Phase 2 CI smoke runs them". The security-audit gap R1 (zero
fuzzing) stays open until a fuzz target actually runs on infrastructure
that has nightly + cargo-fuzz.

This change adds that infrastructure: the path-filtered, non-blocking
`fuzz-smoke.yml` per-PR workflow prescribed by the 2026-08-31 adoption
decision §3.4 and §6. The workflow's own first run (on its introducing
PR) executes three of the four deferred drills — the tmin drill, the
cold-main assertion, and the end-to-end main-checkout refusal. The
remaining criterion (the 300 s full run) closes on a one-time
post-merge `workflow_dispatch` at `time=300`; the synthetic-drill
promotion wording is superseded — promotion closes on the first real
crash triage, recorded in this change's verification.md.

## What Changes

- **New** `.github/workflows/fuzz-smoke.yml`:
  - path-filtered `pull_request` trigger (fuzzed crate `crates/camel-dsl/**`,
    `fuzz/**`, wrapper `scripts/xtask/**`, the workflow file itself) plus
    `workflow_dispatch` for manual drill runs
  - job-level `continue-on-error: true` — never blocks a merge (decision §6.1)
  - nightly toolchain + pinned cargo-fuzz install (cached)
  - **drill steps that exercise the deferred Phase 1 criteria** (closure
    follows the three-way evidence split in design.md D1 — PR run,
    post-merge 300 s dispatch, first-real-crash promotion):
    1. main-checkout refusal drill (expect exit 1 + guard message)
    2. worktree-isolated 60 s smoke run of `dsl_yaml` (exit 0, corpus and
       artifacts only under worktree `target-fuzz/`)
    3. cold-main assertion (main checkout `./target` untouched by the run)
    4. tmin drill: temporary panic injection in the worktree harness →
       crash artifact detected, minimized artifact produced, promotion
       instruction printed (verifies the `entries_created_after` /
       `newest_file` detection against real cargo-fuzz output for the
       first time)
  - `cargo audit --file fuzz/Cargo.lock` step (handoff note from the
    Phase 1 holistic review: the 346-pkg fuzz lockfile is outside the
    root-lock audit coverage)
  - job summary: on a real find, triage instructions (`bd create -t bug
    -p 1`); the PR smoke annotates — automated bd filing is nightly-scope
    (decision §6.2), not this change
- **Wrapper fix (only Rust change, `scripts/xtask`)**: the Phase 1
  `minimize()` invokes `cargo fuzz tmin` without `-artifact_prefix`, so
  tmin writes its output to cargo-fuzz's default `fuzz/artifacts/` while
  the wrapper searches `target-fuzz/artifacts/` — the minimized artifact
  can never be detected (caught by this change's spec blessing round).
  Fix: pass the same `-artifact_prefix` to tmin; assert minimized output
  lands under `target-fuzz/artifacts/` and `fuzz/artifacts/` is never
  created.
- **No other Rust changes.** No QUALITY GATES entry — per decision §6.1
  fuzzing never enters the gate list.

## Affected Crates

`scripts/xtask` (tmin artifact-prefix fix + unit test). Downstream
surface consumed as-is: `fuzz/` crate + seeds, wrapper guards.

## bd

- `rc-7rw2` — Phase 2: fuzz-smoke CI workflow (this change)
- `rc-4g9j` — epic (Adopt cargo-fuzz + scoped cargo-mutants)
