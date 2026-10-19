# Tasks: camel-lint-directories

Conventions for every task:
- Work only inside this worktree. Never run cargo in the main checkout.
- Every cargo build/test battery runs inside the containment scope
  (replace `<n>` with the task's unit number, and use the exact cargo
  invocation the task spells out):
  `systemd-run --user --scope --collect --unit=fleet-368-<n> -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 <task cargo command> -j4`
  then `systemctl --user stop fleet-368-<n>.scope`. `pgrep -c compiled_artifa`
  must print 0 before and after each battery. Scratch only under
  `/home/shared/tmp/368/`, never `/tmp`.
- Test code may use `unwrap()` only with the trailing `// allow-unwrap`
  comment, matching existing style in `lint.rs` tests.
- Clippy per task: `cargo clippy -p camel-cli -- -D warnings` (scoped).
  Format: `cargo fmt --check --all`.

### Task 1.1: Golden byte-identity fixtures for single-file mode

Capture stdout/stderr/exit of the PRE-change `camel` binary for the four
single-file cases (clean, error, missing, reserved) into committed
fixtures. Later tasks compare post-change output byte-for-byte against
these files, proving single-file mode is unchanged.

**Files:**
- `crates/camel-cli/tests/golden/lint-file/cwd/g-clean.yaml` (new)
- `crates/camel-cli/tests/golden/lint-file/cwd/g-error.yaml` (new)
- `crates/camel-cli/tests/golden/lint-file/cwd/g-reserved.test.yaml` (new)
- `crates/camel-cli/tests/golden/lint-file/g-clean.stdout`, `g-clean.stderr`, `g-clean.exit` (new)
- `crates/camel-cli/tests/golden/lint-file/g-error.stdout`, `g-error.stderr`, `g-error.exit` (new)
- `crates/camel-cli/tests/golden/lint-file/g-missing.stdout`, `g-missing.stderr`, `g-missing.exit` (new)
- `crates/camel-cli/tests/golden/lint-file/g-reserved.stdout`, `g-reserved.stderr`, `g-reserved.exit` (new)

(NOT under `tests/fixtures/` — the `lint_corpus` corpus walker sweeps
`crates/**/tests/fixtures/**/*.{yaml,yml,json}` and would pull the
golden inputs into the baseline gate. `tests/golden/` is outside that
glob.)

**Steps:**
1. Create `cwd/` with exactly three files (byte-exact content):
   - `g-clean.yaml`: `id: gclean\nfrom: timer:gold?period=1000\nsteps:\n  - to: log:gold\n`
   - `g-error.yaml`: `id: gerror\nfrom: direct:start\nsteps:\n  - to: timer:gold?bogusOption=1\n`
   - `g-reserved.test.yaml`: `routeFiles: [g]\nexpects: {}\n`
   (`g-missing.yaml` is deliberately absent.)
2. Build the binary once, scoped:
   `systemd-run --user --scope --collect --unit=fleet-368-0 -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo build -p camel-cli -j4`,
   then stop the unit. Binary lands at `target/debug/camel`; if that
   relative location does not exist, resolve the target directory via
   `cargo metadata --no-deps --format-version 1` (field
   `target_directory`) and use `<target_directory>/debug/camel`. The
   capture outputs stay reproducible because the lint arguments are the
   relative file names, never the binary path.
3. For each case `c` in `g-clean`, `g-error`, `g-missing`, `g-reserved`:
   from inside `cwd/` run
   `../../../../../../target/debug/camel lint <case-arg>` (six `..`
   levels: cwd → lint-file → golden → tests → camel-cli → crates →
   worktree root)
   with stdout redirected to `../<c>.stdout`, stderr to `../<c>.stderr`,
   and `$?` written to `../<c>.exit`, where `<case-arg>` is exactly the
   file name for clean/error/reserved (`g-clean.yaml`,
   `g-error.yaml`, `g-reserved.test.yaml`) and `g-missing.yaml` for the
   missing case. Paths in outputs are the relative names as typed — that
   is what makes the goldens reproducible.
