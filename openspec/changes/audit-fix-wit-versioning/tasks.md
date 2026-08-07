# Tasks: audit-fix-wit-versioning

## Phase 1: WIT versioning + dead code removal + test hardening

### Task 1.1: Version ALL WIT files + remove WIT-006 markers

**Files:**
- `crates/camel-wit/wit/camel-plugin.wit` (modified)
- `crates/camel-wit/wit/camel-bean.wit` (modified)
- `crates/camel-wit/wit/camel-source.wit` (modified)
- `crates/camel-wit/wit/camel-all.wit` (modified)
- `crates/components/camel-component-wasm/wit/camel-plugin.wit` (modified)
- `crates/components/camel-component-wasm/wit/camel-bean.wit` (modified)
- `crates/components/camel-component-wasm/wit/camel-source.wit` (modified)
- `examples/security-wasm-policy/guest-init-check/wit/camel-plugin.wit` (modified)
- `examples/security-wasm-policy/guest/wit/camel-plugin.wit` (modified)
- `examples/wasm-bean-example/wit/camel-bean.wit` (modified)
- `examples/wasm-bean-example/wit/camel-plugin.wit` (modified)
- `examples/wasm-example/guest/wit/camel-plugin.wit` (modified)
- `examples/wasm-source-webhook/wit/camel-plugin.wit` (modified)
- `examples/wasm-source-webhook/wit/camel-source.wit` (modified)
- `examples/wasm-streaming-plugin/guest/wit/camel-plugin.wit` (modified)

**Steps:**
1. In each of the 15 files, change the first line from
   `package camel:plugin;` to `package camel:plugin@1.0.0;`.
2. In each file, remove the `// TODO(WIT-006): ...` comment block (1-2
   lines starting with `// TODO(WIT-006)`). Leave `TODO(WIT-001)` and
   `TODO(WIT-009)` comments untouched — they are out of scope.
3. Verify no `WIT-006` string remains in any `.wit` file.
4. Verify `cargo build -p camel-component-wasm` still succeeds — the
   versioned package declaration is compatible with the existing
   `bindgen!` macro invocations.

**Rationale for doing all 15 files in one task:** the existing
`test_host_wit_matches_canonical` test compares canonical vs host copies
using `strip_comments` (which preserves package declarations). If only
canonical files were versioned, this test would fail on the version
mismatch. Versioning all files atomically keeps the comparison passing.

**Tests:**
- name: `test_wit_constants_contain_package_declaration` (existing, will still pass)
  setup: all 15 WIT files versioned
  action: assert PLUGIN_WIT, BEAN_WIT, FULL_WIT contain `package camel:plugin@1.0.0`
  assert: all assertions pass (substring match still works for versioned string)
  command: `cargo test -p camel-wit test_wit_constants_contain_package_declaration`
  expected: pass
- name: host build succeeds with versioned WIT
  setup: 3 host-copy WIT files versioned
  action: `cargo build -p camel-component-wasm`
  assert: exit code 0
  command: `cargo build -p camel-component-wasm`
  expected: pass

**Acceptance:**
- `grep -rl 'package camel:plugin@1.0.0;' crates/camel-wit/wit/ crates/components/camel-component-wasm/wit/ examples/*/wit/ examples/*/guest/wit examples/*/guest-init-check/wit 2>/dev/null | wc -l` returns 15
- `grep -rl 'WIT-006' --include='*.wit' crates/ examples/ | wc -l` returns 0
- `cargo build -p camel-component-wasm` exits 0

- [ ] 1.1

### Task 1.2: Remove dead runtime code + clean lib.rs + remove camel-api dep

**Files:**
- `crates/camel-wit/src/lib.rs` (modified)
- `crates/camel-wit/Cargo.toml` (modified)

**Steps:**
1. Remove the `// TODO(WIT-006): ...` comment block (3 lines starting at
   line 13) from `src/lib.rs`.
