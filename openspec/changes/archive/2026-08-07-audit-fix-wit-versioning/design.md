# Design: audit-fix-wit-versioning

## Approach

Single-phase implementation: all WIT files versioned atomically (to keep
the cross-crate comparison test passing), then dead code removed, then
test hardened + docs updated. A single phase is required because the
existing `test_host_wit_matches_canonical` comparison test will fail if
canonical files are versioned before host copies (or vice versa).

### Task 1.1: Version ALL WIT files + remove WIT-006 markers

All 15 `.wit` files (4 canonical, 3 host copies, 8 examples) change their
package declaration from `package camel:plugin;` to
`package camel:plugin@1.0.0;`. All `TODO(WIT-006)` blocks in these files
are removed. `TODO(WIT-001)` and `TODO(WIT-009)` markers are left
untouched.

Doing all 15 files in one task is required because the existing
`test_host_wit_matches_canonical` test uses `strip_comments` (which
preserves package declarations) to compare canonical vs host copies.
Versioning canonical without host would cause this test to fail.

### Task 1.2: Dead code removal (rc-m9nn)

Dead runtime code removal from `camel-wit`: `WitHost` struct + `Default`
impl + `allocate`/`deallocate`/`resource_count`/`max_resources` methods,
6 MIME constants (`APPLICATION_JSON` through `APPLICATION_FORM_URLENCODED`),
and `wit_dir()` function. Negative search confirms 0 external callers.
The `camel-api` dependency in `Cargo.toml` is removed — its sole consumer
was `CamelError` in `WitHost::allocate`.

Tests for removed code are deleted. The `test_wit_constants_contain_package_declaration`
test is updated to assert the versioned package string for all four WIT
constants (PLUGIN_WIT, BEAN_WIT, SOURCE_WIT, FULL_WIT).

### Task 1.3: Harden comparison test + update ADR + CONTEXT.md

The auto-skipping cross-crate WIT comparison test
(`test_host_wit_matches_canonical` in `camel-wit/src/lib.rs`) is hardened:
the `if !host_wit_dir.exists() { return; }` guard is removed, making the
test always run and catch drift between canonical and host copies. The
test is expanded to compare all three host-copy files
(`camel-plugin.wit`, `camel-bean.wit`, `camel-source.wit`) against their
canonical counterparts, not just `camel-plugin.wit`.

ADR-0053 status changes from "Accepted; implementation pending (`rc-aaxe`)"
to "Accepted; implemented". `camel-wit/CONTEXT.md` is updated to reflect
zero-dependency leaf + versioned package.

## Affected crates

- **camel-wit**: version 4 canonical WIT files, remove dead code (WitHost,
  MIME constants, wit_dir), remove `camel-api` dep, update tests, update
  CONTEXT.md.
- **camel-component-wasm** (package `camel-component-wasm`): version 3
  host-copy WIT files (text-only change, no structural or bindgen change).

## Architecture boundaries

This change touches the Contract layer (`camel-wit`) and the Runtime layer
(`camel-component-wasm` WIT copies):

- **Contract layer** (`camel-wit`): the WIT package is the ABI contract
  between rust-camel and WASM guests. Versioning it at 1.0.0 establishes
  the compatibility guarantee. Removing dead code restores the
  zero-dependency leaf property — `camel-wit` should not pull runtime types.
- **Runtime layer** (`camel-component-wasm`): only the host-copy WIT files
  receive a version string. No bindgen path change, no dependency change,
  no build infrastructure change.

The change does not modify any interface shape, function signature, record
field, or world definition. WIT shape is unchanged; package identity
intentionally changes from unversioned to `camel:plugin@1.0.0`. This is a
deliberate package-identity break as specified by ADR-0053 §2: pre-v1
unversioned packages have no compatibility guarantee, and the 1.0.0
release establishes the baseline contract.

### rc-osj0 deferral rationale

The original proposal included folding host WIT duplication (rc-osj0) into
canonical consumption. Spec-bless expert (e_gpt) rejected the cross-crate
bindgen path approach for two reasons:

1. **camel-all.wit is not bindgen-safe.** It duplicates world declarations
   from the other three canonical files. `wit-bindgen` 0.58 rejects
   overlapping declarations, so `path: "../camel-wit/wit"` (which includes
   all four files) would fail at compile time.
2. **`cargo package` breaks.** `camel-component-wasm` is publishable with
   `include = ["src/**/*", "wit/**/*", ...]`. A `../camel-wit/wit`
   relative path would not resolve in a packaged `.crate` tarball.

A proper dup fold requires generated bindings (build.rs + OUT_DIR + include!
pattern) or a separate bindgen-safe WIT directory. This is deferred to a
separate change. A4 mitigates drift risk by making the comparison test
non-skipping.

## Phases

Single delivery phase (3 tasks). A single phase is required because the
existing comparison test creates an ordering constraint: canonical and
host WIT files must be versioned atomically to keep the test passing
throughout implementation.

### Phase 1: WIT versioning + dead code removal + test hardening

Scope: `camel-wit` crate + `camel-component-wasm` WIT copies + examples +
ADR + CONTEXT.md.
Exit criteria: all 15 WIT files versioned @1.0.0, all WIT-006 markers
removed, dead code removed, camel-api dep removed, comparison test
hardened (3 files, no auto-skip guard), ADR status updated,
`cargo test -p camel-wit` passes, `cargo build -p camel-component-wasm`
succeeds.
