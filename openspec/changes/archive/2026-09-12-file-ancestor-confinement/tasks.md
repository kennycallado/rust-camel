# Tasks: file-ancestor-confinement

## camel-component-file (crates/components/camel-file)

### Task 1.1: Nearest-existing-ancestor confinement + symlink-chain rejection in validate_path_is_within_base

**Files:**
- `crates/components/camel-file/src/lib.rs` (modified)

**Steps:**
1. Rewrite `validate_path_is_within_base(base_dir: &Path, target_path: &Path)` (keep the signature and the existing `base_dir`-absent branch — when the configured base does not exist, nothing below it can be a pre-existing symlink, so the current lexical traversal check stays for that case):
   a. `canonical_base = base_dir.canonicalize()` (existing error mapping).
   b. `rel = target_path.strip_prefix(base_dir)` against the ORIGINAL non-canonicalized base (keep the existing "not under base" and `path_contains_traversal` errors). Do NOT require the lexical target to prefix the canonical base.
   c. Symlink-chain rejection: for each cumulative component `base/c1`, `base/c1/c2`, … along `rel`, call `symlink_metadata`; if the component exists and `file_type().is_symlink()` → reject with `CamelError::ProcessorError` naming the symlinked component (message shape: `"Path '{}' traverses symlinked component '{}' below base '{}'"`). This applies to every component below the base, including symlinks that resolve inside the canonicalized base.
   d. Nearest-existing-ancestor containment: walk `target_path`'s ancestors upward to the nearest existing one (via `symlink_metadata().is_ok()`); `canonicalize()` it; require `starts_with(&canonical_base)`; otherwise reject with the existing "outside base directory" error. If no ancestor below the target exists, the base itself is the ancestor (it exists in this branch) and the check degenerates to base containment.
   e. Delete the now-unreachable lexical-only fallback at lib.rs:1232-1247 (the `else` branch where neither target nor parent exists) — ancestor resolution replaces it.
2. In `FileProducer::call` (lib.rs ~1363-1371), after the `auto_create` `create_dir_all(parent)` block and BEFORE the `config.file_exist` match, add a second `validate_path_is_within_base(dir_path, &target_path)?` call (post-create re-verification: a symlink followed during directory creation yields an outside-base canonical parent and the write is refused before any open/rename; it cannot undo directories created during a concurrent race — that residual stays documented in CONTEXT.md via Task 1.3).
3. Leave `atomic_write.rs`, `validate_relative_filename`, the done-file call site (it already calls the shared validators), and all strategy arms unchanged.

**Tests:** (executable spec — name, arrange, act, assert; `#[cfg(unix)]` for symlink cases; follow the existing inline-test style in lib.rs)
- `validator_rejects_symlinked_ancestor`: tempfile base dir; outside dir; `std::os::unix::fs::symlink(&outside, base/link)`; target `base/link/new/f.txt` → `validate_path_is_within_base` returns `Err` whose message names the symlinked component.
- `validator_rejects_in_base_alias`: base with real dir `base/real`; symlink `base/alias -> base/real`; target `base/alias/new/f.txt` → `Err` (symlink below base rejected even though it resolves inside the base).
- `validator_accepts_missing_deep_path`: base exists, nothing below it; target `base/a/b/c.txt` → `Ok(())`.
- `validator_accepts_symlinked_base`: real dir `real/`; symlink `base -> real`; target `base/x.txt` → `Ok(())` (base itself may be a symlink; only components below base are checked).
- `validator_rejects_existing_outside_target` (regression guard for existing behavior): `base/link -> outside` and target `base/link/existing.txt` where `outside/existing.txt` exists → `Err` naming the symlinked component `link` (the step-1c chain scan fires before the ancestor-containment check; containment is still violated — assert the error message `contains("symlinked component")`).
- `command`: `cargo test -p camel-component-file --lib validator_` — all pass.
- `expected`: `validator_rejects_symlinked_ancestor` and `validator_rejects_in_base_alias` FAIL on the pre-change tree (currently return `Ok`); the three acceptance guards error-or-pass pre-change with the OLD error messages and must still pass post-change with the symlinked-component message where step 1c applies.

