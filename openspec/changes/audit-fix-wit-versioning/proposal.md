# Proposal: audit-fix-wit-versioning

## Why

The `camel:plugin` WIT package is unversioned. ADR-0053 (Accepted) mandates
`camel:plugin@1.0.0` before the v1.0 freeze so that guests compiled against the
release have a compatibility contract. The same audit surfaced a batched
same-crate issue: dead runtime code in the contract crate (rc-m9nn).

## What Changes

**Affected crates:** `camel-wit`, `camel-component-wasm`

**Included:**
- Version all 15 WIT files from `package camel:plugin;` to
  `package camel:plugin@1.0.0;` (4 canonical in `camel-wit/wit/`, 3 host copies
  in `camel-component-wasm/wit/`, 8 examples across `examples/*/wit/`).
- Remove all `TODO(WIT-006)` markers from WIT files and `lib.rs`.
- Remove dead runtime code from `camel-wit` (rc-m9nn): `WitHost` struct +
  impl + `Default`, 6 MIME constants (`APPLICATION_JSON` etc.), `wit_dir()`
  function, `camel-api` dependency. Makes `camel-wit` zero-dependency.
- Replace the auto-skipping cross-crate WIT comparison test
  (`test_host_wit_matches_canonical`) with a hard assertion (no
  `if !exists { return; }` escape hatch) that compares ALL THREE host-copy
  files (`camel-plugin.wit`, `camel-bean.wit`, `camel-source.wit`) against
  their canonical counterparts.
- Update ADR-0053 status from "implementation pending" to "implemented".
- Update `camel-wit/CONTEXT.md` to reflect zero-dependency + versioned state.

**Excluded (deferred to separate change):**
- rc-osj0 (host WIT dup fold): the cross-crate bindgen path approach
  (`../camel-wit/wit`) is NOT viable because (a) `camel-all.wit` in the
  canonical directory has overlapping world declarations that
  `wit-bindgen` rejects, and (b) relative cross-crate paths break
  `cargo package` for the publishable `camel-component-wasm` crate. This
  needs its own design (checked-in generated bindings or build-time
  generation) and is tracked separately. A4 mitigates the drift risk by
  making the comparison test non-skipping.
- Per-element `@since` annotations (ADR-0053 §6: conditional on toolchain
  support; for the initial 1.0.0 establishment every element is introduced at
  the same version, so `@since` adds no value at this point).
- WIT-001 (camel-all.wit generation from canonical sources) — separate issue.

## Acceptance criteria

- All 15 WIT files declare `package camel:plugin@1.0.0;`.
- No `TODO(WIT-006)` markers remain in any source file.
- `camel-wit` has zero runtime dependencies (`camel-api` removed).
- `cargo build -p camel-wit -p camel-component-wasm` succeeds.
- `cargo test -p camel-wit` passes.
- The `test_host_wit_matches_canonical` test runs unconditionally (no
  `if !exists { return; }` guard) and compares all three host-copy files.
- ADR-0053 status reads "Accepted; implemented".
- rc-aaxe, rc-m9nn ready to close. rc-osj0 remains open (deferred).

## Risk budget

**Acceptable:**
- Removing `WitHost`/MIME constants is safe — 0 external callers confirmed
  by negative search (`rg` across `crates/`, `scripts/`, `examples/`).
- Making the comparison test hard may surface drift if the host and canonical
  WIT files have diverged. The diff has been verified empty today, so the
  test will pass.

**Out of bounds:**
- Changing any WIT interface shape (types, functions, worlds) — this change
  only adds a version, no semantic contract change.
- Adding `@since` annotations (deferred per ADR-0053 §6).
- Eliminating host WIT duplication (rc-osj0 — deferred to separate change).
