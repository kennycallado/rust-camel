# Proposal: 487-template-test-debt

## Why

Four behavior-neutral test/docs debt items remain from missions 462 and 470
(bd `rc-1shz2`, `rc-qkxbm`, `rc-bsg0z`, `rc-zc6gc`). They share one crate and
one theme: test code that grew past the maintainability boundary, or an
assertion/doc that no longer pins the promised contract.

- `crates/components/camel-template/src/template_set.rs` is 1147 lines, ~73%
  of it the in-file `mod tests`. The production surface is ~314 lines.
- `crates/components/camel-template/src/producer.rs` is 1061 lines; ~110 of
  them are self-contained tracing-capture test infra that other tests in the
  crate could reuse.
- `dump_serializes_object_array_scalar_deterministically` builds its fixture
  from alphabetically ordered keys, so it cannot distinguish sorted-key
  serialization from insertion order; the docs promise sorted keys.
- `docs/testing/tracing-capture-guards.md` claims a 22-site inventory
  snapshot that predates several guard additions, including the
  camel-template one.

## What Changes

Test code and test documentation only.

- `template_set.rs` keeps production code and declares `#[cfg(test)] mod
  tests;`; the test body moves verbatim to `src/template_set/tests.rs`
  (private access and the crate's `feature = "default"` gate preserved).
- The `dump` fixture switches to non-alphabetical object keys
  (`{"b": 1, "a": 2}` -> `{"a":2,"b":1}`), pinning sorted output for
  unsorted input. Evidence note: minijinja stores object keys sorted, so the
  test pins the output contract, not a proof that the filter resorts.
- The tracing-capture helpers move from `producer.rs` to a crate-level
  `#[cfg(test)] src/test_support.rs` (the camel-dsl natural-home pattern),
  exposed as `pub(crate)` only for `capture_warns`.
- `docs/testing/tracing-capture-guards.md` receives an accurate
  re-inventory: 30 guard bodies in 28 files as of this change, including
  the moved camel-template home; stale `error_handler.rs` entry removed.

Explicitly excluded: production code, public API, new dependencies, new
tests beyond the listed assertion change, a new crate, and any archive or
merge of this change.

## Acceptance criteria

- `cargo test -p camel-template --lib` stays green at the same test count
  before and after (126 lib tests: 28 template_set + 17 producer); test names
  unchanged.
- `cargo test -p camel-template --lib --no-default-features` compiles with
  zero warnings and runs only the 23 non-engine tests (baseline measured
  before the edit; no `template_set`/`producer`/`test_support` test).
- `cargo fmt --check`, `cargo clippy -p camel-template --all-targets -- -D
  warnings` (both feature modes), the 24 `cargo xtask lint-*` gates runnable locally (25 minus `lint-commits`, which fetches `origin/main` and is CI-owned) plus `schema --check`, rustdoc `-D warnings`
  for `camel-template`, `cd docs && mdbook test`, and `openspec validate
  --all --strict` pass.
- No file under `crates/*/src/` gains or loses non-test behavior; the diff
  is `#[cfg(test)]` code plus `docs/testing/`.

## Risk budget

Acceptable: mechanical file moves inside `camel-template` and one assertion
tightening. Out of bounds: any runtime change, renamed or deleted test,
weakened assertion, new dependency, public API change, or edit outside
`crates/components/camel-template/` and `docs/testing/`.
