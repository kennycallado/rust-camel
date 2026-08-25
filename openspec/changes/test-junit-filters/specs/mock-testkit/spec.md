# Delta: mock-testkit

## ADDED Requirements

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
testcase named `<expansion>`. Attribute counts on both the `testsuites`
and per-suite levels SHALL follow exact formulas: `tests` = testcase
count (`passed + failed + errors` against the stdout summary counters),
`failures` = failing rows, `errors` = error testcases. When the flag
validates, the report SHALL be written on exit-0, exit-1, and exit-2
runs alike; an invalid filter flag (before validation completes) SHALL
produce no report. A report write failure SHALL print to stderr and exit
2. Text SHALL be XML-escaped, and characters XML 1.0 forbids in content
SHALL be removed. When the flag is absent, no report is written and
behavior is unchanged.

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
- **Then** `r.xml` holds the passing suite plus an `errors="1"` suite for the broken document with one `<error>` testcase named `<document>`, and the exit code is 2

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

### Requirement: Document filters

`camel test --filter-file <GLOB>` (repeatable) SHALL narrow the expanded
document set to documents whose entire displayed-path string matches at
least one glob (`glob`-crate semantics: `*` does not cross `/`; `**`
does); the match happens before reading, so file-filtered-out documents
are never read or parsed. `camel test --filter-endpoint <NAME>`
(repeatable) SHALL narrow the set to file-admitted documents whose
`expects` map contains at least one of the given names, evaluated after
parsing; a file-admitted document that fails to parse SHALL still report
its error and set the exit code to 2 regardless of the endpoint filter,
and SHALL count as a survivor. When both kinds are given, both SHALL
apply (AND); repeats of one kind are OR. When at least one filter is
given and no document survives, `camel test` SHALL report a misuse error
naming the filters and exit 2. An invalid glob pattern SHALL exit 2
before any document runs. Filtered-out documents SHALL produce no stdout
lines, no summary counts, and no JUnit rows. Survivors' stdout SHALL be
identical to running them directly.

#### Scenario: file glob narrows the run
- **Given** two documents `a.test.yaml` and `sub/b.test.yaml`
- **When** `camel test a.test.yaml sub/b.test.yaml --filter-file '*.test.yaml'` runs
- **Then** only `a.test.yaml` runs, and `sub/b.test.yaml` produces no lines or counts

#### Scenario: file filter applies before reading
- **Given** a document excluded by `--filter-file` that also fails to parse
- **When** `camel test bad.test.yaml --filter-file 'other*'` runs
- **Then** the document is skipped without a read attempt and the exit code is 2 only from the zero-survivors misuse error

#### Scenario: endpoint filter matches expects keys
- **Given** two parseable documents where only one declares `expects: {orders: ...}`
- **When** `camel test . --filter-endpoint orders` runs
- **Then** only the document declaring `orders` runs and the other is skipped silently

#### Scenario: filters compose AND across kinds
- **Given** documents matching the glob and documents declaring the endpoint, overlapping only in one document
- **When** `camel test . --filter-file './sub/**' --filter-endpoint orders` runs
- **Then** only the overlap document runs

#### Scenario: zero survivors is misuse
- **Given** a filter set matching no expanded document
- **When** `camel test . --filter-endpoint nosuch` runs
- **Then** a misuse error naming the filter is printed to stderr and the exit code is 2

#### Scenario: invalid glob is misuse
- **Given** a glob pattern `glob::Pattern` rejects
- **When** `camel test . --filter-file '['` runs
- **Then** the pattern error is printed to stderr, no document runs, and the exit code is 2

#### Scenario: parse error still surfaces under endpoint filter
- **Given** a broken document and a filter-endpoint that would exclude it had it parsed
- **When** `camel test . --filter-endpoint orders` runs
- **Then** the broken document's parse error is reported to stderr and the exit code is 2