4. Determinism proof: repeat step 3 writing to
   `/home/shared/tmp/368/golden2/<c>.{stdout,stderr,exit}` and compare the
   12 capture files one by one with `cmp` against the committed captures
   (do NOT `diff -r` the parent dir — it would report the `cwd/` input
   tree as "Only in"). All 12 must be byte-identical.
5. Verify exit files contain `0` (clean), `1` (error), `2` (missing),
   `0` (reserved); verify `g-clean.stdout` and `g-clean.stderr` are empty.
6. `git add` the 15 files and commit
   `test(camel-cli): golden captures for lint single-file mode`.

**Tests:** (this task IS fixture creation; verified by step 4-5 checks)

**Acceptance:**
- 15 files committed; `git status` clean after commit.
- Second capture byte-identical (step 4 diff empty).
- Exit files read 0/1/2/0; clean stdout+stderr empty.

- [x] 1.1

### Task 1.2: Directory walker `collect_lint_targets`

**Files:**
- `crates/camel-cli/src/commands/lint.rs` (modified)

**Steps:**
1. Add `pub fn collect_lint_targets(root: &Path) -> (Vec<PathBuf>, Vec<(PathBuf, std::io::Error)>)`
   to `lint.rs`, exactly per blessed design.md:
   - Depth-first pre-order over `std::fs::read_dir`; each directory's
     entries sorted by file-name `OsStr` bytes before visiting.
   - Eligible extension: file name ends in `.yaml`, `.yml`, or `.json`,
     case-sensitive.
   - Candidate only when `std::fs::metadata(entry.path())` (follows
     symlinks) reports `is_file()`; on metadata error for an
     eligible-extension name, push `(path, err)` into the failures vec
     (dangling symlink case). Non-eligible names: metadata errors ignored.
     Non-file kinds (dir handled by descent; FIFO/socket/device ignored).
   - Below the root only: prune entries whose file name starts with `.`
     and directories named `target` (never descended, never collected).
     No pruning at the root itself.
   - Never descend into symlinked directories: use
     `entry.file_type()` (no-follow) — symlinked dir entries are skipped
     entirely; symlinked FILES pass the metadata-is_file check above and
     are collected.
   - `read_dir` failures on subdirectories push `(dir_path, err)` into
     the failures vec and continue the walk.
2. Keep the function free of any printing/output coupling (caller owns
   reporting).
3. If `commands` is private to the binary target and clippy flags
   `collect_lint_targets` as `dead_code` before Task 1.3 wires it, mark it
   `#[cfg_attr(not(test), allow(dead_code))]` in this task; Task 1.3
   removes the attribute when dispatch starts using it. Do NOT merge the
   two tasks.
4. Commit: `feat(camel-cli): add lint directory walker`.

**Tests:** (unit tests in `mod tests` in `lint.rs`; sync `#[test]`;
`tempfile::tempdir()` trees; assert with `assert_eq!` on the full
`Vec<PathBuf>` of targets)
- `walker_collects_exact_eligible_set`: tree with `a.yaml`, `sub/b.yml`,
  `sub/c.json`, `notes.txt`, `D.YAML`. `D.YAML` is not eligible
  (case-sensitive), `notes.txt` is not eligible. DFS pre-order with sorted
  entries visits root names `a.yaml` then `sub/`, and inside `sub/` sorts
  `b.yml` before `c.json`. Assert targets are exactly, in order:
  `[<root>/a.yaml, <root>/sub/b.yml, <root>/sub/c.json]`.
- `walker_prunes_hidden_and_target_below_root`: tree with
  `.hidden/r.yaml`, `target/f.json`, `routes/g.yaml` → targets exactly
  `[routes/g.yaml]`.
- `walker_hidden_root_and_target_root_are_linted`: two roots
  `<tree>/.hidden` and `<tree>/target`, each with one eligible file →
  walking each root returns that file.
- `walker_symlink_dir_not_descended_symlink_file_collected`: unix-only
  (`#[cfg(unix)]`): tree with `loop` symlink → root (cycle) and `link.yaml`
  symlink → real `real.yaml`; sorted by name the targets are exactly
  `[link.yaml, real.yaml]` and the walk terminates (test finishing proves
  it).
- `walker_fifo_ignored`: unix-only: create the FIFO with
  `Command::new("mkfifo").arg(path)` (no new dependency) at `pipe.yaml` →
  targets empty, failures empty.
