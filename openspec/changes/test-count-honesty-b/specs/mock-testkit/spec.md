## MODIFIED Requirements

### Requirement: Exit codes, reporting, and multi-document execution

`camel test` SHALL execute documents in CLI argument order, sequentially. A
document-level error (unreadable file, parse error, boot failure, or input
delivery failure such as a processor error propagating out of the route)
SHALL be reported and execution SHALL continue with the next document; a
document whose input delivery failed SHALL skip settling, endpoint
evaluation, and reply evaluation for that document. Exit codes: 0 when every
expectation of every document passes; 1 when any expectation (endpoint or
reply) fails, a settle timeout occurs, or a scenario verdict fails
(`receive-timeout`, `validation-mismatch`);
2 for misuse, unreadable files, document/route parse errors, input delivery
failures, and apparatus failures (runtime `scenario-var-unresolved` — an
unset variable is an authoring bug — `doc-validation`, `tier-filter-collision`,
`partner-bind-failure`, `partner-startup-failure`, `action-transport-failure`,
`infra-unavailable`, `full-boot-failure`, `shutdown-failure`). When classes
coexist, precedence is 2 > 1 > 0. stdout SHALL carry one `PASS`/`FAIL` line
per endpoint per document, one `PASS`/`FAIL` line per asserted reply per
document, one line per scenario action verdict for scenario documents, a tier
annotation per document, and a final `N passed, M failed`
summary. The tier annotation SHALL tell the truth about execution: `lean`
for lean-derived documents, `full` for scenario documents (which execute
through the full boot), and `full*` for unit documents that derive FULL but
execute on the lean boot — the lean registry is pinned (ADR-0064: direct,
log, mock, seda, timer) and is not grown for unit documents. Each `full*`
unit document SHALL also print one stderr advisory naming the lean registry
and the scenario-tier alternative. A FULL-derived unit document that fails
with a document-level error SHALL have its failure text carry an appended
hint naming the lean registry and directing wasm and other full-boot
components to a scenario document; a bare `Component not found: <scheme>`
alone SHALL NOT be the reported failure for these documents. A
`shutdown-failure` after a recorded verdict SHALL NOT mask the
verdict.

When one or more entries fail with a parse-class error (expansion errors,
unreadable files, document/route parse errors, boot failures, tier-filter
collisions on explicitly named documents, and input delivery failures), the
summary line SHALL carry the passed and failed counts plus an additive
segment `, N parse-error docs (skipped)` (`doc` singular when N is 1).
Parse-class failures SHALL NOT count toward the passed or failed counts, and
the exit-code rules above SHALL apply unchanged. stderr SHALL name every
parse-error entry on one line, in occurrence order, immediately before the
summary line; the per-failure stderr diagnostics SHALL stay as before. When
no parse-class failure occurred and no case is `not run`, the summary SHALL
read exactly `N passed, M failed` and stderr SHALL carry no naming line.

The run SHALL count the cases each attempted document declares, using the
row labels a successful run prints: for a unit document the `expects:` keys,
the `reply[i] <to>` label per input that declares a reply expectation, and
`<sequence>` when a sequence is declared; for a scenario document the
`scenario[i] <kind>` label per declared action. A document that fails to
read or parse SHALL declare no cases. The run SHALL count a case as
`not run` when its document parsed but the case produced no verdict row:
per document, `not run` is the declared case count minus the document's
verdict rows, saturated at zero. Verdict rows beyond the declared set (a
failed `logs:` block, a failed teardown row) SHALL NOT push the count
negative. The summary SHALL carry the additive segment `, N not run`
immediately after the failed count and only when N > 0, placed before any
parse-error segment. When the segment appears, the summary SHALL end with
` — N cases`, where N is the total declared case count over attempted
documents that parsed, and ` in M docs` SHALL follow when the run attempted
more documents than completed. An attempted document is a document that
produced a report or a parse-error entry; a completed document is an
attempted document that reached evaluation with no document-level error.
Documents excluded silently by a document filter, and expansion-level
entries, are neither attempted nor completed; a tier-filter collision on an
explicitly named document is attempted and not completed. A
settled-but-timed-out document keeps its one `<settle>` failed verdict row;
the declared cases it replaced SHALL count as `not run`. When no
parse-class failure occurred and no case is `not run`, the summary SHALL
read exactly `N passed, M failed`; when parse-class failures occurred and
no case is `not run`, the summary SHALL read exactly the parse-error form
above with no trailer. The exit-code rules SHALL apply unchanged, and a
`not run` case SHALL NOT move the exit code.

#### Scenario: all pass
- **Given** a document whose expectations all hold
- **When** `camel test <doc>` runs
- **Then** stdout lists PASS lines and a summary with zero failed, and the exit code is 0

