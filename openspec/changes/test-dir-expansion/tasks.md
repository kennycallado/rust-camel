# Tasks: test-dir-expansion

## Task 1.1: directory expansion in camel test

**Files:**

- `crates/camel-cli/src/commands/test.rs` (helper, `run_tests` wiring,
  `TestArgs` doc + value_name, `#[cfg(test)]` tests)
- `crates/camel-cli/README.md` (usage line)

**Steps:**

1. Write the tests below first; run `cargo test -p camel-cli
   --bin camel dir_expansion` (or the crate's test target) and confirm
   each new test fails.
2. Add `expand_test_paths(args: &[PathBuf]) -> (Vec<PathBuf>,
   Vec<String>)`: file args pass through; directory args walk
   recursively (skip `target`, `.git`, `node_modules` at any depth),
   keep entries satisfying `camel_dsl::discovery::is_test_document`,
   byte-sorted within the argument; dedupe via `canonicalize` keeping
   first occurrence; zero-document directory yields an error string
   naming it; unreadable path yields an error string.
3. Wire into `run_tests`: expansion errors go to `err`, set the
   parse-error class (exit 2), and the run continues with remaining
   documents.
4. Update `TestArgs` help doc + `value_name = "FILE|DIR"` and the
   module doc-comment; update the README usage line to
   `camel test <FILE|DIR>...`.
5. Run `cargo fmt --check` and
   `cargo clippy -p camel-cli -- -D warnings`; fix findings.

**Tests (write first, name/arrange/act/assert):**

1. `dir_expansion_recursive_sorted` — arrange: tempdir with
   `b.test.yaml`, `a.test.yaml`, `sub/c.test.yml`; act:
   `expand_test_paths(&[dir])`; assert: documents equal
   `[dir/a.test.yaml, dir/b.test.yaml, dir/sub/c.test.yml]`, errors
   empty.
2. `dir_expansion_skips_excluded_dirs` — arrange: tempdir with
   `ok.test.yaml` and `target/gen.test.yaml`; act: expand; assert:
   only `ok.test.yaml` present.
3. `dir_expansion_empty_dir_is_error` — arrange: tempdir with one
   `.keep` file; act: expand; assert: one error naming the dir,
   documents empty.
4. `dir_expansion_dedupes_first_occurrence` — arrange: tempdir with
   `a.test.yaml`; args `[dir, dir/a.test.yaml]`; act: expand; assert:
   `a.test.yaml` appears exactly once.
5. `dir_expansion_file_args_verbatim` — arrange: args `[foo.yaml]`
   (non-suffixed, may not exist on disk beyond the name); act:
   expand; assert: passes through unchanged, no suffix filtering, no
   existence error from expansion itself (existence is the runner's
   read step).

**Acceptance Criteria:**

- Five tests pass; existing camel-cli test-command tests pass.
- `cargo test -p camel-cli` green; `cargo clippy -p camel-cli --
  -D warnings` green; `cargo fmt --check --all` green.
- `camel test <dir>` on the repo's `examples/yaml-dsl/config` runs
  `mock-demo.test.yaml` (manual smoke via the test suite is enough if
  a bin-run is impractical).

- [x] 1.1
