# Tasks: bridge-path-helper

## camel-bridge

### Task 1.1: Production-wiring guard test + checkpoint-A red evidence (pre-extraction)

**Files:**
- `crates/services/camel-bridge/src/download.rs` (modified — tests module only)
- `openspec/changes/bridge-path-helper/evidence.md` (new)

**Steps:**
1. Verify seeding safety: `rg -n "CXF_BRIDGE" crates/services/camel-bridge/` and confirm no existing
   test resolves the CXF workspace binary or calls `workspace_binary_present(&CXF_BRIDGE)`.
   - If clear (expected): proceed; the wiring test needs no locking.
   - If a collision exists: do NOT stop — implement the spec's mutex fallback instead: add
     `static WORKSPACE_PATH_LOCK: std::sync::Mutex<()>` (initialized via `std::sync::OnceLock`),
     have `workspace_binary_present` acquire it (guard held for the whole check), and have the
     wiring test hold it from before seeding until after cleanup; note the fallback in the task
     report. (If the collision is in non-test production code: STOP and report
     `test-design-gap: CXF used by production path <site>`.)
2. In the `#[cfg(test)] mod tests` of `download.rs`, add a test-oracle helper
   `fn test_oracle_workspace_path(spec: &BridgeSpec) -> Option<PathBuf>` that replicates the CURRENT
   guard construction independently (`find_workspace_root()` + `bridges`/`bridge_dir_name(spec)`/
   `build`/`native`/`{name or name.exe}` literal components — no shared production code).
3. Parameterize the env helpers: change `fn neutralize_binary_override()` to
   `fn neutralize_binary_override(spec: &BridgeSpec)` (remove `spec.env_binary_path`) and
   `fn set_loopback_release_url()` to `fn set_loopback_release_url(spec: &BridgeSpec)` (set
   `spec.env_release_url` to `http://127.0.0.1:9`); update the three existing call sites to pass
   `&XML_BRIDGE`. Required so the CXF wiring test neutralizes the CXF override and stays offline
   under checkpoint-A sabotage (ambient CXF overrides would bypass workspace resolution, and an
   un-neutralized CXF release URL would let a drifted production path contact GitHub).
4. Add test `ensure_binary_uses_workspace_layout` (tokio, no cfg gate beyond what tokio needs):
   - setup: `neutralize_binary_override(&CXF_BRIDGE)`; `set_loopback_release_url(&CXF_BRIDGE)`;
     empty `tempfile::TempDir` as cache dir; target = `test_oracle_workspace_path(&CXF_BRIDGE)`
     (assert `Some`, the repo tree always satisfies the `bridges/` sentinel).
   - if target exists as a regular file: assert
     `ensure_binary_for_spec(&CXF_BRIDGE, "0.0.0-test", cache).await` returns exactly `target`
     (no tree mutation branch).
   - else: `create_dir_all(target.parent())`, seed dummy bytes with
     `std::fs::File::options().write(true).create_new(true)` — on `AlreadyExists` recheck and use
     the existing regular file; on `PermissionDenied` or read-only fs return early with
     `eprintln!("SKIP: checkout not writable: {e}")`; any other error `panic!`. Then assert
     `ensure_binary_for_spec(&CXF_BRIDGE, "0.0.0-test", cache).await` is `Ok(exactly target)` —
     an `Err` (production missed the seeded path) or a different `Ok` path fails the assert.
   - cleanup before any assert that could fail: remove the seeded file (only if this test created
     it) and the `bridges/cxf/build/native` dirs this test created (bottom-up, only if empty);
     guard cleanup with a struct whose `Drop` also attempts it so an assert cannot leak the dummy.
5. Record checkpoint A in `evidence.md`: (clean) run
   `cargo test -p camel-bridge --lib download::tests::ensure_binary_uses_workspace_layout` → green,
   command + exit code; (sabotage, NOT committed) temporarily change production's `join("build")` to
   `join("out")` in `ensure_binary_for_spec`, re-run → RED with an assertion failure (not a skip,
   not a setup error), record output tail + exit code; restore the exact original line, re-run →
   green, record. `git diff` must show no sabotage residue before committing.

**Tests:**
- `ensure_binary_uses_workspace_layout`: seeded dummy at the oracle CXF workspace path, empty temp
  cache, loopback release URL → `ensure_binary_for_spec` → `Ok` path equals the seeded path
  exactly; tree restored afterwards.
  command: `cargo test -p camel-bridge --lib download::tests::ensure_binary_uses_workspace_layout`
  expected: green on the clean duplicated tree (current guard and production agree); RED
  (assertion) under the checkpoint-A production-drift sabotage; green again after restore.

**Acceptance:**
- `cargo test -p camel-bridge --lib download::tests::ensure_binary_uses_workspace_layout` exits 0
  on the restored tree, and the test executed (not skipped).
