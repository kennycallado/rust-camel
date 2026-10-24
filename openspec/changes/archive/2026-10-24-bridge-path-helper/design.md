# Design: bridge-path-helper

## Approach

All edits stay in `crates/services/camel-bridge/src/download.rs`. Three private helpers become the
single source of truth:

```rust
fn binary_file_name(spec: &BridgeSpec) -> Cow<'_, str>   // "{name}" or "{name}.exe" (windows)
fn workspace_binary_path_from(root: &Path, spec: &BridgeSpec) -> PathBuf
    // root.join("bridges").join(bridge_dir_name(spec)).join("build").join("native").join(binary_file_name)
fn workspace_binary_path(spec: &BridgeSpec) -> Option<PathBuf>
    // find_workspace_root()? + workspace_binary_path_from
```

(The guard tests deliberately restate the layout as independent oracle literals; the no-duplicate
prohibition below applies to production code and the skip guard, never to the test oracles.)

Path constructors do NOT check existence, canonicalize, or create directories. `ensure_binary_for_spec`
replaces its inline join chain with `if let Some(local_path) = workspace_binary_path(spec)` and its
cache-path tail with `binary_file_name(spec)`; resolution precedence (env override → workspace →
verified cache → download) is untouched. The test guard becomes
`workspace_binary_path(spec).is_some_and(|p| p.is_file())`, preserving skip semantics exactly.

### Guard tests (the false-green kill)

Two complementary tests; neither derives its expectation through the implementation helpers:

1. `workspace_binary_path_matches_documented_layout` (deterministic): for every bridge spec
   (`JMS_BRIDGE`, `XML_BRIDGE`, `CXF_BRIDGE`), `workspace_binary_path_from(temp_root, spec)` must equal
   `temp_root/bridges/{dir}/build/native/{expected}`, where `{expected}` is built independently from the
   documented contract (literal `"bridges"`, `"build"`, `"native"` components + the platform suffix
   rule restated in the test). Catches shared-helper drift.
2. `ensure_binary_uses_workspace_layout` (production wiring): neutralize the binary-path override,
   empty temp cache dir, loopback release URL. Use `CXF_BRIDGE`: the three existing guarded tests all
   use `XML_BRIDGE`, so a transient seeded CXF file cannot flip their skip decisions (worker must
   verify no other test resolves the CXF workspace binary; if one does, fall back to a shared static
   `Mutex` held across seeding→resolution→cleanup and taken by every `workspace_binary_present`
   caller). If `workspace_binary_path(CXF_BRIDGE)` already exists on disk as a regular file, assert
   `ensure_binary_for_spec` returns exactly that path (no tree mutation). Otherwise create parent
   dirs, seed a dummy file with exclusive `create_new`, run the same assertion, then delete only the
   seeded file and the directories this test created. Skip classification for `create_new` errors:
   `AlreadyExists` → recheck and use the existing regular file (assert branch, no skip);
   `PermissionDenied`/read-only filesystem → environmental skip with a visible message; any other
   setup error → fail loudly, never skip. Catches production bypassing the helper.

Evidence protocol (recorded in the change evidence note, sabotage never committed). Three explicit
checkpoints:
- A (pre-extraction, duplicated tree): mutate ONLY production's join (`build` → `out`) → test 2 fails
  with an assertion failure (not a skip, not a setup error). Restore.
- B (post-extraction): mutate ONLY the shared helper's join (`build` → `out`) → test 1 fails (test 2
  stays green because both sites drift together — that is exactly why test 1 exists). Restore.
- C: final clean run — both guard tests green, full `cargo test -p camel-bridge` green, no sabotage
  retained in the tree. Each checkpoint records the command and exit code.

## Affected crates

- `camel-bridge`: helper extraction in `download.rs` + two new tests. No other crate.

## Architecture boundaries

Services support crate only (bridge primitives: binary acquisition). No Runtime, DSL, component, or
data/control-plane impact. `process.rs` is not touched — mission 390 owns it; this change only keeps
consuming `BridgeError` from it as today.

## Alternatives considered

- Root-parameter-only helper (no `workspace_binary_path` wrapper): forces both call sites to repeat
  `find_workspace_root()`; rejected — the wrapper is what makes both sites one call apart.
- End-to-end wiring test alone: cannot see helper-only drift (pre-flight e_gpt: seeding through the
  helper and asserting the same path tolerates shared drift); hence the independent layout test.
- Source-scan test counting format strings: brittle, rejected.
- Seeding with plain write + recursive cleanup: can clobber a developer's real binary or delete
  foreign files; rejected in favour of `create_new` + owned-path-only cleanup.

Refs: bd rc-lnbdx; pre-flight e_gpt ses_e9284710dffedqrE3SabUqVyol.
