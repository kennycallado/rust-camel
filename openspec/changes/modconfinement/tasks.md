# Tasks: modconfinement

## camel-component-file

### Task 1.1: Extract path-confinement helpers into private module

**Files:**
- `crates/components/camel-file/src/path_guard.rs` (new)
- `crates/components/camel-file/src/lib.rs` (modified)
- `crates/components/camel-file/CONTEXT.md` (modified)

**Steps:**
1. Copy the existing five helpers `validate_relative_filename`,
   `validate_path_is_within_base`, `is_valid_temp_prefix`,
   `open_options_no_follow`, and `path_contains_traversal` from `lib.rs` into
   `path_guard.rs`, changing only module-required visibility from private to
   `pub(crate)` and preserving every function body and `#[cfg(unix)]` attribute.
2. Declare `mod path_guard;` in `lib.rs`, remove the original helper
   definitions, and import the five helpers from `crate::path_guard`; keep all
   existing tests in `lib.rs` and make their existing bare helper references
   resolve through the parent module imports.
3. Update only `camel-file/CONTEXT.md` citations for moved helpers from
   `src/lib.rs` to `src/path_guard.rs`; retain `src/lib.rs` citations for
   `FileConfig::validate` and unrelated logic.
4. Review the diff to confirm no helper body, public API, dependency, or test
   behavior changed, then format the touched Rust files.

**Tests:**
- `test_validate_relative_filename_rejects_evil_values`: existing setup of
  absolute, traversal, and NUL filenames; call the moved validator; assert the
  same rejection errors as before; command `cargo test -p camel-component-file
  --lib test_validate_relative_filename_rejects_evil_values`; expected pass.
- `symlinked_ancestor` tests: existing temporary base and symlinked
  ancestor/leaf setup; run each existing producer strategy; assert rejection and
   no outside write; command `cargo test -p camel-component-file --lib
   symlinked_ancestor`; expected pass.
- `test_rejects_*_temp_prefix` and `test_done_file_rejects_*`: existing invalid
  URI/name setup; exercise configuration and done-file confinement; assert the
  same errors as before; command `cargo test -p camel-component-file --lib`; expected pass.
- `path_guard` compilation and lint: arrange moved module plus parent imports;
  compile and lint the crate; command `cargo clippy -p camel-component-file
  --all-targets -- -D warnings`; expected exit 0.

**Acceptance:**
- `cargo fmt --check --all` exits 0.
- `cargo test -p camel-component-file --lib` exits 0 with all pre-existing tests passing.
- `cargo clippy -p camel-component-file --all-targets -- -D warnings` exits 0.
- `cargo xtask lint-context-citations` exits 0.
- `path_guard.rs` contains all five helpers, `lib.rs` contains none of their
  definitions, and no public API or dependency files changed.

- [x] 1.1
