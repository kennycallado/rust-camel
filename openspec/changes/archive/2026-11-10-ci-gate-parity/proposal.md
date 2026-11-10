# Proposal: ci-gate-parity

## Why

Two guards exist in-tree but never run in CI:

- **rc-o8pxk** — a registry gate CI skips: `.github/workflows/docs.yml`
  "Build Rustdoc" documents seven crates. The `doc-build` registry in
  `AGENTS.md` documents eight (`camel-component-api` was added on
  2026-11-09, mission 554). A rustdoc regression in `camel-component-api`
  fails `scripts/gates-local.sh` locally and passes CI.
- **rc-in54n** — an in-tree test that compiles out:
  `crates/camel-integration-test/tests/reconcile_budget_test.rs` holds a
  1200-member reconcile control behind `#![cfg(feature = "lang-rhai")]`.
  No workflow passes `--features lang-rhai`, so the control compiles out in
  every build that skips the feature — local sweeps and CI alike — and the
  regression reproduction (bd rc-m01r9) has no CI tripwire.

This is one intent: *the guard exists in-tree but CI never runs it*. For
rc-o8pxk the local sweep already enforces the gate — the `AGENTS.md`
registry "cannot drift" claim holds only there — so CI must run the same
command for the claim to hold on main. For rc-in54n nothing runs the
control anywhere until a workflow passes the feature.

## What Changes

- `docs.yml`: "Build Rustdoc" runs the eight registry crates — the exact
  `doc-build` command, `-D warnings` included (already set at workflow env).
- New `.github/workflows/integration-rhai.yml`: runs the reconcile control
  `cargo test -p camel-integration-test --features lang-rhai --test
  reconcile_budget_test` on `workflow_dispatch` plus path-filtered
  `pull_request`/`push`. The `#[ignore]`d 5001 bench is not run.
- Delta specs for two new capabilities (`ci-doc-gate`,
  `ci-rhai-reconcile-control`), retained by the spec-bless ruling
  (`ci-compile-gate` and `gate-forwarding-lint` own neither gate;
  `skip_specs` rejected).

Excluded: no `ci.yml` changes, no Rust or test changes, no `scripts/**`, no
`AGENTS.md` edits, no new secrets, no `--ignored` runs.

## Acceptance criteria

- Both YAML files parse; the CI `run:` blocks equal the verified local commands.
- The eight-crate doc command exits 0 locally with `-D warnings`.
- The rhai control passes locally as written, wall clock recorded.
- `bash scripts/gates-local.sh` exits 0.

## Risk budget

Workflow files only; a bad edit breaks at YAML parse or the first CI run and
is reverted by a one-line change. No runtime code, no crate graph, no data
path is touched. Risk accepted: CI minutes for a path-narrow, cache-warm job.

Bd: rc-o8pxk, rc-in54n