- `walker_dangling_symlink_is_failure`: unix-only: `broken.yaml` symlink →
  nonexistent target → targets empty, failures has exactly
  `(broken.yaml, _)`.
- `walker_unreadable_subdir_reported_with_root_gate`: create subdir, set
  permissions `0o000`; if the process can still read it (root), restore
  `0o755` and return early (skip); else failures contains the subdir and
  targets from the rest of the tree still collect; restore `0o755` before
  returning so `TempDir` cleanup does not leak files under
  `/home/shared/tmp`.
- `walker_sorted_dfs_order_trap`: tree with `b.yaml`, `a-b.yaml` in root
  and `a/z.yaml` → order `[a/z.yaml, a-b.yaml, b.yaml]`.

Battery (unit `fleet-368-1`):
`systemd-run --user --scope --collect --unit=fleet-368-1 -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo test -p camel-cli --lib -j4`

**Acceptance:**
- All `walker_*` tests pass under the scoped battery; `pgrep -c compiled_artifa` is 0 before/after.
- `cargo fmt --check --all` clean;
  `cargo clippy -p camel-cli --all-targets -- -D warnings` clean;
  `cargo clippy -p camel-cli --no-default-features --features flavor-regular,exec --all-targets -- -D warnings` clean (both scoped, unit `fleet-368-1b`).
- `cargo xtask lint-unwrap` and `cargo xtask lint-test-sleep` exit 0.
- No printing inside `collect_lint_targets`.
- Committed as `feat(camel-cli): add lint directory walker`.

- [x] 1.2

### Task 1.3: Directory pipeline `run_lint_dir` + dispatch + summary

**Files:**
- `crates/camel-cli/src/commands/lint.rs` (modified)
- `crates/camel-cli/tests/lint_directories.rs` (new)

**Steps:**
1. Add `pub struct DirLintOutcome { pub exit_code: i32, pub linted: usize, pub skipped: usize, pub failed: usize, pub diagnostics: usize }`.
2. Add `pub async fn run_lint_dir(root: &Path) -> DirLintOutcome`:
   - FIRST probe `std::fs::read_dir(root)` (root-abort check): on error,
     print `error: failed to read '{root.display()}': {e}` to stderr and
     return exit 2 with all counts 0 — no warning, no summary, no walk.
     Remove the Task 1.2 `#[cfg_attr(not(test), allow(dead_code))]`
     attribute from `collect_lint_targets` (dispatch now uses it).
   - Build the engine ONCE via `production_engine()`; on error print
     `error: {e}` to stderr and return exit 2 with all counts 0 (no
     summary line, no walk).
   - Call `collect_lint_targets(root)`. If the target list is empty, print
     `warning: no route files found under {root.display()}` to stderr.
   - Per target in walker order:
     a. `camel_dsl::discovery::is_reserved_document(path)` → print the
        single-file wording `skipped: {path.display()} is a reserved
        document (camel test or camel job)` to stdout, `skipped += 1`.
     b. Else `std::fs::read_to_string` error (includes dangling symlink
        and non-UTF-8) → print `error: failed to read '{path.display()}': {e}`
        to stderr, `failed += 1`.
     c. Else run `engine.lint(&source)`, apply the
        `is_stagec_exempt_path(path)` R-MOCK filter (as-typed path, never
        canonicalized), render each diagnostic with the existing
        `render_diagnostic(diag, path_str, &source)` where `path_str` is
        the walked path as a string; count diagnostics of all severities;
        `linted += 1`; set `saw_error` when any severity is Error.
   - AFTER the per-target loop and BEFORE the summary line: print each
     walker failure as `error: failed to read '{path.display()}': {e}` to
     stderr (covers both dangling eligible symlinks and unreadable
     subdirectories) and count each in `failed`.
   - Print exactly one summary line to stdout:
     `linted {N} file(s), {M} skipped, {F} failed, {K} diagnostic(s)`.
   - Exit code: 2 if `failed > 0`; else 1 if `saw_error`; else 0.