**Acceptance:**
- `cargo test -p camel-component-file --lib validator_` exits 0.
- `cargo clippy -p camel-component-file --all-targets -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.
- No new `unwrap()` in the touched functions (`cargo xtask lint-unwrap` clean for lib.rs relative to main).

- [x] 1.1

### Task 1.2: Producer regression matrix — symlinked ancestor across all write paths

**Files:**
- `crates/components/camel-file/src/lib.rs` (modified — tests only)

**Steps:**
1. Add a `#[cfg(unix)]` producer test module (or extend the existing producer tests) with a shared arrange helper: tempfile `base/`, tempfile `outside/` on the SAME filesystem, `symlink(&outside, base.join("link"))`, and a `FileConfig` builder matching the existing producer-test config shape (reuse the harness the current producer tests use to drive `FileProducer::call` with a body and `CamelFileName`).
2. Write the matrix tests below. `TryRename` configs must set `tempPrefix` (e.g. `"tmp-"`); the done-file test uses a SAFE body fileName plus an independently symlinked `doneFileName`.

**Tests:** (executable spec — name, arrange, act, assert)
- `producer_rejects_symlinked_ancestor_all_strategies`: for EACH of `fileExist` = Fail, Append, Override, TryRename: arrange helper + `CamelFileName = link/new/file.txt` → producer call → assert `Err` mentioning the confinement violation AND `outside/new/` does NOT exist AND the `outside/` directory contains no files (no outside artifacts at all).
- `producer_done_file_rejects_symlinked_ancestor`: safe body fileName `body.txt` INSIDE base; `doneFileName = link/done-marker` → producer call → assert `Err` AND `outside/done-marker` does not exist AND `outside/` empty.
- `producer_ignore_probe_rejects_symlinked_ancestor`: arrange helper; pre-create `outside/existing.txt`; `fileExist=Ignore`, `CamelFileName = link/existing.txt` (target exists THROUGH the symlink) → producer call → assert `Err` (NOT the early no-op success) AND `outside/existing.txt` content unchanged.
- `producer_in_base_alias_rejected`: `base/real` dir; `base/alias -> base/real`; `CamelFileName = alias/f.txt` (default Override) → `Err` naming the symlinked component.
- `producer_nested_new_dirs_still_succeed`: no symlinks; `CamelFileName = a/b/c.txt`, `autoCreate=true` → `Ok`, `base/a/b/c.txt` exists with the body.
- `producer_symlinked_base_still_accepted`: real dir + `base -> real` symlink as the configured `directory`; `CamelFileName = x.txt` → `Ok`, file written under the real dir.
- `command`: `cargo test -p camel-component-file --lib producer_` — all pass.
- `expected`: `producer_rejects_symlinked_ancestor_all_strategies` and `producer_in_base_alias_rejected` FAIL on the pre-change tree (outside escape / silent in-base write); `producer_done_file_rejects_symlinked_ancestor` and `producer_ignore_probe_rejects_symlinked_ancestor` already error pre-change via the existing canonicalize path (old "outside base directory" message) and must STAY green post-change (now via the symlinked-component error); the two success tests pass before and after (no over-blocking).

**Acceptance:**
- `cargo test -p camel-component-file --lib` exits 0 (full crate unit suite).
- `cargo clippy -p camel-component-file --all-targets -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 1.2

### Task 1.3: Update CONTEXT.md residual-TOCTOU note

**Files:**
- `crates/components/camel-file/CONTEXT.md` (modified)

**Steps:**
1. In the `fileExist` row of "Accepted names / values" (the bullet describing `O_NOFOLLOW` and the "Ancestor-directory replacement remains a residual TOCTOU surface" sentence): replace the residual-TOCTOU sentence with: intermediate components below the configured base are symlink-REJECTED (including in-base symlinks) and containment is re-verified against the nearest existing ancestor plus a post-create re-check after `autoCreate` — the deterministic symlinked-ancestor escape is closed; the documented residual is now a concurrently planted symlink between validation and mkdir/open (post-create re-verification blocks the write but cannot undo directories created during the race); the base itself may be a symlink.
2. In "Rejected names / values", update the `Symlink leafs on producer opens` bullet: change "Intermediate components are not symlink-protected (residual TOCTOU; see Accepted names / values)" to state that ANY symlinked component below the configured base is rejected (leaf or intermediate, in-base or escaping), with the narrowed concurrent-planting residual referenced.
3. Keep ADR-0016 strictness framing and existing citations; no other rows change.

**Tests:**
- Not a code change — verified by review. `command`: `git diff --stat crates/components/camel-file/CONTEXT.md` shows only the two bullets touched.

**Acceptance:**
- Both residual-note edits present; `cargo xtask lint-context-citations` exits 0 (citation hygiene intact).
- English prose per repo language policy.

- [x] 1.3
