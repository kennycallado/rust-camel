# Proposal: clitestpath

Stop hand-rolling the `camel` binary path in `new_test` (bd `rc-z4nzt`): resolve it via cargo's `CARGO_BIN_EXE_camel` so the four subprocess tests survive any placement of the test binary. Test-only regression from CI run `43390879681` (`6cb399cc2`, `v0.68.0`): the test binary ran from `target/debug/build/camel-cli/<hash>/out/`, the pop-pop-push arithmetic resolved to a nonexistent file, and 4 of 18 tests died with `Os { code: 2, NotFound }` at `.output().unwrap()`.

## What changes

- `crates/camel-cli/tests/new_test.rs`: replace the four hand-rolled `CAMEL_BIN_PATH`-override + `current_exe()` pop/pop/push blocks (`fails_if_directory_exists_and_not_empty`, `unknown_template_returns_error`, `rejects_path_traversal_in_name`, `stdout_shows_next_steps`) with `let bin = env!("CARGO_BIN_EXE_camel");` — the same cargo-provided path every other camel-cli test already uses (`tests/common/mod.rs:272,333`, `scaffolded_*` in this file).
- Nothing else: no assertion, message, or test-name changes; no `tests/common/mod.rs` or neighbouring-test refactors; no `crates/**/src/**` file touched.

## Acceptance criteria

- `cargo test -p camel-cli --test new_test` → 18 passed; the same under `--no-default-features --features flavor-regular,exec` → 18 passed.
- The tests no longer depend on where cargo places the test binary (relative `deps/` sibling or `build/<hash>/out/`).
- `cargo fmt --check --all` and `cargo clippy -p camel-cli --all-targets -- -D warnings` pass.

Bd: rc-z4nzt
