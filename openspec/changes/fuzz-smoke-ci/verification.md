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

## Evidence slot 1 — introducing PR (PENDING)

Status: PENDING

The three drills (tmin, cold-main, refusal) close when the pushed PR's
`fuzz-smoke` run is green. Checklist to observe in the Actions log:

- Each drill's PASS entry in the job log: `PASS tmin-drill`,
  `PASS cold-baseline`, `PASS refusal-drill`, plus `PASS smoke`,
  `PASS fuzz-lock-audit`, `PASS cold-main-final`.
- The summary step's all-clear text: no `FAIL` entries, no
  "infrastructure failure" line.
- Job duration from the Actions log, for the < 6 min steady-state
  criterion. The FIRST run is zero-cache: expect 11-14 min. The
  criterion applies to steady-state runs only.

## Evidence slot 2 — post-merge 300 s dispatch (PENDING)

Status: PENDING

One-time `workflow_dispatch` on `main` with `time=300` after merge.

- Dispatch ref: `main`
- Expected exit: 0
- Run URL:

## Evidence slot 3 — first real crash promotion (PENDING by design)

Status: PENDING by design

Promotion closes on the first real crash triage per decision §6.3:
minimized input committed as a `#[test]` in `crates/camel-dsl`. The
synthetic drill panic is the bug, not a promotable input — the superseded
wording per design.md D1.

## rc-7rw2 closure checklist

Ordered:

1. PR green (evidence slot 1).
2. Cost measured < 6 min steady-state.
3. 300 s dispatch green (evidence slot 2).
4. `bd close rc-7rw2`.