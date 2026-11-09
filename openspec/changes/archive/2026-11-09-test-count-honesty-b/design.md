# Design: test-count-honesty-b

## Context

`TestRunSummary` holds `passed`, `failed`, `parse_errors` (crates/camel-cli/src/commands/test.rs:133). All three project from produced rows. The audit names the residual loss paths: a settle timeout collapses every declared case into one `<settle>` row (commands/test/runner.rs:456-464, run_phases settle branch), a scenario run stops at the first failing action (crates/camel-integration-test/src/runner.rs:583-611), and a document-level error produces no rows. This change adds the declared dimension and reports the delta as `not run`.

## Decisions

### D1: Declared cases are the labels a successful run prints

`declared_labels(&ParsedDocument) -> Vec<String>`:

- Unit document: the `expects:` keys in map order, then `reply[{i}] {input.to}` per input that declares `expect_reply` in input order, then `<sequence>` when `sequence:` declares entries. These reuse the exact label constructions the runner emits (`runner.rs:816` for replies, `evaluate_sequence` for `<sequence>`, `evaluate_endpoint` for expects keys).
- Scenario document: `scenario[{i}] {kind}` for every declared action, through `action_label` (commands/test/scenario.rs:126). Visibility widens to `pub(crate)`.
- The `logs:` block is not declared: a successful scenario run prints no `logs` row. The block renders a row only when it fails (`outcome_rows`, scenario.rs), so declaring it would report a spurious `not run` on every passing scenario with a `logs:` block and break the byte-identity rule.
- A document that failed to read or parse declares nothing. It stays in the parse-error channel (step A's class).

Rationale: the mission pins the definition to "the same labels a successful run prints". With this definition every declared label of a fully evaluated document has a row, so the saturating deficit is zero by construction and `not_run == 0` needs no special-casing. (Rows can still exceed declared through logs and teardown rows; D2 covers that.)

### D2: two accounting quantities, stated separately

stdout `not_run` is a per-document saturating deficit of verdict rows:
`not_run(doc) = declared.len().saturating_sub(rows_passed + rows_failed)`.
The run total sums over documents. Saturation is per document: rows beyond the declared set (a failed `logs:` block, a failed `finally[i]` teardown row) never push the deficit negative, and per-document saturation keeps them from reducing another document's `not_run`.

JUnit skipped testcases are label-based: one per declared label that has no row in the suite. This is a different quantity from the stdout deficit. A failing `logs:` block adds a row beyond the declared set: stdout deficit stays 0 and the skipped list stays empty.

Settle: the `<settle>` row is a verdict the document earned, so it reduces the stdout deficit by one. A document with 8 declared cases prints `0 passed, 1 failed, 7 not run`. JUnit still lists all 8 labels as skipped, because none of the 8 labels has a row (D6).

### D3: stdout wording

Full format, one line on stdout:

```
{passed} passed, {failed} failed[, {not_run} not run][, {n} parse-error {doc|docs} (skipped)][ — {total_declared} cases[ in {attempted} docs]]
```

- `, {not_run} not run`: only when `not_run > 0`, immediately after the failed count, before the parse-error segment.
- `, {n} parse-error {doc|docs} (skipped)`: unchanged (step A wording stays).
- ` — {total_declared} cases`: only when `not_run > 0`. `total_declared` sums declared cases over attempted documents that parsed.
- ` in {attempted} docs`: appended only when attempted exceeds completed (definitions in D8).

Byte identity: when `not_run == 0` the line is byte-identical to today's output, for the clean form and the parse-error form. The stderr parse-error naming line is unchanged.

Examples (normative for tests):

| Run | Summary line |
|---|---|
| settle doc, 8 declared, settle timeout | `0 passed, 1 failed, 7 not run — 8 cases` |
| passing unit doc (2 cases) + unit doc error (3 declared) | `2 passed, 0 failed, 3 not run, 1 parse-error doc (skipped) — 5 cases in 2 docs` |
| scenario, 4 actions, action 2 fails | `2 passed, 1 failed, 1 not run — 4 cases` |
| clean run | `5 passed, 0 failed` |
| parse-error only | `0 passed, 0 failed, 1 parse-error doc (skipped)` |

### D4: JUnit skipped testcases and the identity

Per suite, after the row testcases, every declared label that has no row renders in declared order:

```
<testcase name="{label}" classname="{doc-path}"><skipped message="not run: {reason}"/></testcase>
```

Row testcases already carry `classname="{document path}"` (junit.rs:135, 144, 157); skipped testcases keep the same document identity. The `skipped` attribute appears on a `testsuite` and on `testsuites` only when that level holds at least one skipped testcase, and is omitted otherwise, so a fully evaluated document's suite stays byte-identical. On both levels `tests` equals the testcase count and satisfies `tests = passed + failed + errors + skipped`. Suites of fully evaluated documents and expansion error suites are unchanged byte for byte.

### D5: skip reason attribution (precedence order)

1. `doc_error` present → `document error`. Covers route load, boot, input delivery, and the scenario boot/bind/infra classes.
2. No doc error, suite holds a failed `<settle>` row → `settle timeout`.
3. Otherwise, first failing action row `scenario[{i}] …` → `stopped after scenario[{i}] failure`.

No other reasons exist. A document whose rows cover every declared case renders no skipped testcases.

### D6: known consequence, accepted

A settle-collapsed document reports a stdout deficit of `declared − 1` but lists `declared` skipped testcases in JUnit. Cause: the `<settle>` verdict is one produced row (it reduces the deficit), while no declared label owns it (all 8 labels stay rowless). The two quantities are different by construction (D2), not drift: stdout counts a saturating row deficit, JUnit counts rowless declared labels. Rows beyond the declared set (logs, teardown rows) widen the gap in the same direction. The blessing gate reviewed this and accepted it: the alternative, dropping one label from the skipped list to align the integers, hides a real unreached case.

### D7: placement

- New module `declared.rs` in `crates/camel-cli/src/commands/test/`: `declared_labels`, the per-document `not_run` delta, and the skipped-label/reason attribution, with unit tests in `declared_tests.rs`.
- `junit::DocReport` gains `declared: Vec<String>` (empty at read/parse-failure sites). The driver fills it from the parsed document before the run.
- Summary print and `TestRunSummary` in `commands/test.rs`; `TestRunSummary` gains `not_run: usize`.
- `junit.rs` renders skipped testcases and the `skipped` attribute.
- Untouched: `crates/camel-integration-test` (execution order, settle algorithm, early-stop rule), exit codes, `outcome_rows`, the settle row, scenario execution.

### D8: filter and admission interactions

`--filter-endpoint` admits or excludes whole documents by `expects` key membership; an admitted document evaluates all its expects, so declared covers all keys. `--filter-file` and tier filters exclude documents before any `DocReport`: not attempted, no declared cases, no rows. A tier-filter collision on an explicitly named document is parse-class: attempted, not completed, declares nothing. Expansion errors are entry-level, not documents: no declared cases, no skipped testcases. Definitions used by the trailer: attempted = documents that produced a `DocReport` or a parse-error entry; completed = attempted documents that reached evaluation with no document-level error.

### D9: `TestRunSummary` surface

`not_run: usize` is the only new field. `exit_code`, `passed`, `failed`, `parse_errors` keep their meanings. The declared total and the attempted/completed document counts stay internal to the driver print.

## Risks

- Byte-identity break on clean runs: pinned by existing tests (`no_flags_output_is_byte_identical`, `clean_run_has_no_parse_error_segment`) plus new negative-case tests.
- JUnit consumer drift: `skipped` is additive; all-pass suites are unchanged (`junit_all_pass_report` pins the shape).
- sccache contention with a concurrent mission: on `Failed to create temp dir`, stop the server and retry (mission order).

## Out of bounds

Runner semantics, exit codes, the JUnit shape for passing and error cases, renaming the parse-error segment, per-case `<error>` entries for document-level failures.