3. In `run(args)`: `match std::fs::metadata(&args.file)` — `Ok(m)` with
   `m.is_dir()` → `run_lint_dir`, print nothing extra, exit with its
   code; `Ok(_)` (file) or `Err(_)` (missing/dangling) → existing
   single-file path UNCHANGED.
4. Never print a summary or warning in single-file mode.
5. Commit: `feat(camel-cli): lint directories recursively`.

Route templates pinned for every tree-building test in this task:
- clean route: the `g-clean` body with a unique `id` per file, i.e.
  `id: <unique>\nfrom: timer:gold?period=1000\nsteps:\n  - to: log:gold\n`
- error route: `id: <unique>\nfrom: direct:start\nsteps:\n  - to: timer:x?bogusOption=1\n`
- warning route: `id: <unique>\nfrom: direct:start\nsteps:\n  - to: mock:x\n`

**Tests:** (integration `tests/lint_directories.rs`; `#[tokio::test]`;
in-process `run_lint_dir` for outcome assertions; subprocess via
`env!("CARGO_BIN_EXE_camel")` + `Command::current_dir(cwd_fixture_dir)` for
byte-stream assertions)
- `dir_lints_exact_eligible_file_set`: tree `a.yaml`(clean), `sub/b.yml`(clean), `sub/c.json`(clean), `notes.txt`, `D.YAML` → `linted == 3`, exit 0.
- `dir_exit_one_when_one_file_errors`: 2 clean + 1 route with `to: timer:x?bogusOption=1` → exit 1.
- `dir_exit_zero_all_clean`: 3 clean routes → exit 0, `diagnostics == 0`.
- `dir_exit_two_and_continues_on_read_failures`: unix tree with invalid-UTF-8 file `bad.bin.yaml` (write `std::fs::write` bytes `0xFF`), dangling `broken.yaml`, one error route → in-process: exit 2, `failed == 2`, `linted == 1`, `diagnostics >= 1`; subprocess variant: count stderr lines matching `^error: failed to read '` — exactly 2 (never count bare `error: ` prefixes; rendered diagnostics use capitalized `Error[CODE]:` headers but do not rely on that).
- `dir_reserved_docs_skipped_with_info`: clean route + `demo.test.yaml` + `demo.job.yml` → `linted == 1`, `skipped == 2`, exit 0.
- `dir_only_reserved_docs_exits_zero`: only reserved docs → `linted == 0`, `skipped == 2`, exit 0.
- `dir_summary_counts_every_outcome`: unix tree with 2 clean + 1 mock-route warning (`to: mock:x`, non-exempt path) + 1 reserved + dangling `broken.yaml` → fields exactly `linted 3, skipped 1, failed 1, diagnostics 1`, exit 2 (failed dominates 1).
- `dir_visit_order_sorted_depth_first`: root with `b.yaml`, `a-b.yaml` (each an error route) and `a/z.yaml` (error route) → subprocess stderr contains all three file headers in byte order `a/z.yaml` < `a-b.yaml` < `b.yaml`; build each needle as `root.join(name).display().to_string()` so `b.yaml` cannot substring-match inside `a-b.yaml` (the needle includes the path separator).
- `dir_unreadable_root_aborts_without_summary`: unix-only: create a dir, set mode `0o000`, skip early (restoring `0o755`) if the process can still read it (root); subprocess `lint <dir>` → exit 2, stderr matches `^error: failed to read '`, and NEITHER stdout nor stderr contains `linted ` or `warning: no route files found`; restore `0o755` before returning.
- `dir_repeated_runs_byte_identical`: subprocess twice on the same tree
  (include one clean route, one error route, one `demo.test.yaml`, and one
  `demo.job.yml` so the skip info wording is exercised on stdout) →
  stdout bytes equal AND stderr bytes equal, and some stdout line matches
  `^skipped: .*demo\.test\.yaml is a reserved document \(camel test or camel job\)$`
  (the `.*` absorbs the invocation-form path prefix; build the needle from
  the same arg form the test passes).
