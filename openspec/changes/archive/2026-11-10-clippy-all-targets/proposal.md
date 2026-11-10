# Proposal: clippy-all-targets

## Why

The `AGENTS.md` clippy gate runs `cargo clippy --workspace --all-features` with no
`--all-targets`, so `#[cfg(test)]` code and `tests/` targets are never linted by the gate.
Two release-blocking defects already reached `main` as test-only code (bd `rc-e6kz2`):
four `clippy::len_zero` in `camel-test/tests/sql_test.rs` that kept a whole crate red for
every branch, and an unbounded `tokio::time::timeout` wait in a new StreamList test —
the latter caught by the separate `cargo xtask lint-unbounded-wait` gate, not clippy.
Clippy with `--all-targets` is direct motivation for the first class (clippy lints in
test targets invisible to the gate) and would have caught it; it is context, not cure,
for the second. The gate cannot see clippy lints in test code today.

Before changing the gate, the cost had to be measured, not assumed. The measured survey
(see design.md for the full table) shows the residual test-target debt on
`main` (`70a696ae2`) is exactly **one** mechanical lint: a deprecated
`AtomicUsize::fetch_update` call in `camel-master/tests/master_component.rs`. The two
defects cited by the issue were already fixed on main (`6da6a1439` and the StreamList
bounding work).

## What Changes

- Decision **(a)**: add `--all-targets` to the workspace clippy line in the
  `AGENTS.md` QUALITY GATES registry, keeping the existing four excludes
  (`camel-cli`, `camel-component-kafka`, `security-keycloak`, `security-wasm-policy`).
  The literal no-excludes full flag is infeasible on dev boxes: under `--all-features`
  the kafka component pulls `rdkafka-sys`/`sasl2-sys`, whose build script panics without
  native libsasl2 (measured: the run aborts incomplete, having linted 41 of 183
  workspace members before dying on the native dependency).
- One test-only code fix to make the widened gate green: rename
  `.fetch_update(SeqCst, SeqCst, ..)` to `.try_update(..)` in
  `crates/components/camel-master/tests/master_component.rs` — a pure rename with an
  identical signature, no behaviour change, no `#[allow]`.
- No gate registry restructuring, no per-crate opt-in list, no CI workflow edits
  (`.github/workflows/**` untouched, `camel-component-llm/**` untouched). Known
  limitation, recorded in the design: CI hard-codes its own copy of the clippy gate
  (`.github/workflows/ci.yml`), so the registry edit changes local sweeps and the
  merge gate immediately; CI parity needs a workflow edit owned elsewhere — bd
  follow-up filed.

## Acceptance criteria

- The exact new clippy invocation from AGENTS.md exits 0 locally.
- `bash scripts/gates-local.sh` exits 0 (it parses the registry, so no script change).
- No `#[allow]` additions; the only Rust diff is the one-line rename above.
- Design records the measurement table, the decision, and the cost argument.

## Risk budget

- Acceptable: one first-time full run cost (~48 s wall with warm dependency caches) to
  build all test targets; steady-state delta measured at ~0.5 s per warm local run.
  Per-push CI overhead when code changed is unmeasured (see design).
- Out of bounds: silencing lints, changing test behaviour, touching excluded crates'
  manifests, or a hard-coded per-crate opt-in list (drift-prone; rejected in design).
- skip_specs: no spec canon owns the gate registry — `openspec/specs/gate-forwarding-lint/`
  owns the `lint-gate-forwarding` xtask, `ci-compile-gate` only references the
  `--all-features` gate in a comment. Proposed `skip_specs: true`; e_gpt rules.

Affected crates: `camel-master` (test fix), gate registry in `AGENTS.md`.
bd: `rc-e6kz2`.
