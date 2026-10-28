# Proposal: xtask-walkdir-errors

## Why

`cargo xtask lint-*` gates walk the tree with `walkdir` and most sibling lint
modules swallow walk errors via `.filter_map(|e| e.ok())`. An unreadable
directory or file silently disappears from the scan and the gate prints `OK`,
so gate integrity is fail-open. `lint_no_deprecated` (bd rc-9fji0 follow-up
from mission 433) already propagates walk errors; this change applies the same
fail-closed pattern to the sibling lint modules under `scripts/xtask/src/lint_*.rs`.

Tracked as bd **rc-9fji0** (P3, tech-debt sweep, gate integrity).

## What Changes

- Add a small shared walker helper in a new `scripts/xtask/src/walk.rs` module:
  a walkdir error becomes `Err("cannot walk <path>: <error>")` (same message
  shape as `lint_no_deprecated`) and aborts the lint with exit 1.
- Apply the helper at 5 `.filter_map(|e| e.ok())` sites across the 5 affected
  lint modules:
  - `lint_cancel_tokens.rs` (1 site)
  - `lint_component_deps.rs` (1 site)
  - `lint_context_citations.rs` (1 site — the violation-discovery walk in
    `lint_context_citations`)
  - `lint_metric_labels.rs` (1 site)
  - `lint_single_source.rs` (1 site)
- Two `lint_context_citations.rs` sites are PARKED, not swept (mission
  Required #4 — documented fail-open semantics): `parse_crate_items`
  (documented "best-effort cross-reference… silently skipped") and
  `collect_crate_src_dirs` (same symbol-table mechanism for that
  cross-reference). Filed as a deferral with ticket classification; the
  owner decides whether the citation lint wants fail-closed walks.
- Modules that already propagate (`lint_no_deprecated`,
  `lint_no_env_mutation_unguarded`, `lint_no_free_port`,
  `lint_no_raw_http_test_server`, `lint_test_sleep`, `lint_unbounded_wait`,
  `archive.rs`) are NOT touched.
- Out of scope (deferred): the 14 fail-open walk sites in `main.rs` (inline
  lints such as `lint_unwrap`, `lint_secrets`, `lint_log_levels`,
  `lint_log_redaction`). Recorded in the deferrals ledger; separate follow-up.
- No rule changes, no threshold changes, no output-format changes. Happy-path
  output (`lint-x: OK (N findings)`) is preserved byte-for-byte.

## Acceptance criteria

- `cargo test -p xtask` green, including a new deterministic injected-error
  test proving a walk error aborts the lint instead of passing silently, and
  a CLI-level test asserting exit code 1 with the affected path on stderr.
- Every touched `cargo xtask lint-*` gate is green on the repo tree and its
  output is byte-identical to the pre-change baseline.
- `cargo fmt --check --all` and `cargo clippy -p xtask -- -D warnings` clean.
- `cargo xtask lint-unwrap` green.
- `openspec validate --all --strict` green; no spec change (`skip_specs: true`).

## Risk budget

- Mechanical sweep; the only semantic delta is fail-open → fail-closed on
  walk errors, which on the real tree never triggers (all scan roots exist).
- `lint_component_deps` and `lint_single_source` keep their
  `if !dir.exists() { return Ok(...) }` guards — a missing scan root is not
  a walk error and stays `Ok`. `lint_cancel_tokens` has no such guard, so a
  missing `crates/components` becomes `Err` (the `lint_no_deprecated` house
  behavior); its synthetic-tree tests are updated accordingly if any relied
  on silent fail-open.
- Existing unit tests that call the touched lints on synthetic trees will be
  updated only where they relied on fail-open (mechanical, in keeping with
  the `lint_no_deprecated` house pattern).
- Two documented-fail-open sites in `lint_context_citations.rs` are parked
  unchanged (see What Changes); no semantic change is made to them.

## Affected crates

- `xtask` (`scripts/xtask`): new `walk.rs` helper module + 5 lint modules.
