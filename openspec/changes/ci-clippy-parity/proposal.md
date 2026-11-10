# Proposal: ci-clippy-parity

## Why

Mission 561 (`e42f9add9`, bd rc-e6kz2) widened the workspace clippy line in the
`AGENTS.md ## QUALITY GATES` registry with `--all-targets`, so `#[cfg(test)]`
code and `tests/` targets get linted. `.github/workflows/ci.yml` hard-codes the
previous line without it, so CI still skips test-target lints — the exact hole
561 closed. Local gates pass; CI would not. This is the second registry/CI
drift in two days (559: `doc-build` crate set in `docs.yml`; 561: clippy flags
in `ci.yml`). bd: rc-2s1vm (P2, discovered-from rc-e6kz2).

## What Changes

- `ci.yml` quality job, `clippy` step: mirror the registry clippy command
  exactly — workspace line gains `--exclude security-keycloak`,
  `--exclude security-wasm-policy`, `--all-targets`. The other three
  invocations already match the registry and stay untouched.
- The `clippy` step gains a comment naming `AGENTS.md ## QUALITY GATES` as the
  source of truth (mission 559 pattern in `docs.yml`, bd rc-o8pxk).
- New `cargo xtask lint-gate-parity` (optional task, TAKEN): compares the
  registry `clippy` and `doc-build` commands against their workflow steps via an
  explicit mapping — `clippy` → `ci.yml` / `quality` / `clippy`,
  `doc-build` → `docs.yml` / `validate` / `Build Rustdoc`. Normalization
  preserves statement boundaries (each invocation stays a separate line) and
  `VAR=value` prefixes are compared against the mapped workflow's effective
  environment (step/job/workflow `env:` chain), so `docs.yml`'s workflow-level
  `RUSTDOCFLAGS` satisfies the registry's inline `RUSTDOCFLAGS="-D warnings"`
  prefix. A missing registry entry or missing mapped step is a violation.
  Fails with a paste-ready expected/actual pair. Unit-tested against fixtures. Added to the `AGENTS.md ## QUALITY GATES` registry as a new
  entry so `gates-local.sh` — which parses the registry — runs it. No existing
  registry gate value changes.
- `openspec/specs/ci-doc-gate/spec.md` delta: the "Registry and workflow
  disagree" scenario moves from review-only enforcement to mechanical
  enforcement by the new lint.
- Out of scope: any other workflow behaviour (triggers, timeouts, matrix,
  permissions), a `ci.yml` step for the new lint itself (CI wiring of
  `lint-gate-parity` is a follow-up), and `cargo-audit`/other gates' parity.

## Acceptance criteria

- `ci.yml` clippy step text matches the registry command; the file parses as
  YAML.
- The exact new workspace clippy command exits 0 locally.
- `bash scripts/gates-local.sh` exits 0 and reports the new gate in its count.
- `lint-gate-parity` fails on a deliberately drifted copy and passes on the
  clean tree, printing expected/actual.
- No existing AGENTS.md gate value changed.

## Risk budget

- Low: registry/CI drift is a config sync; the widened clippy line is already
  proven on main (rc-e6kz2 close note: exit 0, one deprecation fix).
- The AGENTS.md registry ADD (one new entry, no value edits) is the one
  judgement call — the hard limit says "do not touch gate values"; adding a
  gate is how gates-local (a registry parser) picks the lint up. Escalated to
  the spec bless for a ruling.
- Parity lint must stay static: file parsing only, no cargo builds, no new
  dependencies.

Affected crates: none in the workspace proper (`scripts/xtask` only, plus
`.github/workflows/ci.yml`, `AGENTS.md` registry add, `scripts/tests/` fixture
if needed).
