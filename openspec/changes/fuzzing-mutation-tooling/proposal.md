# Proposal: fuzzing-mutation-tooling

## Why

The 2026-08-31 security audit names fuzzing as recommendation R1: the project
has zero fuzz targets, yet its parser surface is exposed to untrusted bytes.
The YAML route parser (`camel-dsl`) is the largest grammar and is
control-plane reachable through remote route pushes.

The adoption decision (e_opus, 2026-08-31,
`docs/audits/2026-08-31-fuzzing-mutation-adoption-decision.md`, local disk)
settled the verdict: adopt cargo-fuzz now, one pilot target `dsl_yaml`,
run through an xtask wrapper that respects the cold-main disk policy.

bd: rc-a456 (epic rc-4g9j).

## What Changes

Included (Phase 1 of the epic only):

- New excluded `fuzz/` crate with one target `fuzz_targets/dsl_yaml.rs`,
  listed in `workspace.exclude` in the root `Cargo.toml`.
- New `cargo xtask fuzz <target> [--time N]` subcommand: worktree guard
  (refuse to run in the main checkout), toolchain guards (cargo-fuzz and
  nightly presence), forced `CARGO_TARGET_DIR=target-fuzz`, corpus under
  `target-fuzz/corpus/<target>`, crash artifacts pinned under
  `target-fuzz/artifacts/<target>/` via `-artifact_prefix`, and crash
  minimization via `cargo +nightly fuzz tmin <target> <artifact>`.
- Committed seed corpus for `dsl_yaml`, derived from existing camel-dsl
  adversarial test inputs plus the audit's alias-bomb and deep-nesting cases.
- `.gitignore` entry for `/target-fuzz/`, which covers build output,
  corpora, and crash artifacts.

Excluded (tracked as separate bd issues and separate changes):

- CI fuzz-smoke workflow (rc-7rw2, blocked-by this change).
- cargo-mutants scoped probe (rc-eba8).
- Fuzz targets 2-6 from the decision doc (dsl_json, env interpolation,
  Simple expression, SSRF, per-component URI).

## Acceptance criteria

Verified locally by this change:

- `cargo xtask fuzz dsl_yaml` in a worktree without cargo-fuzz installed
  exits non-zero with the install hint (toolchain-guard path verified).
- The root `Cargo.lock` gains zero new entries; `cargo metadata` shows no
  `camel-fuzz` workspace member.
- Neither `fuzz` nor `xtask fuzz` appears in the QUALITY GATES block.

Verified where nightly + cargo-fuzz exist; executed by the Phase 2 CI
smoke (bd rc-7rw2), not claimed as passed by this change:

- `cargo xtask fuzz dsl_yaml --time 300` runs clean in a worktree, writing
  only under `target-fuzz/` (mtime check on the shared main `./target`).
- An intentional injected panic in the harness is caught, `tmin`-minimized,
  and lands as a committed Rust regression test in under 10 minutes
  end-to-end.
- The end-to-end main-checkout refusal drill.

This change records the deferral in `verification.md`
(`integration-verification-deferred-to-CI`); those criteria close when the
Phase 2 CI smoke runs them.

## Risk budget

Acceptable: none to production code paths; this change adds tooling only.
Out of bounds: any write into the main `./target`, any production lockfile
churn, any raw crash corpus committed to git, any new blocking gate.
