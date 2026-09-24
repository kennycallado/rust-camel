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