#### Scenario: any failure exits 1
- **Given** two documents where the second has one failing expectation
- **When** `camel test a.test.yaml b.test.yaml` runs
- **Then** both documents' endpoints are evaluated and reported, and the exit code is 1

#### Scenario: parse error with assertion failure exits 2
- **Given** a first document with one failing expectation and a second document that fails parsing
- **When** `camel test a.test.yaml bad.test.yaml` runs
- **Then** both documents are attempted, the parse error is reported, and the exit code is 2

#### Scenario: malformed document exits 2
- **Given** a document that is not valid YAML or fails schema validation
- **When** `camel test <doc>` runs
- **Then** the error is printed to stderr and the exit code is 2

#### Scenario: input delivery failure exits 2 and skips evaluation
- **Given** a document whose route input delivery fails (for example a bean processor returning an error with no error handler configured)
- **When** `camel test <doc>` runs
- **Then** the failure is reported, no endpoint lines are printed for that document, and the exit code is 2

#### Scenario: scenario receive timeout exits 1

- **Given** a scenario `receive` action whose partner never sends
- **When** the deadline elapses
- **Then** the action line reports `receive-timeout` and the exit code is 1

#### Scenario: tier annotation appears per document

- **Given** a run of one lean and one full document
- **When** `camel test` executes both
- **Then** each document's output line carries its derived tier annotation

#### Scenario: full-derived unit document annotates full* with advisory

- **Given** a unit-tier document whose route file contains a `wasm:` step
- **When** `camel test <doc>` runs
- **Then** the document's output line carries the `full*` annotation, stderr carries one advisory naming the lean registry (direct, log, mock, seda, timer) and the scenario-tier alternative, and the lean registry is unchanged

#### Scenario: lean registry miss is actionable

- **Given** a unit-tier document whose route file contains a `wasm:` step
- **When** `camel test <doc>` runs and route add fails on the registry miss
- **Then** the reported failure names the missing component AND the lean registry and points to the scenario document vocabulary — never a bare `Component not found: wasm`

#### Scenario: apparatus failure keeps precedence over verdict failure

- **Given** a first document with a failing expectation and a second document failing with `partner-bind-failure`
- **When** `camel test a.test.yaml b.test.yaml` runs
- **Then** both failures are reported and the exit code is 2

#### Scenario: parse-error doc shows in the summary

- **Given** one document whose expectations pass and one document that fails parsing
- **When** `camel test a.test.yaml bad.test.yaml` runs
- **Then** stdout ends with a summary `1 passed, 0 failed, 1 parse-error doc (skipped)`, stderr names `bad.test.yaml` on the line immediately before the summary, and the exit code is 2

#### Scenario: parse-error doc only

- **Given** a run whose only document fails parsing
- **When** `camel test bad.test.yaml` runs
- **Then** stdout ends with `0 passed, 0 failed, 1 parse-error doc (skipped)` and the exit code is 2

#### Scenario: clean run carries no parse-error segment

- **Given** a run where every document parses and every expectation passes
- **When** `camel test a.test.yaml b.test.yaml` runs
- **Then** the summary reads `N passed, 0 failed` with no parse-error segment, and stderr carries no naming line

#### Scenario: settle timeout reports not run cases

- **Given** a passing unit document with 2 declared cases and a unit document with 8 declared cases whose settle phase times out
- **When** `camel test a.test.yaml b.test.yaml` runs
- **Then** the settle document keeps its one failed `<settle>` row, the summary reads `2 passed, 1 failed, 7 not run — 10 cases`, and the exit code is 1

#### Scenario: document error reports not run cases

- **Given** a passing unit document with 2 declared cases and a unit document with 3 declared cases whose input delivery fails
- **When** `camel test a.test.yaml b.test.yaml` runs
- **Then** the summary reads `2 passed, 0 failed, 3 not run, 1 parse-error doc (skipped) — 5 cases in 2 docs`, and the exit code is 2

#### Scenario: scenario early stop reports not run actions

- **Given** a scenario document with 4 declared actions where actions 0 and 1 pass and action 2 fails
- **When** the document runs
- **Then** action 3 produces no verdict row, the summary counts `2 passed, 1 failed, 1 not run — 4 cases`, and the exit code is 1

#### Scenario: clean run prints no not run segment

- **Given** a run where every declared case of every document produces a verdict row
- **When** the run completes
- **Then** the summary carries no `not run` segment and no `cases` trailer and is byte-identical to the previous format

### Requirement: JUnit XML report

