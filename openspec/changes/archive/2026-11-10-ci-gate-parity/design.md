# Design: ci-gate-parity

## Approach

### Issue A — rc-o8pxk: docs.yml rustdoc crate set

`AGENTS.md` `## QUALITY GATES` is the gate registry and the source of truth.
The local sweep (`scripts/gates-local.sh`) parses that block, so it cannot
drift from the registry. `docs.yml` cannot share that mechanism cheaply: the
registry is prose in a markdown file, and deriving workflow YAML from it
needs a generator script — the order forbids importing a new tool into CI.
So the registry stays the single source of truth, and `docs.yml` copies the
registry command verbatim with a comment citing `AGENTS.md doc-build` and
bd rc-o8pxk as the drift tripwire. Reviewers comparing the two strings catch
future drift; the delta spec (`ci-doc-gate`) makes the parity requirement
normative, and the local sweep still catches it first.

The CI step becomes exactly the registry command: eight crates, `--no-deps`,
`-D warnings`. `RUSTDOCFLAGS: "-D warnings"` is already set at workflow env
level (docs.yml lines 27–28), which the step inherits — no new env needed.

### Issue B — rc-in54n: integration-rhai.yml

New workflow. Runner, cache, env, disk-space, and timeout conventions
mirror the integration family (redis is the closest sibling: same
`camel-integration-test` target). The trigger set is NOT family-mirrored
— see Trigger choice below:

- Triggers: `workflow_dispatch`, plus `pull_request` and `push` on `main`
  with path filters: `crates/camel-integration-test/**`,
  `crates/camel-core/**`, `crates/languages/camel-language-rhai/**`,
  `crates/camel-api/**`, `crates/camel-language-api/**`, `Cargo.toml`,
  `Cargo.lock`, `.github/workflows/integration-rhai.yml`.
  `camel-api` and `camel-language-api` are the direct workspace
  dependencies of `camel-language-rhai` (its `Cargo.toml` `[dependencies]`);
  `Cargo.toml`/`Cargo.lock` catch a `rhai` workspace-dependency bump that
  touches only the lockfile — every family sibling lists both.
  **Correction to the order:** it lists
  `crates/components/camel-language-rhai/**`, but the crate lives at
  `crates/languages/camel-language-rhai/` (bd rc-m01r9 cites
  `crates/languages/camel-language-rhai/src/lib.rs`; `crates/components/`
  has no such directory). The order's path would never match and the
  workflow would never trigger — the repo layout wins.
- Runner `ubuntu-latest`, `timeout-minutes: 30`, pinned
  checkout/toolchain/rust-cache SHAs copied from the family, disk-space
  step, and the family env block (`CARGO_INCREMENTAL=0`,
  `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0` — ENOSPC
  mitigation). No `libclang` step: redis builds `camel-integration-test`
  without it and the rhai dependency tree is pure Rust.
- One test step: `cargo test -p camel-integration-test --features lang-rhai
  --test reconcile_budget_test`. Debug control only; the 5001 bench stays
  `#[ignore]`d (no `--ignored`).
- A job comment records the observed baseline — control ~6.6–7.4 s on the
  mission machine — as an observation, states timing is machine-dependent,
  and points at bd rc-m01r9 and the release bench recipe.

### Trigger choice (per-push matrix vs nightly)

Not in the `ci.yml` per-push matrix: the hard limit forbids duplicating it,
and an unconditional lang-rhai debug build on every push costs every
unrelated change minutes of build time for a path-narrow control.
Not nightly (the `integration-mail.yml` precedent): mail's nightly is
justified by two container pulls plus a cold build for wire behaviour that
rarely changes; the rhai control is a ~7 s test whose blast radius
(`camel-language-rhai`, `camel-core`, the harness crate) changes often, and
a nightly would leave a 24 h blind window at the same build cost. Path-
filtered PR + push pays only when the guarded crates change; rust-cache
keeps repeat builds warm. `workflow_dispatch` covers manual verification.

`push` on `main` is order-mandated, not family style (no `integration-*.yml`
has a push trigger). Justification: it closes the merge-skew window — a PR
green against base X can still break after main moves (a `Cargo.lock` bump,
a `camel-core` change), and with PR-only triggers the break waits for the
next unrelated PR to that path; push also covers direct main commits.

### Spec canon decision

`openspec/specs/ci-compile-gate/` owns the `camel-test` compile-gate steps
inside `ci.yml`; `gate-forwarding-lint/` owns the xtask lint. Neither owns
`docs.yml` rustdoc coverage or the rhai control. This change therefore
proposes two new capabilities — `ci-doc-gate`, `ci-rhai-reconcile-control` —
as delta specs. The spec bless rules whether they land or the change flips
to `skip_specs: true`.

## Affected crates

None in Rust. `.github/workflows/docs.yml` and
`.github/workflows/integration-rhai.yml` only. The guarded crates
(`camel-component-api`, `camel-language-rhai`, `camel-integration-test`,
`camel-core`) are not modified.

## Architecture boundaries

CI workflow plumbing only; neither the data plane nor the control plane is
touched. No Runtime, DSL, Components, Services, Languages, or Functions code
changes; the workflows run existing in-tree guards in CI.
