## MODIFIED Requirements

### Requirement: Declarative test document parsing

`camel test` SHALL accept one or more test documents (reserved suffix
predicate `camel_dsl::discovery::is_test_document`: `*.test.yaml`, with
`*.test.yml` as an alias of the same format). A test
document SHALL contain exactly one route source: `routeFiles` (paths to
route YAML files, resolved relative to the test document's directory),
`routeFilesFromRoot` (paths resolved against the nearest ancestor
`Camel.toml` directory), or inline `routes` (same schema as route files).
A document SHALL contain exactly one vocabulary: either the unit-tier
vocabulary (`inputs`, `expects`, `intercepts`) or the integration-tier
vocabulary (`scenario`, per the integration-tier spec). A document mixing
both vocabularies SHALL be rejected as `doc-validation`.
A unit-tier document MAY contain optional `inputs` (each input MAY declare an optional
`expectReply` block, see Reply capture and assertion) and SHALL contain a
mandatory non-empty `expects` map keyed by mock endpoint name — except that
a unit-tier document MAY omit `expects` (or leave it empty) when at least one input
declares `expectReply`; a unit-tier document with neither endpoint expectations nor any
`expectReply` SHALL be rejected. A unit-tier document MAY contain an
optional `intercepts` map keyed by real endpoint URI (any scheme except `mock:`),
where each key is used verbatim (no trimming or normalization; query
parameters are significant and participate in exact matching) and each value
carries exactly one action: `skipTo` (a `mock:` URI with a non-empty
endpoint path that replaces the send before component resolution) or
`divertCopyTo` (a `mock:` URI with a non-empty endpoint path that receives
a copy while the real send continues). Documents declaring
two or three route sources, or none, SHALL be rejected. Unknown fields
SHALL be rejected, including inside intercept action objects. A scenario
document MAY contain an optional `env` map and an optional ambient
passthrough allowlist, per the integration-tier spec.

Matcher grammar. Body matcher keys: `equals`, `regex`, `contains`,
`startsWith`, `endsWith`, `exists`, `jsonSubset`. Header matcher keys:
`equals`, `regex`, `exists`. `exists` takes no argument (a `null` value).
`jsonSubset` SHALL be an object mapping and applies to bodies only; it is
a document error on a header value. An object whose sole key is
`predicate` SHALL be rejected with a message stating predicate matchers
are not supported, in every grammar position; in dual-grammar positions
(header values and `expectReply.body`), an object with multiple keys that
merely contains a `predicate` key is a literal value and parses
unchanged (in strict `bodies` entries, multi-key maps are rejected as
usual). Matchers SHALL be validated at
parse time: `regex` patterns SHALL compile and malformed values SHALL fail
parsing with exit code 2 naming the document field and the offending key.

Matcher positions. `expects.bodies` list entries SHALL use strict matcher
syntax: a bare string is `equals` (exactly as before this change); a map
with exactly one recognized body matcher key is that matcher; any other
scalar, a bare array, or a map with zero, multiple, or unrecognized keys
is a document error (exit 2) stating body entries must be strings or
matcher maps, naming the key where present.

Header values (`expects.headers` and `expectReply.headers`) accepted any
JSON value before this change and SHALL keep that behavior: a bare scalar
or array, or an object that is not a single-recognized-key matcher map,
is a literal `equals` value compared structurally; a map whose sole key
is a recognized header matcher key (`equals`, `regex`, `exists`) is that
matcher. A sole `jsonSubset` key on a header value is a document error
stating `jsonSubset` applies to bodies only, and a sole `predicate` key
is rejected with the reserved-key message.

`expectReply.body` accepted strings and JSON structures before this
change, so it keeps both: every bare scalar (string, number, boolean,
null) and every array is a literal `equals` value; a JSON object with
exactly one recognized matcher key is that matcher; a JSON object whose
sole key is `predicate` is
rejected (exit 2); any other JSON object is a literal `equals` value
(structural equality), preserving the existing JSON-reply behavior.

#### Scenario: valid document with reference and expectations

- **Given** a test document with `routeFiles: [config/routes.yaml]` and `expects: {mock:result: {count: 3}}`
- **When** `camel test` parses the document
- **Then** parsing succeeds, `config/routes.yaml` resolves relative to the test document's directory, and one expectation set exists for endpoint `result`

#### Scenario: unknown field rejected

- **Given** a test document containing an unknown top-level field
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 and a message naming the unknown field

#### Scenario: empty expects rejected

- **Given** a test document with `expects: {}` or no `expects` key and no input declaring `expectReply`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating that `expects` is mandatory unless an input declares `expectReply`

#### Scenario: both routeFiles and routes rejected

- **Given** a test document containing both `routeFiles` and `routes`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating the route source keys are mutually exclusive and naming `routeFiles` and `routes`

#### Scenario: routeFilesFromRoot combined with another source rejected

- **Given** a test document containing `routeFilesFromRoot` together with `routeFiles`, with `routes`, or with both of the other keys
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating the route source keys are mutually exclusive and naming every present key

#### Scenario: no route source rejected

- **Given** a test document containing none of `routeFiles`, `routeFilesFromRoot`, or `routes`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating exactly one route source is required

#### Scenario: unsupported body scalar rejected

- **Given** an input whose `body` is a null, boolean, or number scalar
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating only string, object, and array bodies are supported

#### Scenario: settle out of range rejected

- **Given** a test document with `settle: 0ms` or `settle: 10s`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating `settle` must satisfy `0 < settle <= 5s`

#### Scenario: intercepts with skipTo accepted

- **Given** a test document with `intercepts: {kafka:orders: {skipTo: mock:orders}}`
- **When** `camel test` parses the document
- **Then** parsing succeeds and one intercept rule exists mapping `kafka:orders` to a skip action targeting `mock:orders`

#### Scenario: intercepts with divertCopyTo accepted

- **Given** a test document with `intercepts: {seda:audit: {divertCopyTo: mock:audit}}`
- **When** `camel test` parses the document
- **Then** parsing succeeds and one intercept rule exists mapping `seda:audit` to a divert action targeting `mock:audit`

#### Scenario: intercept action with both keys rejected

- **Given** a test document with an intercept action containing both `skipTo` and `divertCopyTo`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating exactly one of `skipTo` or `divertCopyTo` is required

#### Scenario: intercept action with neither key rejected

- **Given** a test document with an empty intercept action `{}`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating exactly one of `skipTo` or `divertCopyTo` is required

#### Scenario: non-mock intercept target rejected

- **Given** a test document with `intercepts: {kafka:orders: {skipTo: direct:orders}}`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 naming the offending rule and target and stating intercept targets must start with `mock:`

#### Scenario: mock intercept source rejected

- **Given** a test document with `intercepts: {mock:a: {skipTo: mock:b}}`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 naming the offending rule and stating intercept sources must not use `mock:`

#### Scenario: empty intercept source URI rejected

- **Given** a test document with `intercepts: {"": {skipTo: mock:orders}}`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 naming the offending rule and stating the intercept source URI must not be empty

#### Scenario: intercept target with empty endpoint path rejected

- **Given** a test document with `intercepts: {kafka:orders: {skipTo: "mock:"}}`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 naming the offending rule and stating the intercept target needs a mock endpoint name

#### Scenario: query parameters on an intercept source are significant

- **Given** a route sending to `kafka:orders?x=1` and a document whose only intercept key is `kafka:orders`
- **When** `camel test` executes the document
- **Then** no rule matches the send (exact URI matching), and the document fails at route load with an error naming `kafka` as unresolvable

#### Scenario: unknown intercept action field rejected

- **Given** a test document with an intercept action containing an unknown field such as `replaceWith`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 and a message naming the unknown field

#### Scenario: bare string body stays exact match

- **Given** a test document with `expects: {mock:result: {count: 1, bodies: ["plain"]}}`
- **When** `camel test` parses and executes the document against a body of `plain`
- **Then** parsing succeeds without matcher interpretation and the expectation passes exactly as before this change

#### Scenario: matcher map body accepted

- **Given** a test document with `expects: {mock:result: {count: 1, bodies: [{regex: "^order-[0-9]+$"}]}}`
- **When** `camel test` parses the document
- **Then** parsing succeeds and one body expectation carries a regex matcher

#### Scenario: matcher map header accepted

- **Given** a test document with `expects: {mock:result: {count: 1, headers: {X-Trace: {regex: "^[a-f0-9]{8}$"}}}}`
- **When** `camel test` parses the document
- **Then** parsing succeeds and the header expectation carries a regex matcher

#### Scenario: unknown matcher key rejected

- **Given** a test document with `expects: {mock:result: {count: 1, bodies: [{xpath: "//id"}]}}`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 naming the field (`bodies`) and the unrecognized key (`xpath`)

#### Scenario: reserved predicate key rejected

- **Given** a test document whose body expectation uses the sole key `predicate`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating predicate matchers are not supported

#### Scenario: multi-key object containing predicate stays literal

- **Given** a header value `{predicate: "raw", mode: "batch"}` (multiple keys)
- **When** `camel test` parses and executes the document against a received header equal to that object
- **Then** parsing succeeds without matcher interpretation and the header expectation passes by structural equality

#### Scenario: matcher map with wrong key count rejected

- **Given** a test document with a body expectation `{}` or `{regex: "a", contains: "b"}`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating a matcher map must have exactly one key

#### Scenario: invalid regex rejected at parse

- **Given** a test document with a body expectation `{regex: "(unclosed"}`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 naming the field and the regex error

#### Scenario: jsonSubset with non-object rejected at parse

- **Given** a test document with a body expectation `{jsonSubset: [1, 2]}`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating `jsonSubset` must be an object

#### Scenario: jsonSubset on a header value rejected at parse

- **Given** a test document with a header expectation `{jsonSubset: {a: 1}}`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating `jsonSubset` applies to bodies only

#### Scenario: reply body object without matcher keys stays literal JSON

- **Given** an input with `expectReply: {body: {"status": "ok"}}` (single unrecognized key)
- **When** `camel test` parses and executes the document against the JSON reply body `{"status": "ok"}`
- **Then** parsing succeeds without matcher interpretation and the reply assertion passes by structural equality

#### Scenario: mixed vocabulary rejected at load

- **Given** a document declaring both `scenario:` and `inputs:`
- **When** `camel test` parses the document
- **Then** the run reports `doc-validation` and exits 2 before any boot

#### Scenario: scenario document with unit-tier fields rejected

- **Given** a document declaring `scenario:` and `expects:`
- **When** `camel test` parses the document
- **Then** the run reports `doc-validation` and exits 2

#### Scenario: scenario-only document with env accepted

- **Given** a scenario document declaring `scenario:` and an `env` map
- **When** `camel test` parses the document
- **Then** parsing succeeds under the integration-tier vocabulary


### Requirement: In-process route execution

`camel test` SHALL boot one `CamelContext` in-process per document. LEAN
documents (per the integration-tier tier derivation) boot the lean
registration: the real mock component (plus direct, timer, log, seda), with
routes loaded through the same per-file YAML parser `camel run` uses, and
SHALL NOT start WASM plugins, file-watch, or network servers, and SHALL NOT
load user beans (including WASM or native beans); they MAY register built-in
in-process stub beans declared in the test document's `beans:` block. Route
execution SHALL involve no IPC and no RuntimeBus/QueryBus traffic. FULL
documents whose scenario endpoints declare any real wire scheme boot
through the shared `camel-bundles` cascade in-process, driven
by the document's nearest `Camel.toml`; FULL documents whose scenario
endpoints are all in-memory (`fake:` scheme) run the in-memory smoke
path in every build — they exercise document shape and action grammar,
not the system under test, so no boot adds information. The harness MAY bind partner-side
loopback listeners on `127.0.0.1` for FULL documents, which is traffic under
test, not a control channel. No document tier ever drives a separately
deployed `camel run` process.

#### Scenario: routes run in-process
- **Given** a test document referencing a route file with `from: direct:start` → `to: mock:result`
- **When** `camel test` executes the document with an input `{to: direct:start, body: "x"}`
- **Then** the exchange reaches the in-process mock endpoint and `mock:result` records body `x`

#### Scenario: self-starting route without inputs
- **Given** a test document whose route uses `timer:tick?period=50&repeatCount=3` → `to: mock:result` and `expects: {mock:result: {count: 3}}`
- **When** `camel test` executes the document with no `inputs`
- **Then** the timer drives 3 exchanges and the count expectation is evaluated against them

#### Scenario: full document boots the shared cascade in-process

- **Given** a scenario document whose routes need the http bundle
- **When** `camel test` executes it
- **Then** the boot registers bundles through `camel-bundles` from the nearest `Camel.toml`, in-process, and any harness partner listener binds on loopback


### Requirement: Exit codes, reporting, and multi-document execution

`camel test` SHALL execute documents in CLI argument order, sequentially. A
document-level error (unreadable file, parse error, boot failure, or input
delivery failure such as a processor error propagating out of the route)
SHALL be reported and execution SHALL continue with the next document; a
document whose input delivery failed SHALL skip settling, endpoint
evaluation, and reply evaluation for that document. Exit codes: 0 when every
expectation of every document passes; 1 when any expectation (endpoint or
reply) fails, a settle timeout occurs, or a scenario verdict fails
(`receive-timeout`, `validation-mismatch`, runtime `scenario-var-unresolved`);
2 for misuse, unreadable files, document/route parse errors, input delivery
failures, and apparatus failures (`doc-validation`, `tier-filter-collision`,
`partner-bind-failure`, `partner-startup-failure`, `action-transport-failure`,
`infra-unavailable`, `full-boot-failure`, `shutdown-failure`). When classes
coexist, precedence is 2 > 1 > 0. stdout SHALL carry one `PASS`/`FAIL` line
per endpoint per document, one `PASS`/`FAIL` line per asserted reply per
document, one line per scenario action verdict for scenario documents, a tier
annotation (`lean` or `full`) per document, and a final `N passed, M failed`
summary. A `shutdown-failure` after a recorded verdict SHALL NOT mask the
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

#### Scenario: apparatus failure keeps precedence over verdict failure

- **Given** a first document with a failing expectation and a second document failing with `partner-bind-failure`
- **When** `camel test a.test.yaml b.test.yaml` runs
- **Then** both failures are reported and the exit code is 2


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
and SHALL count as a survivor. `camel test --unit` and `camel test
--integration` SHALL narrow the set by derived tier, symmetrically: a
nonmatching document admitted through directory expansion is excluded
silently, while a nonmatching document named explicitly as a CLI argument
fails with `tier-filter-collision` and exit 2; supplying both flags is misuse
with exit 2. With no filter flags, every expanded document runs at its
derived tier. When filters of different kinds are given, all SHALL apply
(AND); repeats of one kind are OR. When at least one filter is
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

#### Scenario: unit filter excludes full documents silently

- **Given** a directory holding lean and full documents
- **When** `camel test . --unit` runs
- **Then** only lean documents run and no error is reported for the excluded full documents

#### Scenario: explicitly named full document collides under unit filter

- **Given** a document deriving FULL
- **When** `camel test doc.test.yaml --unit` runs with that explicit path
- **Then** the run reports `tier-filter-collision` and exits 2

#### Scenario: both tier flags are misuse

- **Given** any document set
- **When** `camel test . --unit --integration` runs
- **Then** the run exits 2 without reading any document

#### Scenario: tier filter composes with file filter

- **Given** lean and full documents where one lean document matches a glob
- **When** `camel test . --unit --filter-file 'sub/**'` runs
- **Then** only the lean glob-matching document runs

#### Scenario: no filter runs everything at derived tier

- **Given** a directory holding lean and full documents
- **When** `camel test .` runs
- **Then** every document executes at its derived tier