`camel test --junit <FILE>` SHALL write a JUnit-format XML report after
the run. The report SHALL contain one `testsuite` per attempted document
(named by the document path as displayed in stdout) and one `testcase` per
assertion row, using the same row labels as the `PASS`/`FAIL` lines
(endpoint name, `reply[i] <to>` reply label, `<settle>`). A failing row
SHALL carry a `<failure>` element; a document-level error (unreadable
file, parse error, boot failure, route load failure, input delivery
failure) SHALL appear as one `<error>` testcase named `<document>` in
that document's suite. `failure` and `error` elements SHALL carry a
`message` attribute holding the first line of the detail text and element
text holding the full detail. An expansion-level error (unreadable
directory entry, zero-document directory) SHALL appear as one synthetic
`testsuite` named by the path in the error message with a single `<error>`
testcase named `<expansion>`. Each declared case of a parsed document that
produced no verdict row SHALL appear as one `<testcase>` named by the row
label a successful run would print, with the document path as the
`classname` attribute, carrying a `<skipped>` element whose
`message` attribute starts with `not run:`. The message SHALL read
`not run: document error` when the document failed at the document level,
`not run: settle timeout` when the suite holds a failed `<settle>` row, and
`not run: stopped after scenario[i] failure` when a scenario action failed
and later declared actions produced no row. A document that failed to read
or parse SHALL contribute no skipped testcases. Attribute counts on both
the `testsuites` and per-suite levels SHALL follow exact formulas: `tests`
= testcase count = `passed + failed + errors + skipped`, `failures` =
failing rows, `errors` = error testcases, `skipped` = skipped testcases.
The `skipped` attribute SHALL appear on a `testsuite` and on `testsuites`
only when that level holds at least one skipped testcase, and SHALL be
omitted otherwise. A fully evaluated document's suite SHALL be byte-identical
to the previous format. When the flag validates, the report SHALL be
written on exit-0, exit-1, and exit-2 runs alike; an invalid filter flag
(before validation completes) SHALL produce no report. A report write
failure SHALL print to stderr and exit 2. Text SHALL be XML-escaped, and
characters XML 1.0 forbids in content SHALL be removed. When the flag is
absent, no report is written and behavior is unchanged.

#### Scenario: all-pass report
- **Given** a document whose expectations all hold
- **When** `camel test doc.test.yaml --junit r.xml` runs
- **Then** `r.xml` holds one suite with a testcase per endpoint and reply row, `failures="0"`, `errors="0"`, and the exit code is 0

#### Scenario: failure detail lands in the report
- **Given** a document with one failing expectation
- **When** `camel test doc.test.yaml --junit r.xml` runs
- **Then** the failing row's testcase carries a `<failure>` containing the mismatch detail text, the exit code is 1, and the report is written

#### Scenario: document error lands in the report
- **Given** a run where one document fails to parse and another passes
- **When** `camel test a.test.yaml bad.test.yaml --junit r.xml` runs
- **Then** `r.xml` holds the passing suite plus an `errors="1"` suite for the broken document with one `<error>` testcase named `<document>` and no skipped testcases, and the exit code is 2

#### Scenario: XML-significant characters are escaped
- **Given** a document whose settle phase times out (row label `<settle>`) and whose detail contains `<`, `&`, a quote, and a control character
- **When** the report is written
- **Then** the output is well-formed XML with `<`, `&`, and the quote escaped and the control character removed, and the testcase name renders the `<settle>` label escaped

#### Scenario: invalid filter flag writes no report
- **Given** `--junit r.xml` combined with an invalid glob in `--filter-file`
- **When** `camel test . --junit r.xml --filter-file '['` runs
- **Then** the pattern error is printed to stderr, no document runs, `r.xml` is not created, and the exit code is 2

#### Scenario: no flag writes nothing
- **Given** any run without `--junit`
- **When** `camel test doc.test.yaml` runs
- **Then** no report file is created and stdout matches the flag-less behavior

#### Scenario: settle timeout produces skipped testcases

- **Given** a unit document with 8 declared cases whose settle phase times out
- **When** the report is written
- **Then** the suite holds the failed `<settle>` testcase plus 8 skipped testcases named by the declared labels with message `not run: settle timeout`, and the attributes read `tests="9"`, `failures="1"`, `errors="0"`, `skipped="8"`

#### Scenario: document error produces skipped testcases and one error

- **Given** a unit document with 3 declared cases whose boot fails
- **When** the report is written
- **Then** the suite holds one `<error>` testcase named `<document>` plus 3 skipped testcases with message `not run: document error`, and the attributes read `tests="4"`, `errors="1"`, `skipped="3"`

#### Scenario: early stop produces skipped testcases for unreached actions

- **Given** a scenario document with 4 declared actions where action 2 fails and actions 0 and 1 pass
- **When** the report is written
- **Then** the suite holds the 3 executed action rows plus one skipped testcase for action 3 with message `not run: stopped after scenario[2] failure`, and the attributes read `tests="4"`, `failures="1"`, `skipped="1"`

#### Scenario: fully evaluated suite carries no skipped testcases

- **Given** a document whose declared cases all produce verdict rows
- **When** the report is written
- **Then** the suite holds no `<skipped>` elements, carries no `skipped` attribute, and is byte-identical to the previous format