2. Remove the 6 MIME constant declarations: `APPLICATION_JSON`,
   `TEXT_PLAIN`, `APPLICATION_OCTET_STREAM`, `TEXT_HTML`,
   `APPLICATION_XML`, `APPLICATION_FORM_URLENCODED` — including their
   `///` doc comments and the `// ── Common content type constants ───`
   section header.
3. Remove the `wit_dir()` function — including its `///` doc comment block.
4. Remove the `use std::sync::atomic::{AtomicUsize, Ordering};` import.
5. Remove the `use camel_api::CamelError;` import.
6. Remove the `DEFAULT_MAX_RESOURCES` constant.
7. Remove the `WitHost` struct definition, its `impl WitHost` block
   (`new`, `with_max_resources`, `allocate`, `deallocate`,
   `resource_count`, `max_resources`), and the `impl Default for WitHost`
   block.
8. Remove all tests that exercise the removed code:
   `test_wit_host_rejects_beyond_max_resources`, `test_wit_host_default_limit_is_1000`,
   `test_wit_host_allows_up_to_limit`, `test_wit_host_deallocate_frees_slot`,
   `test_wit_host_resource_count_tracks_allocations`,
   `test_wit_host_error_is_processor_error`, `test_wit_dir_exists`,
   `test_wit_dir_contains_expected_files`,
   `test_content_type_constants_compile`.
9. Update `test_wit_constants_contain_package_declaration` to assert
   `.contains("package camel:plugin@1.0.0")` instead of
   `.contains("package camel:plugin")` for PLUGIN_WIT, BEAN_WIT,
   FULL_WIT, AND SOURCE_WIT (add SOURCE_WIT assertion — currently
   missing).
10. In `Cargo.toml`, remove the `[dependencies]` section entirely (it
    contains only `camel-api.workspace = true`).

**Tests:**
- name: `test_wit_constants_contain_package_declaration` (modified)
  setup: WIT files versioned, test updated
  action: assert PLUGIN_WIT, BEAN_WIT, SOURCE_WIT, FULL_WIT each contain `package camel:plugin@1.0.0`
  assert: all four assertions pass
  command: `cargo test -p camel-wit test_wit_constants_contain_package_declaration`
  expected: pass
- name: compilation after dead code removal
  setup: WitHost, MIME constants, wit_dir removed; camel-api dep removed
  action: `cargo build -p camel-wit`
  assert: exit code 0, no errors about missing types or imports
  command: `cargo build -p camel-wit`
  expected: pass
- name: no external callers of removed symbols
  setup: dead code removed
  action: `grep -rn 'WitHost\|APPLICATION_JSON\|TEXT_PLAIN\|APPLICATION_OCTET_STREAM\|TEXT_HTML\|APPLICATION_XML\|APPLICATION_FORM_URLENCODED\|wit_dir' --include='*.rs' crates/ examples/ scripts/ | grep -v 'camel-wit/src/lib.rs' | grep -v target | grep -v .worktrees`
  assert: zero output lines
  command: shell grep (manual verification)
  expected: zero lines

**Acceptance:**
- `cargo build -p camel-wit` exits 0
- `cargo test -p camel-wit` passes (remaining tests only)
- `cargo clippy -p camel-wit -- -D warnings` exits 0
- `grep -c 'WIT-006' crates/camel-wit/src/lib.rs` returns 0
- `[dependencies]` section absent from `crates/camel-wit/Cargo.toml`
- `grep -rn 'WitHost\|APPLICATION_JSON\|TEXT_PLAIN\|APPLICATION_OCTET_STREAM\|TEXT_HTML\|APPLICATION_XML\|APPLICATION_FORM_URLENCODED\|wit_dir' --include='*.rs' crates/ examples/ scripts/ | grep -v 'camel-wit/src/lib.rs' | grep -v target | grep -v .worktrees | wc -l` returns 0

- [ ] 1.2

### Task 1.3: Harden comparison test + update ADR + CONTEXT.md

