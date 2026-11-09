# Proposal: gate-report-partial

## Why

`scripts/gates-local.sh` is the merge precondition (AGENTS.md "Local sweep") and its
summary block is copied verbatim into mission parks. With `GATES_ONLY` set, the loop
silently `continue`s past every gate not selected (`gates-local.sh:110-112`), so a
2-gate partial run prints `passed: 2 / failed: 0 / skipped: 0` and exits 0 —
indistinguishable, in a park table or a merge-gate quote, from a full green sweep.
The lie propagates into merge evidence (bd `rc-p8vfu`).

## What Changes

- Not-selected gates are **counted** and reported on a dedicated `not selected:` line
  (partial mode only); they are NOT merged into the existing `skipped:` line —
  "skipped by policy" and "not selected" are different facts.
- A `ran: X of Y` line is added to the summary in BOTH modes (Y = gates parsed from
  the AGENTS.md registry), so every summary states its denominator.
- When `GATES_ONLY` is set, a startup preamble and the summary divider state the run
  is PARTIAL, so a copied summary carries the qualification with it.
- Full-sweep output stays byte-identical to today except the added `ran:` line.
- Exit codes unchanged: a partial run with all selected gates green still exits 0.
- `GATES_SKIP` semantics unchanged.

Explicitly excluded: the registry parser, the gate set, the skip list, any
`cargo xtask` gate, `.github/**`, `AGENTS.md`, `crates/**`. No Rust changes.

## Acceptance criteria

- Full sweep `bash scripts/gates-local.sh`: summary shows the passed/failed/skipped
  counts as on current main (expected `passed: 28, failed: 0, skipped: 4`; actual
  numbers reported) plus `ran: 28 of 32`, exit 0.
- `GATES_ONLY="fmt check"` green run: summary shows `passed: 1`, a distinct
  `not selected:` count naming the unrun gates, `ran: 1 of 32`, PARTIAL divider —
  exit 0.
- `GATES_ONLY="fmt check"` with a whitespace edit that breaks fmt (then reverted):
  exits non-zero and reports `failed: 1` — the failure path survives.
- No test harness exists for this script (`rg -l 'gates-local' scripts/ crates/`
  matches only the script itself); proof is the three runs above. No test
  framework invented.

## Risk budget

- Bash-only change, one file. Worst case is cosmetic output drift, contained by the
  byte-identical full-sweep requirement.
- Out of bounds: changing which gates run, exit-code semantics, gating logic, or
  inventing a test harness.

Affected crates: none (`scripts/gates-local.sh` only).
Reference: bd `rc-p8vfu`.
