# Design: parsecount

## Approach

`run_tests_full` in `crates/camel-cli/src/commands/test.rs` already tracks
the parse class with the boolean `had_parse_error`, set at seven sites:
expansion errors, unreadable file, `parse_document` failure, missing
Camel.toml ancestor, `load_routes` failure, tier-filter collision on an
explicit document, and unit-tier input delivery failure. Every site prints
its diagnostic to stderr at failure time and produces no rows.

Add `parse_error_names: Vec<String>` as the single source of truth for
the parse class. Each site pushes
`path.display().to_string()`. `had_parse_error` is derived
(`!parse_error_names.is_empty()`) before the exit computation, so a future
parse-class site cannot set one half of the pair without the other. The
derived boolean stays
the exit-code input; the vector drives the new output:

- Before the summary line, when the vector is non-empty, stderr gets one
  line: `{n} parse-error doc (skipped): {names}` when `n` is 1, otherwise
  `{n} parse-error docs (skipped): {names}`, names comma-space-joined in
  occurrence order.
- The stdout summary line becomes
  `{passed} passed, {failed} failed, {n} parse-error docs (skipped)` when
  `n > 0` (singular `doc` when `n` is 1). Unchanged when `n == 0`.
- `TestRunSummary` gains `pub parse_errors: usize`. The struct has one
  construction site (`run_tests_full`); the external caller (`main.rs`,
  via `run_tests_full`) consumes `exit_code`, so the field is additive and
  no caller change is needed.

Write order at the tail of `run_tests_full`: loop diagnostics (unchanged),
misuse message (unchanged), new naming line on `err`, summary on `out`,
JUnit report (unchanged). Exit-code computation and the JUnit writer are
untouched: parse errors already surface there as `<error>` testcases, and
the JUnit `tests` formula reconciles `passed + failed + errors`, so the
additive stdout segment introduces no drift.

## Affected crates

- camel-cli: `src/commands/test.rs` (tracking, naming line, summary
  segment, `TestRunSummary` field) plus driver tests in
  `src/commands/test/driver_tests.rs`. No other crate.

## Architecture boundaries

CLI presentation layer only. No runtime, DSL, component, or service code
changes; data/control plane untouched. The change reuses the existing
parse-class taxonomy (ADR-0069 section 7 neighborhood, exit precedence
396415e8) and adds no new class.

## Alternatives considered

- Fold parse errors into `failed`: rejected. It conflates document-level
  and case-level failure and breaks the documented exit-code precedence.
- Exit-code-only fix: rejected. Exit codes are already correct; the gap is
  the summary line.
- JUnit-only fix: rejected. The reporting callers in the bug gate on the
  human summary line, not the report.

Single-phase change; no phase decomposition.
