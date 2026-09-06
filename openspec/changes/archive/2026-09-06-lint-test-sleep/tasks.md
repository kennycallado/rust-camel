# Tasks: lint-test-sleep

## scripts/xtask

### Task 1.1: Scanner core — test-fn discovery with module recursion, sleep resolution, scope rules, escape hatch

**Files:**
- `scripts/xtask/src/lint_test_sleep.rs` (new)
- `scripts/xtask/src/main.rs` (modified — add `mod lint_test_sleep;` to the lint module declarations, next to the other `mod lint_*;` lines)

**Steps:**
1. Create `scripts/xtask/src/lint_test_sleep.rs` declaring `#[derive(Debug)] pub struct Finding { pub line: usize }` and `#[derive(Debug)] pub enum ScanError { Parse { file: std::path::PathBuf, err: syn::Error } }` implementing `std::fmt::Display` as `{file}: parse error: {err}` (no `lint-test-sleep` prefix — the dispatch arm adds it) and `std::error::Error`.
2. Implement `pub fn scan_source(source: &str, file: &std::path::Path) -> Result<Vec<Finding>, ScanError>` that:
   - parses with `syn::parse_file`, mapping the syn error into `ScanError::Parse { file, err }`;
   - walks the item tree RECURSIVELY through inline modules (`syn::Item::Mod` with content, e.g. the dominant `#[cfg(test)] mod tests { … }` pattern), maintaining a per-module symbol map of `use` declarations that bind `tokio::time::sleep` or `std::thread::sleep` (plain, grouped, and aliased imports register their visible name); each test fn resolves short-form sleeps through its full enclosing-module chain (file root module + every nested module on the path);
   - selects `syn::ItemFn` items at any module depth whose attributes resolve to `test` or `tokio::test` (args like `#[tokio::test(flavor = "multi_thread")]` still match); other frameworks are not test fns;
   - walks each test fn body with a scope-tracking visitor: a call is a sleep when its callee path is fully-qualified `tokio::time::sleep` / `std::thread::sleep` (leading `::` tolerated) OR a single segment resolving through the enclosing-module symbol map, UNLESS a local binding with that name exists anywhere in the test fn (inner `fn` item, `let` binding, or closure parameter — conservative fn-wide rule, no name-resolution engine);
   - excludes sleep calls inside any `syn::Expr::Closure` and inside nested `fn` items; traverses `async` blocks (`syn::Expr::Async`) as direct body;
   - skips `sleep_until` and every other path;
   - records the finding line as `call_expr.span().start().line` (1-based; proc-macro2 `span-locations` feature is enabled in xtask) and suppresses it when that source line contains `// allow-test-sleep:` followed by non-whitespace text.
3. Add `mod lint_test_sleep;` in `main.rs` next to the existing lint module declarations so the module compiles into the bin.
4. Add a `#[cfg(test)] mod tests` in `lint_test_sleep.rs` with the tests listed below as source-fixture string tests calling `scan_source(SRC, Path::new("fixture.rs"))`.

**Tests:** (all via `cargo test -p xtask lint_test_sleep`)
- `blocking_sleep_in_plain_test_reported`: source `#[test] fn t() { std::thread::sleep(std::time::Duration::from_millis(1)); }` → `scan_source` returns 1 finding whose `line` is the sleep line
- `async_sleep_in_tokio_test_reported`: `#[tokio::test(flavor = "multi_thread")] async fn t()` body `tokio::time::sleep(d).await;` → 1 finding
- `nested_mod_test_with_mod_level_use_reported`: `#[cfg(test)] mod tests { use tokio::time::sleep; #[tokio::test] async fn t() { sleep(d).await; } }` → 1 finding (module-level use resolved through the enclosing chain)
- `sleep_in_closure_not_reported`: body builds a route-like chain `.process(|ex| async move { tokio::time::sleep(d).await; Ok(ex) })` → 0 findings
- `sleep_in_async_block_reported`: body contains `let f = async { tokio::time::sleep(d).await; };` → 1 finding
- `sleep_in_nested_fn_not_reported`: body defines `fn helper() { std::thread::sleep(d); }` and calls it → 0 findings
- `sleep_in_non_test_fn_ignored`: plain `fn not_a_test()` with both sleep forms, inside `mod tests` too → 0 findings
- `short_form_via_use_reported`: file-root `use tokio::time::sleep;` + body `sleep(d).await;` → 1 finding
- `aliased_import_reported`: `use tokio::time::sleep as pause;` + body `pause(d).await;` → 1 finding
- `shadowed_local_symbol_not_reported`: `use tokio::time::sleep;` + inner `fn sleep(_: std::time::Duration) {}` + body `sleep(d);` → 0 findings
- `sleep_until_not_flagged`: body `tokio::time::sleep_until(d).await;` → 0 findings
- `allow_marker_suppresses`: sleep line suffixed ` // allow-test-sleep: simulates slow consumer` → 0 findings
- `empty_allow_marker_does_not_suppress`: sleep line suffixed ` // allow-test-sleep:` → 1 finding
- `parse_error_propagates`: source `fn broken( {` → `Err(ScanError::Parse { .. })` whose `to_string()` contains `fixture.rs`

