# Proposal: test-count-honesty-b

## Why

`camel test` counts only the verdict rows it produced. Nothing records how many
cases a document declares, so a case the run never reaches leaves both the
numerator and the denominator. The run then looks short instead of incomplete.
Step A (rc-19b09) routed scenario document errors into the parse-error channel.
Two loss paths remain: a settle timeout collapses every declared case of a
document into one `<settle>` row (8 declared cases report as 1 failed), and the
scenario runner stops at the first failing action, so later actions produce no
row. Reference: `.opencode/fleet/inbox/reference/camel-test-count-audit.md`.

## What Changes

- Extract declared case labels per document after parsing. Unit documents
  declare their `expects:` keys, their `reply[i] <to>` labels, and
  `<sequence>`. Scenario documents declare `scenario[i] <kind>` per action. A
  document that fails to read or parse declares nothing.
- Add a `not_run` count: declared minus produced verdict rows, per document.
  Print one additive segment `, N not run` (only when N > 0) plus a trailer
  `— N cases` (and `in M docs` when the run attempted more documents than it
  completed). Clean runs stay byte-identical.
- JUnit: render one `<skipped>` testcase per declared-but-unreached case, add
  the `skipped` attribute, and state the identity
  `tests = passed + failed + errors + skipped`. Document-level errors keep the
  single `<document>` `<error>` testcase.
- Extend the `mock-testkit` spec requirements "Exit codes, reporting, and
  multi-document execution" and "JUnit XML report".

Excluded: execution order, the settle algorithm, the early-stop rule, exit
codes, the JUnit shape for passing cases, and the parse-error wording from
step A.

Affected crates: `camel-cli` (`commands/test` module). bd: rc-ypk3e.

## Acceptance criteria

- A settle-timeout document reports its unreached cases as `not run`; the
  `<settle>` row stays one failed verdict and the exit code stays 1.
- A document-level error or a scenario early stop feeds `not_run` correctly.
- A clean run prints a byte-identical summary and an unchanged JUnit suite.
- Regression tests plus the existing `parse_error_*`, `junit_*`, and clean-run
  tests are green.
- A real CLI run on a three-document fixture shows the summary line and JUnit
  header before and after.
- `bash scripts/gates-local.sh` exits 0 (expected `passed 28, failed 0,
  skipped 4`).

## Risk budget

The change is accounting and reporting only, inside `camel-cli`. The runner
crates keep their semantics, so verdict and exit behavior carry no risk.
Acceptable: summary-line churn on runs that already hid unreached cases.
Out of bounds: runner changes, new exit-code classes, per-case `<error>`
entries for document-level failures.
