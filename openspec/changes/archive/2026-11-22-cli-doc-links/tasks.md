# Tasks: cli-doc-links

## camel-cli

### Task 1.1: Backtick private names in three public doc comments

**Files:**
- `crates/camel-cli/src/compile/materialize.rs` (modified, lines 15 and 138)
- `crates/camel-cli/src/compile/policy.rs` (modified, line 42)

**Steps:**
1. `materialize.rs:15`: change ``[`standard_materialization_candidates`]`` to ``` `standard_materialization_candidates` ```.
2. `materialize.rs:138`: change ``([`standard_materialization_candidates`])`` to ``(`standard_materialization_candidates`)``.
3. `policy.rs:42`: change ``[`is_in_memory_class`]`` to ``` `is_in_memory_class` ```.
4. Touch nothing else (no code, no visibility, no other doc links).

**Tests:** (executable spec)
- `doc_build_camel_cli`: arrange: worktree with steps 1-3 applied → act: `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-cli --no-deps` → assert: exit code 0, no `private-intra-doc-links` output.
- `camel_cli_lane`: act: `cargo test -p camel-cli --no-default-features --features flavor-regular,exec --no-fail-fast` → assert: exit code 0.
- `diff_is_comment_only`: act: `git diff main...HEAD -- crates/` → assert: only 3 changed lines, each a `//!` or `///` line.

**Acceptance:**
- The doc command above exits 0 (before: 101).
- `git diff --stat -- crates/` touches only `materialize.rs` and `policy.rs`, comment lines only.
- `cargo fmt --check -p camel-cli` clean.

- [x] 1.1
