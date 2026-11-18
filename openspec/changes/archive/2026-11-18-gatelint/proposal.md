# Proposal: gatelint

## Why

`cargo xtask lint-gate-parity` (added by mission 569, bd rc-2s1vm) compares the
AGENTS.md `## QUALITY GATES` registry commands against their mapped workflow
steps — `clippy` maps to `.github/workflows/ci.yml` (job `quality`), `doc-build`
maps to `.github/workflows/docs.yml` (job `validate`, step `Build Rustdoc`) —
and fails on drift. It is registered in the AGENTS.md registry, so
`scripts/gates-local.sh` runs it on every local sweep — but CI never invokes
it. A workflow edit that drifts from the registry passes CI and only fails
someone's local sweep later. The comment above the ci.yml clippy step
(lines 411-413) tells the reader that `lint-gate-parity` protects them, which
is currently only true locally.

Wiring the lint into CI closes this gap: drift fails CI directly, on the commit
that introduced it. The archived `ci-clippy-parity` change explicitly deferred
this wiring as out of scope ("a `ci.yml` step for the new lint itself"); this
change is that follow-up.

## What Changes

- Add one step named `lint-gate-parity` (exactly matching the registry gate
  name) running `cargo xtask lint-gate-parity` to the `quality` job of
  `.github/workflows/ci.yml`, placed immediately **before** the `clippy` step
  (after `fmt check`), so the parity diagnostic still runs and reports even
  when a later step fails — a step placed after `clippy` would be skipped by
  GitHub's default `success()` gating exactly when clippy drift makes clippy
  fail.
- Update the clippy-step comment so it states the truth: the registry is the
  source of truth, and the `lint-gate-parity` step above runs in CI and fails
  the job on drift.

Included: only the workflow step and its comment. Excluded: the clippy or
doc-build commands themselves, the container jobs, `scripts/gates-local.sh`,
and any registry change (the gate is already registered). One workflow file,
one step, one comment.

## Acceptance criteria

- The `quality` job contains a step named `lint-gate-parity` running
  `cargo xtask lint-gate-parity`, immediately before `clippy` (after
  `fmt check`), with no custom `if:` condition or `continue-on-error`.
- The clippy-step comment names the CI step that enforces parity.
- The committed workflow diff (vs merge-base) adds exactly this one step plus
  the permitted comment edit — nothing else in the file changes.
- The committed `.github/workflows/ci.yml` parses as valid YAML, and parsing
  it asserts `jobs.quality.steps` contains exactly one `lint-gate-parity`
  step running `cargo xtask lint-gate-parity`, positioned between the
  `fmt check` and `clippy` steps, with no `if:` condition and no
  `continue-on-error`.
- `cargo xtask lint-gate-parity` exits 0 on the committed tree.
- Mutation demo: removing `--all-targets` from the workspace clippy invocation
  only (the three single-crate invocations untouched) makes the lint exit
  non-zero reporting a `clippy` mismatch; the tree is then restored
  byte-identically (`git diff` empty, `git status --porcelain` empty) and the
  lint re-run exits 0.
- Workflow-lint check: the repo has no actionlint/yamllint or YAML-schema
  check for workflows (`cargo xtask schema --check` does not validate
  workflow YAML) — its absence is recorded explicitly in the park. The YAML
  parse assertion above substitutes for it, needing no new dependency or CI
  step.
- No new toolchain requirement, no extra setup/cache, no `--locked` change.

## Risk budget

Minimal: one declarative YAML step using the already-installed stable
toolchain, plus a comment edit. The lint itself is unchanged and already
covered by its own xtask tests and the `gate-parity-lint` capability spec.
Out of bounds: changing any gate command, touching container jobs, or editing
`scripts/gates-local.sh`.

Affected crates: none (CI wiring only). Bd: rc-sa7r6 (p2).
