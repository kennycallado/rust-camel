# Tasks: parsecount

## camel-cli test driver

### Task 1.1: parse-error tracking, summary segment, stderr naming line

**Files:**
- `crates/camel-cli/src/commands/test.rs` (modified)
- `crates/camel-cli/src/commands/test/driver_tests.rs` (modified)

**Steps:**
1. Write the three tests listed under Tests into `driver_tests.rs` first, next to `parse_error_continues_and_exits_two` (they use the existing `temp_dir`, `write_passing`, `write_bad` helpers). Verify they fail (compile error on the new `TestRunSummary` field is the expected failure).
2. Add field `pub parse_errors: usize` to `TestRunSummary` in `test.rs` with doc comment: number of parse-class entries (documents and expansion entries) skipped before producing rows; never counted in `passed` or `failed`.
3. In `run_tests_full`, add `let mut parse_error_names: Vec<String> = Vec::new();` beside `had_parse_error`.
4. At every site that sets `had_parse_error = true`, also push `path.display().to_string()` onto `parse_error_names` (every site has the `path` binding in scope — the expansion-error loop iterates `(path, message)`; push the bare path, never the diagnostic message). The seven sites: the expansion-error loop, read-failure branch, `parse_document`-failure branch, missing-Camel.toml-ancestor branch (its diagnostic embeds the path in a sentence — push only the path), `load_routes`-failure branch, tier-filter-collision branch (explicitly named documents), and the unit-tier `result.doc_error` branch.
5. After the zero-survivor misuse block and before the summary `writeln`, when `parse_error_names` is non-empty write ONE line to `err`: `{n} parse-error doc (skipped): {names}` when `n` is 1, `{n} parse-error docs (skipped): {names}` otherwise, with names comma-space-joined in occurrence order.
6. Change the summary line: when `n > 0` write `{passed} passed, {failed} failed, {n} parse-error doc(s) (skipped)` (singular `doc` when `n` is 1, else `docs`); when `n == 0` keep the existing `{passed} passed, {failed} failed` line unchanged.
7. Set `parse_errors: parse_error_names.len()` in the returned `TestRunSummary`. Fix any other `TestRunSummary` construction sites the compiler reveals.
8. Do not touch exit-code computation, the JUnit writer, or any existing stderr diagnostic line.

**Tests:** (executable spec — name, arrange, act, assert; all tests use `let mut out = Vec::new(); let mut err = Vec::new();` and call `run_tests(&[a, b], &mut out, &mut err).await` with the test's own path list, same shape as `parse_error_continues_and_exits_two`)
- `parse_error_doc_named_in_summary`: temp dir with `write_passing(a)` + `write_bad(bad)`; `run_tests(&[a, bad])` → `summary.exit_code == 2`, `summary.parse_errors == 1`, stdout contains `1 passed, 0 failed, 1 parse-error doc (skipped)`, stderr contains `1 parse-error doc (skipped): ` and `bad.test.yaml`, and stderr's last non-empty line is the naming line.
- `clean_run_has_no_parse_error_segment`: temp dir with two passing docs; `run_tests` → exit 0, stdout contains `2 passed, 0 failed` and does NOT contain `parse-error`, stderr does NOT contain `parse-error`, `summary.parse_errors == 0`.
- `parse_error_only_zero_ran`: temp dir with only `write_bad(bad)`; `run_tests(&[bad])` → exit 2, stdout is exactly `0 passed, 0 failed, 1 parse-error doc (skipped)\n`, `summary.passed == 0`, `summary.failed == 0`, `summary.parse_errors == 1`.

Command: `cargo test -p camel-cli --lib parse_error_doc_named_in_summary clean_run_has_no_parse_error_segment parse_error_only_zero_ran` (note: cargo takes one filter; run each name separately or use the shared `parse_error` prefix filter). Expected before implementation: all three fail.

**Acceptance:**
- `cargo test -p camel-cli --lib -- parse_error_doc_named_in_summary clean_run_has_no_parse_error_segment parse_error_only_zero_ran` — each filter run passes (cargo applies the last filter; run three invocations, one per name).
- Existing `parse_error_continues_and_exits_two`, `precedence_parse_beats_assertion`, and `all_pass_exits_zero` still pass.
- `cargo fmt --check --all` exits 0.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 1.1

### Task 1.2: plural naming, mixed-class counts, expansion-error coverage

**Files:**
- `crates/camel-cli/src/commands/test/driver_tests.rs` (modified)

**Steps:**
1. Write the four tests listed under Tests (TDD: they must pass against Task 1.1 code; if any fails, fix the implementation in `test.rs`, not the test).
2. Follow the existing mixed-run pattern near `junit_document_error_and_expansion` for the expansion fixture: an empty directory argument is the expansion error trigger (`no test documents found`).

**Tests:** (executable spec — name, arrange, act, assert)
- `two_parse_error_docs_plural_and_order`: `write_passing(a)`, `write_bad(b1)`, `write_bad(b2)`; `run_tests(&[a, b1, b2])` → stdout contains `2 parse-error docs (skipped)`, stderr naming line contains `b1.test.yaml, b2.test.yaml` in that order, exit 2, `summary.parse_errors == 2`.
- `parse_error_with_failures_mixes_counts`: `write_failing(f)`, `write_bad(b)`; `run_tests(&[f, b])` → stdout contains `0 passed, 1 failed, 1 parse-error doc (skipped)`, exit 2, `summary.passed == 0 && summary.failed == 1 && summary.parse_errors == 1`.
- `expansion_error_counts_in_summary`: temp dir `empty/` with no files; `run_tests(&[empty_dir])` → exit 2, stdout is exactly `0 passed, 0 failed, 1 parse-error doc (skipped)\n`, stderr contains `1 parse-error doc (skipped): ` and the directory's displayed name, `summary.parse_errors == 1`.
- `expansion_error_alongside_passing_doc`: passing file `a.test.yaml` plus an empty directory argument (pattern of the existing mixed-expansion test) → stdout contains `1 passed, 0 failed, 1 parse-error doc (skipped)`, exit 2.

Command: `cargo test -p camel-cli --lib two_parse_error_docs_plural_and_order` plus one invocation per remaining name. Expected: all pass after Task 1.1; any failure is an implementation gap in the naming or counting, fixed in `test.rs`.

**Acceptance:**
- All four new tests pass individually.
- Full driver module green: `cargo test -p camel-cli --lib commands::test` exits 0.
- `cargo fmt --check --all` exits 0; `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 1.2