- `dir_empty_dir_warns_and_exits_zero`: subprocess on empty dir → stdout contains `linted 0 file(s), 0 skipped, 0 failed, 0 diagnostic(s)`, stderr contains `warning: no route files found`, exit 0.
- `dir_stagec_exemption_inside_tree`: `tests/fixtures/route.yaml` under temp root sends `to: mock:x` → no R-MOCK warning; identical file at `<tmp>/other/route.yaml` → R-MOCK warning present.
- `missing_root_exits_two`: subprocess `lint nonexistent-dir/` → exit 2, stderr has `error:`.
- `single_file_golden_byte_identity`: for each committed golden case, subprocess with `current_dir = tests/golden/lint-file/cwd`, arg as in Task 1.1 → stdout/stderr byte-equal to the committed `.stdout`/`.stderr`, exit equal to `.exit`.
- `single_file_prints_no_summary_line`: on the golden clean capture output, assert no line matches `^linted \d+ file\(s\), \d+ skipped, \d+ failed, \d+ diagnostic\(s\)$` (the byte-identity test already implies it; keep this explicit assertion).

Batteries (units `fleet-368-2`, `fleet-368-2b`):
`systemd-run --user --scope --collect --unit=fleet-368-2 -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo test -p camel-cli --test lint_directories -j4`
`systemd-run --user --scope --collect --unit=fleet-368-2b -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo test -p camel-cli --lib --test lint_test_doc_skip --test lint_corpus -j4`

**Acceptance:**
- All tests above pass under scoped batteries; `pgrep -c compiled_artifa` 0 before/after.
- `cargo fmt --check --all` clean;
  `cargo clippy -p camel-cli --all-targets -- -D warnings` clean;
  `cargo clippy -p camel-cli --no-default-features --features flavor-regular,exec --all-targets -- -D warnings` clean.
- `cargo xtask lint-unwrap` and `cargo xtask lint-test-sleep` exit 0.
- Existing single-file tests (`--lib` `lint_*`, `tests/lint_test_doc_skip.rs`, `tests/lint_corpus.rs`) still pass.
- No summary line in single-file mode.
- Committed as `feat(camel-cli): lint directories recursively`.

- [x] 1.3

### Task 1.4: Docs + corpus-walker follow-up bd

**Files:**
- `docs/src/cli/index.md` (modified)
- `docs/src/getting-started/cli.md` (modified)

**Steps:**
1. `docs/src/cli/index.md`: change the command-table entry from
   `camel lint <FILE>` to `camel lint <PATH>`; in the `## camel lint`
   section add a directory-mode paragraph: accepts a directory, recurses
   for `.yaml`/`.yml`/`.json` (case-sensitive), prunes hidden entries and
   `target/` below the root, never descends symlinked directories, skips
   reserved documents (`*.test.yaml`, `*.test.yml`, `*.job.yaml`,
   `*.job.yml`) with info lines, deterministic depth-first sorted order,
   aggregated exit codes (2 root/read failures > 1 any error diagnostic >
   0), one summary line on stdout. Note that non-route documents inside
   the tree are linted and yield R-SCHEMA errors, same as naming them
   individually.
2. `docs/src/getting-started/cli.md`: update the lint table row to
   "Lint a route file or a directory tree" with example
   `camel lint routes/`.
3. From the REPO ROOT (`/home/kenny/dev/rust-camel`, never the worktree)
   create the follow-up bd:
   `bd create "camel lint: evaluate reusing collect_lint_targets in the lint_corpus corpus walker" --description="camel-lint-directories design decision: crates/camel-cli/tests/lint_corpus.rs keeps its own walker. Differences to reconcile if ever attempted: corpus walker includes hidden entries, roots at two trees (examples/ and crates/**/tests/fixtures), excludes examples/validator/schemas, and feeds the baseline-compare contract; CLI walker prunes hidden/target below root and has no output coupling." -t task -p 3 --deps discovered-from:rc-61fgd --json`
   Record the new bd id in the commit body (`Bd: rc-xxxx`) of the docs
   commit.
4. Commit: `docs(camel-cli): document camel lint directory mode`.

**Tests:** (docs-only; verified by STAGE 4 doc gate)
- doc-build gate: `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-cli --no-deps` (scoped, unit `fleet-368-3`) exits 0.

**Acceptance:**
- Both docs mention directory mode with the exact policy points above.
- Follow-up bd exists with `discovered-from:rc-61fgd`; id recorded.
- Doc gate passes.

- [x] 1.4
