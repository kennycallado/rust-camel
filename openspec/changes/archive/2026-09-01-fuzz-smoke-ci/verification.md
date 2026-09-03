# Verification: fuzz-smoke-ci

Task 3.1 record. Every gate below ran in this worktree
(`feature/fuzz-smoke-ci`) at the time of writing. Exit codes are the real
codes observed; nothing is fabricated.

## Local verification (executed)

| Gate | Exit code | Note |
| --- | --- | --- |
| `cargo test -p xtask` | 0 | 195 passed, 0 failed |
| `cargo clippy -p xtask -- -D warnings` | 0 | |
| `cargo fmt --check --all` | 0 | |
| `cargo run --package xtask -- lint-unwrap` | 0 | `lint-unwrap: OK (no violations)` |
| Task 2 test: `workflow_yaml_parses` | 0 | ran under pinned pyyaml venv (see below) |
| Task 2 test: `trigger_semantics_exact` | 0 | ran under pinned pyyaml venv (see below) |
| Task 2 test: `workflow_structural_assertions` | 0 | `STRUCTURAL-OK` |
| Task 2 test: `no_floating_or_short_action_pins` | 0 | |
| Task 2 test: `actionlint_if_present` | skipped | `actionlint` binary absent from PATH |

The two pyyaml-dependent tests (yaml parse, trigger check) executed with
the pinned venv first on PATH:
`PATH=/tmp/nix-shell.LX3hPs/opencode/pyyaml-venv/bin:$PATH` (pyyaml
6.0.3). System python3 is a pip-less Nix build; nothing was installed
into the repo.

Local gates do NOT include the runtime drills (refusal, smoke, tmin)
plus the fuzz-lock audit — CI-owned, they run only in the GitHub
Actions job. Of Phase 1's four deferred criteria, three close via the
PR drills and the 300 s full run closes via Evidence slot 2. This
deferral discipline is unchanged from Phase 1.

## Evidence slot 1 — introducing run (CLOSED)

Status: CLOSED 2026-09-02

No introducing PR: the spec forbids a push trigger and this repo
lands via local squash-merge, so slot 1 closed on the first
post-merge `workflow_dispatch` (default time=60).

- Run: https://github.com/kennycallado/rust-camel/actions/runs/33604997832
- All six ledger entries PASS (tmin drill passed after fix 6ec6d25b,
  the already-minimal empty-input fallback; first dispatch
  33525182783 had caught the defect — the drill's first real catch).
- Zero-cache duration ~15 min, within the first-run expectation.

## Evidence slot 2 — post-merge 300 s dispatch (CLOSED)

Status: CLOSED 2026-09-03

One-time `workflow_dispatch` on `main` with `time=300` after merge.

- Dispatch ref: `main`
- Expected exit: 0
- Run URL: https://github.com/kennycallado/rust-camel/actions/runs/33721725823
- Result: green, 17m21s (zero-cache path), all entries PASS.

## Evidence slot 3 — first real crash promotion (PENDING by design)

Status: PENDING by design

Promotion closes on the first real crash triage per decision §6.3:
minimized input committed as a `#[test]` in `crates/camel-dsl`. The
synthetic drill panic is the bug, not a promotable input — the superseded
wording per design.md D1.

## rc-7rw2 closure checklist

Ordered:

1. DONE — slot 1 via post-merge dispatch (no PR; see slot 1 note).
2. DONE, criterion amended — steady-state measured 7m05–7m08s
   (runs 33732031627, 33732825433, both green, stable across two
   samples) after the warm-path round (dd00826b: pinned nightly
   2026-08-27, scoped weekly-rotated xtask/ASAN caches, isolated
   cargo-fuzz root). Original < 6 min was a design estimate; amended
   to the measured reliable number per e_opus ruling (down from
   15m21s pre-round; main-CI caches verified intact, repo total
   ~3.6 GB of 10 GB).
3. DONE — slot 2 (33721725823, 300 s, green).
4. DONE — bd close rc-7rw2 (2026-09-03), with rc-md2b (cache poison)
   and rc-7j5e (tmin already-minimal fallback) closed as subsumed;
   rc-0wb9 (birth-time re-crash detection) and rc-p957 (dated nightly
   lockstep) remain open follow-ups.