- `evidence.md` records checkpoint A with clean/sabotage/restored commands and exit codes.
- `cargo fmt --check` and `cargo clippy -p camel-bridge --all-targets -- -D warnings` exit 0.
- `git status` clean after commit; `git diff HEAD~1 -- crates/` contains no `join("out")`
  (planning/evidence prose is excluded from the residue check).

- [x] 1.1

### Task 1.2: Extract shared helper, rewire both sites, layout guard test, checkpoints B/C

**Files:**
- `crates/services/camel-bridge/src/download.rs` (modified)
- `openspec/changes/bridge-path-helper/evidence.md` (modified)

**Steps:**
1. Add the three private helpers to `download.rs` (above `ensure_binary_for_spec`):
   `fn binary_file_name(spec: &BridgeSpec) -> Cow<'_, str>` (returns `Cow::Owned(format!("{}.exe",
   spec.name))` under `cfg!(target_os = "windows")`, else `Cow::Borrowed(spec.name)`);
   `fn workspace_binary_path_from(root: &Path, spec: &BridgeSpec) -> PathBuf`
   (`root.join("bridges").join(bridge_dir_name(spec)).join("build").join("native")
   .join(binary_file_name(spec).as_ref())` — pure, no existence check);
   `fn workspace_binary_path(spec: &BridgeSpec) -> Option<PathBuf>`
   (`Some(workspace_binary_path_from(find_workspace_root()?, spec))`).
2. Rewire production: `ensure_binary_for_spec` drops its inline `binary_file_name` let-binding and
   its `bridges/{dir}/build/native/{name}` join chain — workspace check becomes
   `if let Some(local_path) = workspace_binary_path(spec)` (debug! logs unchanged); cache
   bin path tail becomes `.join(binary_file_name(spec).as_ref())`. Doc comment resolution-order
   line 2 stays accurate (same path).
3. Rewire the skip guard: `workspace_binary_present` becomes
   `workspace_binary_path(spec).is_some_and(|p| p.is_file())`; delete its inline construction.
   Crate scan: `rg -n -e '"build"' -e '"native"' -e '\.exe' crates/services/camel-bridge/src/`
   must show no remaining production/skip-guard copies of the layout or suffix rule (the guard
   tests' oracle literals are exempt).
4. Add test `workspace_binary_path_matches_documented_layout`: for each of `JMS_BRIDGE`,
   `XML_BRIDGE`, `CXF_BRIDGE`, build the expectation independently from local literals —
   `expected = root.join("bridges").join(dir).join("build").join("native").join(expected_name)`
   where `dir = spec.name.strip_suffix("-bridge").unwrap()` and `expected_name` restates the
   platform suffix rule locally (`"{name}.exe"` on Windows cfg, else `spec.name`) — and assert
   `workspace_binary_path_from(root, spec)` equals `expected` for a `tempfile::TempDir` root.
   Expectations MUST NOT call the implementation helpers.
5. Switch `ensure_binary_uses_workspace_layout`'s seed site from `test_oracle_workspace_path` to
   `workspace_binary_path(&CXF_BRIDGE)` (the oracle fn stays only if the layout test needs it; the
   wiring test's seed may now legitimately be the shared helper — the layout test remains the
   independent oracle).
6. Record checkpoint B in `evidence.md`: temporarily change the HELPER's `join("build")` to
   `join("out")`, run
   `cargo test -p camel-bridge --lib download::tests::` → `workspace_binary_path_matches_documented_layout`
   RED, `ensure_binary_uses_workspace_layout` GREEN (both sites drift together); record commands +
   exit codes; restore, re-run both → green.
7. Record checkpoint C in `evidence.md`: full `cargo test -p camel-bridge` green (command + exit
   code), at least one executed non-skipped wiring run named, statement that no sabotage remains
   (`git diff` clean).
8. Commit.

**Tests:**
- `workspace_binary_path_matches_documented_layout`: temp root, all three specs →
  `workspace_binary_path_from` equals the independently built documented path; RED under
  checkpoint-B helper drift, GREEN after restore.
  command: `cargo test -p camel-bridge --lib download::tests::workspace_binary_path_matches_documented_layout`
  expected: green after extraction; RED under the checkpoint-B helper-drift sabotage; green after restore.
- `ensure_binary_uses_workspace_layout`: unchanged assertion, seed site now the shared helper →
  stays GREEN under checkpoint-B helper drift (proves why the independent layout oracle exists).
  command: `cargo test -p camel-bridge --lib download::tests::ensure_binary_uses_workspace_layout`
  expected: green on clean tree and under checkpoint-B drift.

**Acceptance:**
- `cargo test -p camel-bridge` exits 0.
- `cargo fmt --check` and `cargo clippy -p camel-bridge --all-targets -- -D warnings` exit 0.
- Step-3 scan output pasted into the PR/task report shows no production/skip-guard duplicates.
- `evidence.md` contains checkpoints A, B, C with commands and exit codes; committed tree has no
  sabotage in source: `git grep -n 'join("out")' -- crates/ scripts/` exits 1 (planning/evidence
  prose is excluded from the residue check).

- [x] 1.2
