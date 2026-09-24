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
annotation (`lean` or `full`) per document, and a final `N passed, M failed`
summary. A `shutdown-failure` after a recorded verdict SHALL NOT mask the
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
no parse-class failure occurred, the summary SHALL read exactly
`N passed, M failed` and stderr SHALL carry no naming line.

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
