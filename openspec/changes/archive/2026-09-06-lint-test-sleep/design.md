# Design: lint-test-sleep

## Approach

A syn-based AST scanner in `scripts/xtask`, following the module layout of the
existing lints (`lint_context_citations.rs`, `lint_metric_labels.rs`) and the
CLI dispatch pattern of `lint_unwrap` in `main.rs`. syn 2.x (`full`, `visit`)
and walkdir are already workspace dependencies of xtask.

Pipeline:

1. **File walk** — walkdir over every Cargo workspace member tree (member globs
   per root `Cargo.toml`: `crates/`, `scripts/`, `examples/`, `benchmarks/`,
   `fuzz/`), collecting `*.rs` and skipping `target/` and other build output
   (same skip rules as the other lints). Non-member directories (`docs/`,
   `site/`, `openspec/`) are not scanned.

2. **Test-fn discovery** — parse each file with `syn::parse_file`; select
   `syn::ItemFn` whose attributes contain a path resolving to `test` or
   `tokio::test` (attribute args like `#[tokio::test(flavor = "multi_thread")]`
   must still match). Functions annotated with other frameworks (rstest & co.)
   are not test fns for v1 — recorded decision, misses are acceptable in an
   advisory lint. Nested `fn` items inside a test body are NOT part of the test
   body: their contents are excluded from scanning.

3. **Sleep-call resolution** — a call expression is a sleep when its callee
   path is:
   - fully qualified `tokio::time::sleep` or `std::thread::sleep` (any leading
     `::`), or
   - a short form (`sleep(...)`) whose single path segment resolves through a
     `use` declaration visible in the scope chain of the test fn: the use items
     of the file root module and of every enclosing module of the fn. Grouped
     imports (`use tokio::time::{sleep, sleep_until}`) and aliased imports
     (`use tokio::time::sleep as pause;` → `pause(...)`) both feed the symbol
     map. A locally-defined binding named identically (local `fn sleep`, `let
     sleep = ...`, fn parameter) shadows the import and suppresses flagging —
     conservative direction. `sleep_until`, `interval`, and third-party timer
     APIs are deliberately NOT flagged (bd rc-99d5.2 names sleep calls only).

4. **Closure scoping** — while walking a test fn body the visitor tracks scope
   kind. A sleep call in the direct test-fn body — including inside `async`
   blocks, which are traversed — is a finding. A sleep inside any
   `syn::Expr::Closure` expression — passed to `.process()`, route builders,
   `tokio::spawn`, or any other receiver — is excluded, as is any sleep inside
   a nested `fn` item. Boundaries are defined structurally, not semantically:
   the lint accepts that a closure used AS synchronization (e.g. a sleep inside
   a `wait_until` condition closure) is a false negative; that is the safe
   error direction for an advisory measurement, and the broad exclusion avoids
   the fragile, drifting builder-method-name list that precise `.process()`-arg
   detection would require.

5. **Escape hatch** — `// allow-test-sleep: <reason>` on the same line as the
   sleep call suppresses the finding, where `<reason>` MUST contain non-whitespace
   text (an empty marker does not suppress). Line-comment model identical to
   lint-unwrap's `// allow-unwrap`.

6. **Reporting** — `cargo xtask lint-test-sleep` prints `file:line: sleep in
   test body — use wait_until or a deadline (// allow-test-sleep: <reason> to
   suppress)` per finding plus a summary count. Exit code is 0 when scanning
   completes (findings included); a file that fails to parse or read produces a
   path-qualified diagnostic on stderr and a non-zero exit, because an
   unparsable file makes the report untrustworthy. Registered in `main.rs`
   dispatch next to the other lints.

**Module shape**: the scanner exposes `scan_source(source: &str, file: &Path)
-> Result<Vec<Finding>, ScanError>` as a pure function (unit-testable without
touching the filesystem; `ScanError::Parse` carries the syn error) plus a
`run(root) -> Result<Report>` walk wrapper that aggregates per-file results and
propagates per-file diagnostics. Test-fn discovery and closure-scope tracking
live in small reusable helpers so bd rc-3lx2 (R1 timeout-dominance) can extend
the same visitor instead of growing a parallel one.

## Affected crates

- `scripts/xtask`: new `lint_test_sleep.rs` module, dispatch entry in
  `main.rs`, unit tests in the module (`#[cfg(test)]`). No other crate changes.

## Architecture boundaries

Dev-tooling plane only. No Runtime / DSL / Components / Services / Languages /
Functions code is read or modified beyond being lint input. The lint observes
source text; it has no runtime linkage. Aligned with ADR-0069 s13 (test
determinism rules) and the rc-99d5 adjudication; the enforcement policy itself
(hard-fail, diff-gating) is deferred to the governance track (bd rc-jwp3) and a
later change.

## Alternatives considered

- **Lexical brace-depth scan (lint-unwrap precedent)** — rejected: cannot
  distinguish closure scoping, which is the entire point (bd rc-99d5.2 states
  this explicitly).
- **Regex line scan** — rejected for the same reason; also false-positives on
  comments and string literals.
- **Custom rustc/clippy lint** — heavier toolchain integration; xtask is the
  established lint home in this repo (10 existing `cargo xtask lint-*` gates).
- **Precise `.process()`/builder-arg closure detection** — deferred: needs a
  maintained list of builder method names that drifts with the DSL; the broad
  closure exclusion is the conservative advisory v1.
- **Separate R8 rule** — rejected by adjudication: this lint is the seed of the
  structural scanner family that bd rc-3lx2 (R1) extends.

Single-phase change: one coherent slice, no milestone grouping needed.