**Acceptance:**
- `cargo test -p xtask lint_test_sleep` — all tests above pass
- `cargo fmt --check --all` exits 0
- `cargo clippy -p xtask --all-targets -- -D warnings` exits 0

- [x] 1.1

### Task 1.2: Workspace walk wrapper and CLI dispatch

**Files:**
- `scripts/xtask/src/lint_test_sleep.rs` (modified)
- `scripts/xtask/src/main.rs` (modified)

**Steps:**
1. In `lint_test_sleep.rs` extend `ScanError` with variant `Read { file: std::path::PathBuf, err: std::io::Error }` (same Display/Error impls: `{file}: read error: {err}`), add `pub struct Report { pub findings: Vec<(std::path::PathBuf, Finding)>, pub files_scanned: usize }` and `pub fn run(root: &std::path::Path) -> Result<Report, Box<dyn std::error::Error>>`: for each member root `crates/`, `scripts/`, `examples/`, `benchmarks/`, `fuzz/` under `root` — skip with `continue` when the directory does not exist — walkdir collecting `*.rs`, skipping the sibling-lint excluded directories (`target`, `.worktrees`, `node_modules`, `archive`); read each file with `std::fs::read_to_string` (a read failure, e.g. a directory entry named `*.rs`, aborts with `ScanError::Read`), call `scan_source` per file and aggregate; a parse failure aborts with the path-qualified `ScanError::Parse` — either way the caller can exit non-zero with a path-qualified diagnostic.
2. In `main.rs` add `LintTestSleep` to the `Commands` enum (next to `LintContextCitations`, same doc-comment style, documenting the escape hatch marker `// allow-test-sleep:` followed by a non-empty reason) and a dispatch arm that: calls `lint_test_sleep::run(&workspace_root)`; on `Ok(report)` prints one line per finding `{file}:{line}: sleep in test body — use wait_until or a deadline (suppress with an allow-test-sleep marker comment)` then `lint-test-sleep: {n} findings across {distinct} files (advisory; {scanned} files scanned)` and returns success (exit 0 even with findings); on `Err(e)` prints `lint-test-sleep error: {e}` to stderr and exits non-zero, mirroring the neighboring arms' error handling.
3. Extend the `#[cfg(test)] mod tests` with walker tests using `tempfile::tempdir()` fixtures (precedent: `lint_gate_forwarding.rs:435`).

**Tests:** (via `cargo test -p xtask lint_test_sleep`)
- `run_walks_member_trees_and_skips_target`: tempdir with `crates/x/src/a.rs` containing a reportable test sleep, plus `crates/x/target/gen.rs` containing another → `run(root)` reports `files_scanned == 1` and exactly 1 finding
- `run_skips_absent_member_roots`: tempdir containing ONLY `crates/x/src/a.rs` (no scripts/, examples/, benchmarks/, fuzz/) → `run(root)` returns `Ok` with 1 finding, no error from the absent roots
- `run_reports_path_qualified_parse_error`: tempdir with `crates/x/src/broken.rs` containing invalid Rust → `run(root)` returns `Err` whose `to_string()` contains `broken.rs`
- `run_reports_read_error_for_directory_named_rs`: tempdir where `crates/x/src/dir.rs` is a DIRECTORY → `run(root)` returns `Err` whose `to_string()` contains `dir.rs`
- `run_ignores_non_member_roots`: tempdir with `docs/note.rs` containing a reportable test sleep → 0 findings (docs/ is not a member root)

**Acceptance:**
- `cargo test -p xtask lint_test_sleep` passes (Task 1.1 tests still green)
- `cargo xtask lint-test-sleep` (run from the repo root) exits 0 and prints the advisory summary
- `cargo fmt --check --all` and `cargo clippy -p xtask --all-targets -- -D warnings` exit 0

- [x] 1.2

### Task 1.3: Workspace measurement run and debt record

**Files:** (none — verification and measurement task; bd is updated via CLI, not a repo file)

**Steps:**
1. Run `cargo xtask lint-test-sleep` from the repo root; capture the finding count and per-file breakdown.
2. Append an advisory-measurement note to bd rc-99d5.2 via `bd update rc-99d5.2 --append-notes` from the repo root. The note MUST: (a) start with the literal text `ADVISORY MEASUREMENT`; (b) name the measured commit as the output of `git rev-parse --short=8 HEAD` run in the worktree; (c) state the total finding count and the count of files with findings as integers from the run; (d) list the top five offending files as `path:count` pairs ordered by descending count.
3. Re-run fmt and clippy plus exactly these three xtask lints and require exit 0 from each: `cargo xtask lint-unwrap`, `cargo xtask lint-ignore`, `cargo xtask lint-log-levels`.

**Tests:** (operational — no new #[test] fns)
- `workspace advisory run`: `cargo xtask lint-test-sleep` exits 0 with a non-negative integer finding count in the summary line
- `bd record`: `bd show rc-99d5.2 --json` notes contain `ADVISORY MEASUREMENT`

**Acceptance:**
- `cargo xtask lint-test-sleep` exit code 0
- bd rc-99d5.2 notes contain `ADVISORY MEASUREMENT` with the measured integers
- `cargo xtask lint-unwrap` exits 0
- `cargo xtask lint-ignore` exits 0
- `cargo xtask lint-log-levels` exits 0

- [x] 1.3
