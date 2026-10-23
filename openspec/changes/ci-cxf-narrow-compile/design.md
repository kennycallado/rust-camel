# Design: ci-cxf-narrow-compile

## Approach

Add a single named step to the existing `full-tests-linux` job in
`.github/workflows/ci.yml`, placed adjacent to the existing wide
compile gate (before "Test (full workspace)"), running exactly:

```
cargo test -p camel-test --features integration-tests-cxf --test cxf_test --no-run
```

The step is unconditional (no `if:`, no `continue-on-error`). A comment
on the step references bd rc-z1ea2 and states why the existing gates do
not cover this path: clippy's `--all-features` and the wide
`--features integration-tests` both activate
`dep:camel-component-cxf`, so they cannot detect a defect confined to
the `integration-tests-cxf` feature line. The narrow invocation enables
default features plus `integration-tests-cxf` — the same selection the
published-bridge smoke compiles — so the PR gate guards exactly what
the smoke builds.

`cargo test --no-run` (not `cargo check --tests`, not clippy) because
it compiles AND links the test binary: a broken native dependency
surface fails at link time, which `cargo check` and clippy never reach.
(Crate-resolution errors — unresolved imports — are caught by all
three; linking is what only `--no-run` adds.) It is also the exact
command shape the smoke runs (`published-bridge-smoke.yml:151`), so the
PR gate guards precisely the smoke's compile.

Placement inside `full-tests-linux` rather than a dedicated job:
follows the rc-8g35d/rc-khjnb compile-gate precedent the
`ci-compile-gate` spec already anchors in this job; no new runner
spin-up and no second rust-cache entry. The narrow step runs after the
wide feature builds in the same target dir, so its marginal cost is the
narrow-feature rebuild delta. Skipped-on-earlier-failure is accepted:
it only affects already-red PRs; every otherwise-green PR runs the
step.

## Affected crates

- None. `camel-test` (feature `integration-tests-cxf`,
  `tests/cxf_test.rs` + `tests/cxf_test/*`) and
  `camel-component-cxf` are referenced but not modified. The only
  product-repo file changed is `.github/workflows/ci.yml`.

## Architecture boundaries

CI-only change; no Runtime / DSL / Components / Services / Languages /
Functions code touched. The delta extends the existing
`ci-compile-gate` capability spec; `published-bridge-smoke-build` and
all cxf-* runtime capabilities are untouched.

## Alternatives considered

- Dedicated `cxf-narrow-compile` job: independent failure signal, but
  adds runner spin-up, a second rust-cache entry, and a second job to
  inventory — rejected as duplication of coverage already anchored in
  `full-tests-linux` (mission requirement 4).
- Narrow clippy line (`cargo clippy -p camel-test --features
  integration-tests-cxf --test cxf_test`): lint overhead on the same
  graph without linking; weaker than `--no-run` for the failure classes
  that matter (crate resolution, link). Rejected.
- `cargo check --tests`: no link stage. Rejected for the same reason.
- Extending `published-bridge-smoke.yml` to run on PRs: explicit
  non-goal (smoke is release-chained by design, rc-cln4q).

## Verification plan

- YAML syntax check (no actionlint/yamllint exists in the repo —
  parser check only; Actions semantics reviewed by hand).
- Baseline pass on the real tree, locally, contained
  (`systemd-run --user`, MemoryMax=12G, TMPDIR=/home/shared/tmp),
  recording elapsed time and cache state; sccache exit 254 → retry with
  `RUSTC_WRAPPER=` empty (fleet standing note).
- Sabotage: `integration-tests-cxf = []` in
  `crates/camel-test/Cargo.toml` (working tree only, reverted) → the
  step command must fail at rustc level (unresolved
  `camel_component_cxf`); restore → must pass. Transcript + exit codes
  in `evidence.md`.
- Cost statement: fresh local elapsed + mission 377 cold bound 449.262 s
  as provenance (historical, not an upper bound for CI cache states).
  `full-tests-linux` runs with a configured 120-minute timeout
  (`ci.yml:73`); a bounded ~7.5-minute cold narrow compile leaves ample
  headroom.

Evidence file: `openspec/changes/ci-cxf-narrow-compile/evidence.md`.
