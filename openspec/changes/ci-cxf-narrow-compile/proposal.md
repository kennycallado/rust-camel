# Proposal: ci-cxf-narrow-compile

## Why

The published-bridge smoke is the only place that compiles
`camel-test --features integration-tests-cxf`, and it runs after a
release, not on PRs (its own header: "release verification, NOT a PR
gate"). Every PR job compiles a feature union instead: clippy uses
`--all-features` and `ci.yml` uses `--features integration-tests`. The
wide `integration-tests` feature also activates
`dep:camel-component-cxf`, so a mistake confined to the narrow feature
line — a dropped or misconfigured dependency in
`integration-tests-cxf = ["dep:camel-component-cxf"]` — compiles clean
in every PR job and surfaces only in the post-release smoke. That is a
gap between "CI is green" and "the shipped smoke will build"
(bd rc-z1ea2; follow-up to rc-pqj93 / mission 377, which landed the
narrow feature for the smoke itself).

## What Changes

- Add one named, unconditional compile-only step to the existing
  `full-tests-linux` job in `.github/workflows/ci.yml`, adjacent to the
  existing wide compile gate (rc-8g35d/rc-khjnb precedent):

  `cargo test -p camel-test --features integration-tests-cxf --test cxf_test --no-run`

- Extend the `ci-compile-gate` OpenSpec capability with the narrow
  compile requirement.
- Add an evidence file under the change dir: job inventory, measured
  local cost, and a sabotage demonstration (narrow feature dependency
  temporarily gated out → step fails at rustc level; restored → passes).
  Sabotage is never committed.

Excluded (non-goals): no changes to `published-bridge-smoke.yml`, no
feature-set changes in `camel-test`, no Java/bridge changes, no version
bump, no new workflow file, no new CI job.

## Acceptance criteria

- Every PR run of `ci.yml` executes the narrow compile step; it is not
  gated on paths, labels, or `continue-on-error`.
- The step compiles AND links the `cxf_test` binary under the narrow
  feature selection (same command shape the smoke runs); it does not
  execute tests.
- Sabotage demonstration: with `dep:camel-component-cxf` removed from
  the `integration-tests-cxf` feature, the step command fails (rustc
  unresolved-crate error on `camel_component_cxf`); with the real tree
  it exits 0. Both recorded with exit codes in the evidence file.
- Added wall-clock cost stated as measured local elapsed plus the
  mission 377 cold bound (449.262 s) as provenance.

## Risk budget

Acceptable: the narrow step is skipped only when an earlier step in the
same job already failed (PR is red anyway). Out of bounds: widening the
narrow feature, touching the smoke workflow, or making the step
best-effort.

Affected crates: none modified — `camel-test` and `camel-component-cxf`
are referenced but unchanged. bd: rc-z1ea2.
