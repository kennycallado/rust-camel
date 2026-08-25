# Proposal: test-junit-filters

## Why

`camel test` is the unit tier of the two-tier testing program (ADR-0064) and
now carries the full declarative surface: intercepts, beans, repository
stubs, reply assertions, and matcher grammar. CI adoption has two blockers
left (bd rc-5cbj, last dependency of epic rc-7roi):

- **No machine-readable output.** The driver prints human `PASS`/`FAIL`
  lines and a final `N passed, M failed`. CI systems (GitHub Annotations,
  Jenkins, GitLab) ingest JUnit XML; without it, failures cannot be
  annotated per-case and test counts cannot trend.
- **No way to narrow a run.** The driver takes `FILE|DIR` paths only. CI
  cannot split a corpus into parallel shards, and a developer cannot focus
  a run on one endpoint's documents without hand-listing files.

## What Changes

Driver-layer additions to `camel test` in `camel-cli` (no Runtime, DSL, or
component changes):

1. `--junit <FILE>` — after the run, write a JUnit-format XML report: one
   `testsuite` per document, one `testcase` per endpoint/reply assertion
   row (same labels as the `PASS`/`FAIL` lines), `<failure>` elements
   carrying the mismatch detail, document-level and parse errors as
   `<error>` testcases. The report is written even when the run exits 1 or
   2, so CI can annotate broken documents too.
2. `--filter-file <GLOB>` (repeatable) — keep only expanded documents whose
   path matches the glob (`glob` crate semantics: `*` does not cross `/`,
   use `**`).
3. `--filter-endpoint <NAME>` (repeatable) — after parsing, keep only
   documents whose `expects` map contains one of the given mock endpoint
   names (exact match). Documents that fail to parse still surface their
   error.

Filters compose with AND across kinds and OR within repeats. A filter that
matches nothing is a misuse error (exit 2). Stdout lines, the summary line,
and the exit-code taxonomy (2 > 1 > 0) are unchanged when no flags are
passed.

## Acceptance Criteria

- `camel test doc.test.yaml --junit report.xml` produces well-formed XML
  with exact attribute formulas: `tests = passed + failed + errors`,
  `failures = failed` (the stdout summary counts), and
  `errors =` document-error plus expansion-error testcases.
- Detail text containing XML-significant characters (including the literal
  `<settle>` endpoint label) is escaped.
- Filters narrow the run; survivors' stdout is identical to running them
  directly; filtered-out documents contribute no rows and no counts.
- Existing driver behavior with no flags is byte-identical (existing unit
  tests compile and pass unchanged).

## Risk Budget

Low. Single crate (`camel-cli`), additive flags, no concurrency or
lifecycle surface. Main risk is XML escaping correctness — locked by a
golden byte-level unit test. Second risk is filter semantics ambiguity —
locked by spec scenarios before implementation.