**Files:**
- `crates/camel-wit/src/lib.rs` (modified)
- `docs/adr/0053-wit-interface-versioning.md` (modified)
- `crates/camel-wit/CONTEXT.md` (modified)

**Steps:**
1. In `crates/camel-wit/src/lib.rs`, replace the
   `test_host_wit_matches_canonical` test:
   - Remove the `if !host_wit_dir.exists() { return; }` guard.
   - Expand to compare all three host-copy files against their canonical
     counterparts: `camel-plugin.wit`, `camel-bean.wit`, `camel-source.wit`.
   - Use the existing `strip_comments` helper for comment-insensitive
     comparison.
   - Assert `host_wit_dir.exists()` (hard-fail if missing instead of skip).
   - The test reads host copies from
     `../components/camel-component-wasm/wit/` relative to `CARGO_MANIFEST_DIR`.
   - Do NOT modify the `test_example_*` tests — their `if !example_dir.exists()`
     guards are out of scope for this change.
2. In `docs/adr/0053-wit-interface-versioning.md`, change the Status line
   from `Accepted; implementation pending (rc-aaxe)` to
   `Accepted; implemented`.
3. In `crates/camel-wit/CONTEXT.md`:
   - Update the dependency posture to reflect zero-dependency leaf
     (camel-api removed).
   - Remove any `WIT-006` references and replace with versioned status.
   - Update the contract surface table if it mentions unversioned package.

**Tests:**
- name: `test_host_wit_matches_canonical` (hardened)
  setup: host-copy and canonical WIT files both versioned identically (all done in Task 1.1)
  action: run the comparison test
  assert: test passes without any `if !host_wit_dir.exists()` guard; all three files compared
  command: `cargo test -p camel-wit test_host_wit_matches_canonical`
  expected: pass
- name: no if-exists guard in host comparison test body
  setup: test hardened
  action: extract the full function body of `test_host_wit_matches_canonical` and verify it does NOT contain `if !host_wit_dir.exists()` and DOES contain references to `camel-plugin.wit`, `camel-bean.wit`, and `camel-source.wit`
  assert: no existence guard; all three filenames present in test body
  command: `awk '/fn test_host_wit_matches_canonical/,/^    }$/' crates/camel-wit/src/lib.rs | grep -c 'if !host_wit_dir.exists'` returns 0 AND `awk '/fn test_host_wit_matches_canonical/,/^    }$/' crates/camel-wit/src/lib.rs | grep -c 'camel-bean\|camel-source'` returns 2+
  expected: 0 and 2+
- name: ADR status updated
  setup: ADR-0053 edited
  action: `grep 'Accepted; implemented' docs/adr/0053-wit-interface-versioning.md`
  assert: at least 1 match
  command: shell grep
  expected: 1+ match
- name: no WIT-006 in CONTEXT.md
  setup: CONTEXT.md updated
  action: `grep -c 'WIT-006' crates/camel-wit/CONTEXT.md`
  assert: returns 0
  command: shell grep
  expected: 0

**Acceptance:**
- `cargo test -p camel-wit test_host_wit_matches_canonical` passes
- The test body of `test_host_wit_matches_canonical` contains zero `if !host_wit_dir.exists` and references all three filenames (`camel-plugin`, `camel-bean`, `camel-source`)
- `grep 'Accepted; implemented' docs/adr/0053-wit-interface-versioning.md` returns 1+ match
- `grep -c 'WIT-006' crates/camel-wit/CONTEXT.md` returns 0

- [ ] 1.3

### Post-phase consolidated gate

**Consolidated WIT-006 negative search** (run after Task 1.3):
- Command: `grep -r 'TODO(WIT-006)' --include='*.wit' --include='*.rs' . | grep -v target | grep -v .worktrees | grep -v docs/archived | grep -v docs/audits | wc -l`
- Expected: 0 (full source tree, excluding only build artifacts, worktrees, and immutable historical docs)

**Full test suite:**
- Command: `cargo test -p camel-wit`
- Expected: all remaining tests pass
