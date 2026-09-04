# mock-testkit Specification

## Purpose
TBD - created by archiving change mock-declarative-testkit. Update Purpose after archive.
## Requirements
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

### Requirement: Input delivery restriction

`inputs.to` SHALL accept only `direct:` endpoints in this change. Any other
scheme SHALL be a document error.

#### Scenario: non-direct input target rejected
- **Given** a test document with `inputs: [{to: seda:queue, body: "x"}]`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating only `direct:` input targets are supported

### Requirement: Expectation evaluation via change #1 API

For each entry in `expects`, the runner SHALL obtain the endpoint via
`MockComponent::get_endpoint` (name = URI suffix after `mock:`), map fields to
change #1 setters (`count` → `expect_count`, `minCount` →
`expect_minimum_count`, `bodies` → `expect_body_matcher` (bare strings as
`equals`, matcher maps as their matcher), `headers` →
`expect_header_matcher` (literal values as `equals`, matcher maps as
their matcher)), and evaluate with `try_assert_satisfied()`. `count`
and `minCount` in the same entry SHALL be a document error (exit 2).
Assertion failures SHALL be reported without aborting remaining endpoints.
Header regex matchers SHALL reach the existing header-regex engine; matcher
mismatches SHALL be assertion failures (exit 1) whose error text names the
matcher kind, its pattern or value, and the received body or header values.

#### Scenario: body and count expectations pass

- **Given** a running document with `expects: {mock:result: {count: 2, bodies: ["a", "b"]}}` and inputs producing exactly those bodies in order
- **When** evaluation runs
- **Then** the endpoint reports PASS and the summary counts it as passed

#### Scenario: mismatch reports change #1 error detail

- **Given** `expects: {mock:result: {count: 3}}` with only 2 exchanges received
- **When** evaluation runs
- **Then** the endpoint reports FAIL with the `MockAssertionError` text containing "expected 3 exchanges, got 2", remaining endpoints still evaluate, and the process exits 1

#### Scenario: count and minCount together rejected

- **Given** a document entry containing both `count` and `minCount`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating they are mutually exclusive

#### Scenario: unknown mock endpoint fails the document

- **Given** `expects: {mock:ghost: {count: 1}}` where no route creates `mock:ghost`
- **When** evaluation runs
- **Then** the endpoint reports FAIL with a message naming `ghost` as absent, and the process exits 1

#### Scenario: regex body matcher passes on nondeterministic content

- **Given** a document whose route emits a body matching `^order-[0-9]+$` and `expects: {mock:result: {count: 1, bodies: [{regex: "^order-[0-9]+$"}]}}`
- **When** evaluation runs
- **Then** the endpoint reports PASS

#### Scenario: body matcher mismatch names the matcher

- **Given** a route emitting `total: 12` and `expects: {mock:result: {count: 1, bodies: [{contains: "total: 13"}]}}`
- **When** evaluation runs
- **Then** the endpoint reports FAIL with text naming the `contains` matcher, its value, and the received body, and the process exits 1

#### Scenario: header regex matcher evaluated

- **Given** a route emitting header `X-Trace: ab12cd34` and `expects: {mock:result: {count: 1, headers: {X-Trace: {regex: "^[a-f0-9]{8}$"}}}}`
- **When** evaluation runs
- **Then** the endpoint reports PASS

#### Scenario: jsonSubset partial body match passes

- **Given** a route emitting the JSON body `{"id": 7, "status": "ok", "meta": {"ts": "...", "seq": 3}}` and `expects: {mock:result: {count: 1, bodies: [{jsonSubset: {status: "ok", meta: {seq: 3}}}]}}`
- **When** evaluation runs
- **Then** the endpoint reports PASS (unmatched fields are ignored; nested subset matches)

### Requirement: Settling before assertion

The runner SHALL settle traffic before evaluating: a document-wide settle
deadline starts when route execution begins and equals one full quiet window
plus a 5-second instability budget (so any valid `settle` value can always
satisfy its own window); all expected endpoints' `received_count` SHALL be
sampled simultaneously every 50ms; the quiet window (default 250ms, `settle:`
override) must elapse with no sampled change (any change resets the window).
Count values above expectations do NOT end settling — only quiescence does.
Hitting the deadline without stability SHALL fail the document with a
settle-timeout message (exit 1), never hang.

#### Scenario: timer route settles before assertion
- **Given** a timer-driven route emitting 3 exchanges and `expects: {mock:result: {count: 3}}`
- **When** the counts are stable for the quiet window within the deadline
- **Then** evaluation proceeds and passes

#### Scenario: unstable traffic hits the deadline
- **Given** a route still emitting when the settle deadline (quiet window + 5-second budget) is reached
- **When** the deadline hits
- **Then** the document fails with a settle-timeout message and exit code 1

#### Scenario: count change resets the quiet window
- **Given** an endpoint whose `received_count` changes 100ms into a 250ms quiet window
- **When** the next sample is taken
- **Then** the quiet window restarts from that sample and evaluation waits for a full stable window or the deadline

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

### Requirement: camel run non-interference

Route YAML files SHALL remain unchanged by this feature: `camel run`'s route
discovery (initial load and watch reload) SHALL skip files whose names end
in `.test.yaml` or `.test.yml` on every pattern path, and test documents
SHALL NOT alter route file schema or runtime behavior. This replaces the
previous contract that honored an explicit `*.test.yaml` glob as a user
override: it is an accepted breaking change (route-shaped files using the
reserved suffix must be renamed). An explicit no-wildcard pattern naming a
test-suffixed file SHALL fail with the reserved-suffix error (see the dsl
specification, "Reserved test suffix in route discovery"). A wildcard
pattern that matches only test documents SHALL yield no routes from those
files.

#### Scenario: run ignores test documents

- **Given** a routes directory containing `routes/demo.yaml` and `routes/demo.test.yaml`
- **When** `camel run` discovers routes with default globs
- **Then** only `demo.yaml` loads as a route and no error occurs

#### Scenario: explicit Camel.toml routes entry ignores test documents

- **Given** `Camel.toml` with explicit `routes = ["routes/*.yaml"]` and a colocated `routes/demo.test.yaml`
- **When** `camel run` starts
- **Then** the test document is skipped, startup succeeds, and a watch reload triggered by saving the test document is a no-op

#### Scenario: explicit glob override honored

- **Given** a user invoking `camel run --routes 'routes/*.test.yaml'`
- **When** discovery runs
- **Then** the explicit glob is processed and every matched test document is skipped (never parsed as a route document); when no other routes match, the command reports that no routes were found

#### Scenario: explicit file naming errors instead of parsing

- **Given** a user invoking `camel run --routes routes/demo.test.yaml`
- **When** discovery runs
- **Then** the run fails with the reserved-suffix error directing the user to `camel test`; the document is never parsed as a route document

### Requirement: Root-anchored route file references

A test document MAY declare `routeFilesFromRoot`, a list of route file paths
resolved against the project root. The project root SHALL be the nearest
ancestor directory of the test document that contains a `Camel.toml` file,
located by walking up from the test document's directory. Resolution SHALL
be independent of the process working directory. When no ancestor directory
contains `Camel.toml`, the document run SHALL fail with exit code 2 and a
`NoProjectRoot` error that names the test document's directory.

#### Scenario: nested test document resolves from project root

- **GIVEN** a project with `Camel.toml` at its root, a route file `routes/orders.yaml`, and a test document at `tests/integration/orders.test.yaml` containing `routeFilesFromRoot: [routes/orders.yaml]`
- **WHEN** `camel test` runs on that document (given by an absolute or otherwise cwd-valid path) from any working directory
- **THEN** the route file resolves to `<project-root>/routes/orders.yaml` and the document executes

#### Scenario: nearest ancestor Camel.toml wins

- **Given** a monorepo with `Camel.toml` at the repository root and a second `Camel.toml` in `services/orders/`, and a test document at `services/orders/tests/a.test.yaml`
- **When** the document declares `routeFilesFromRoot: [routes/a.yaml]`
- **Then** the path resolves against `services/orders/`, the nearest ancestor

#### Scenario: no ancestor Camel.toml rejected

- **Given** a test document in a directory tree with no `Camel.toml` in any ancestor
- **When** `camel test` runs the document with `routeFilesFromRoot` declared
- **Then** the run fails with exit code 2 and a `NoProjectRoot` error naming the test document's directory

### Requirement: Directory argument expansion

`camel test` SHALL accept directory arguments alongside file
arguments. Each directory argument SHALL expand to the test documents
(`*.test.yaml` / `*.test.yml`, per the reserved-suffix predicate
`camel_dsl::discovery::is_test_document`) contained in it, discovered
recursively. Directory names `target`, `.git`, and `node_modules`
SHALL be skipped at any depth. Within one directory argument the
expanded documents SHALL be ordered by byte-sorted path; across
arguments, CLI argument order SHALL be preserved. A file reached more
than once (explicit argument plus directory expansion) SHALL run once,
at its first occurrence. Plain file arguments SHALL keep their existing
verbatim behavior. A directory argument that expands to zero test
documents SHALL be reported as a misuse error (exit 2 class) naming
the directory, and the remaining arguments SHALL still run.

#### Scenario: directory expands recursively to sorted test documents

- **GIVEN** a directory containing `b.test.yaml`, `a.test.yaml`, and a nested subdirectory containing `c.test.yml`
- **WHEN** `camel test <dir>` runs
- **THEN** the documents run in the order `a.test.yaml`, `b.test.yaml`, `c.test.yml` (byte-sorted within the directory argument)

#### Scenario: excluded directory names are skipped

- **GIVEN** a directory whose `target/` subdirectory contains `gen.test.yaml`
- **WHEN** `camel test <dir>` runs
- **THEN** `gen.test.yaml` does not run

#### Scenario: zero-document directory is a misuse error

- **GIVEN** a directory containing no test documents
- **WHEN** `camel test <empty-dir> <other.test.yaml>` runs
- **THEN** an error naming `<empty-dir>` is reported, `<other.test.yaml>` still runs, and the exit code is 2

#### Scenario: duplicate documents run once

- **GIVEN** a directory containing `a.test.yaml` and the invocation `camel test <dir> <dir>/a.test.yaml`
- **WHEN** the run completes
- **THEN** `a.test.yaml` runs exactly once

#### Scenario: plain file arguments unchanged

- **GIVEN** an explicit file argument with any name
- **WHEN** `camel test <file>` runs
- **THEN** the file is read and parsed exactly as before this requirement (no expansion, no suffix filtering)

### Requirement: Declarative intercept application

`parse_test_document` SHALL construct the Stage A `InterceptRules` from the
document's `intercepts` map and store them on the parsed document. The
runner SHALL apply the stored rules through the camel-core builder surface
before any route registration or start, so the Stage A freeze contract
holds by construction. `skipTo` SHALL replace the send before component
resolution (the real component need not be registered). `divertCopyTo`
SHALL deliver a pre-send copy to the mock while the real send continues
(the real component must be registered in the lean boot set). Intercept
targets and `expects` keys SHALL each resolve to mock endpoint names
(expects keys by parse-time normalization; `mock:` URIs by endpoint path),
so both surfaces address the same endpoint. `camel run` SHALL NOT read the
`intercepts` block.

#### Scenario: skip exercises a route referencing an unregistered component

- **Given** a route `from: direct:start` → `to: kafka:orders` and a document with `intercepts: {kafka:orders: {skipTo: mock:orders}}`, one input to `direct:start`, and `expects: {mock:orders: {count: 1}}`
- **When** `camel test` executes the document
- **Then** the exchange reaches `mock:orders`, the expectation passes, and the process exits 0 without any kafka component registered

#### Scenario: divert copies to the mock while the real endpoint receives traffic

- **Given** a route `from: direct:start` → `to: seda:audit` → `to: mock:sink`, a route `from: seda:audit` → `to: mock:drained`, and a document with `intercepts: {seda:audit: {divertCopyTo: mock:audit}}` plus `expects: {mock:audit: {count: 1}, mock:drained: {count: 1}}`
- **When** `camel test` executes the document with one input
- **Then** the mock copy records the exchange AND the real `seda:audit` queue still delivers to `mock:drained`, both expectations pass

#### Scenario: divert on an unregistered real component fails at route load

- **Given** a route `from: direct:start` → `to: kafka:orders` and a document with `intercepts: {kafka:orders: {divertCopyTo: mock:orders}}`
- **When** `camel test` executes the document
- **Then** route loading fails with an error naming `kafka` as unresolvable, reported as a document error (exit code 2, unchanged failure class)

#### Scenario: intercept target and expects key meet on the same endpoint

- **Given** a document whose intercept target is `mock:orders` and whose `expects` key is `mock:orders`
- **When** evaluation runs
- **Then** the expectation is evaluated against the mock endpoint the intercept targeted: both the target URI and the expects key resolve to endpoint name `orders`

#### Scenario: camel run non-interference for intercepts

- **Given** a project whose `*.test.yaml` declares an `intercepts` block
- **When** `camel run` starts with the project's production routes
- **Then** no interception is applied and production behavior is identical to a project without the block

### Requirement: Declarative bean stubs

`camel test` SHALL support a `beans:` block in the test document mapping bean
names to built-in in-process stub declarations
(`{kind, methods?, config?}`). The runner SHALL register each declared stub in
a bean registry threaded through the context builder before routes load, so
routes containing `bean:` steps compile and run. `kind` SHALL be one of
`echo`, `setBody`, `fail`. Validation SHALL be eager: unknown kinds, invalid
or kind-inappropriate `config`, nested unknown fields, empty `methods` lists,
blank bean names, and blank `methods` entries SHALL be document errors
(exit 2) at parse time. Bean names and `methods` entries SHALL be non-blank.
Method allowlists SHALL be cross-validated against the routes' `bean:` steps
(recursively, including nested sub-pipelines such as circuit-breaker
fallbacks) before boot: when `methods` is omitted the stub SHALL accept every
method the routes invoke on it; when `methods` is declared, every route
invocation outside the list SHALL be a document error (exit 2).

#### Scenario: setBody stub transforms the body
- **Given** a test document with `beans: {enricher: {kind: setBody, config: {body: "stubbed"}}}` and a route `from: direct:start` with steps `bean: {name: enricher, method: enrich}` then `to: mock:out`, and an input `{to: direct:start, body: "x"}`
- **When** `camel test` executes the document
- **Then** the route loads, the stub runs, and `mock:out` records body `stubbed` with count 1

#### Scenario: echo stub passes the exchange through
- **Given** a test document with `beans: {gate: {kind: echo}}` and a route whose `bean: {name: gate, method: anyName}` step precedes `to: mock:out`, with input body `x`
- **When** `camel test` executes the document
- **Then** `mock:out` records body `x` unchanged, count 1

#### Scenario: fail stub surfaces as a document error
- **Given** a test document with `beans: {gate: {kind: fail, config: {message: boom}}}` and a route whose `bean: {name: gate, method: check}` step precedes `to: mock:out`, with one input
- **When** `camel test` executes the document
- **Then** execution fails with exit code 2, the failure output contains `boom`, and `mock:out` receives no exchange (input delivery fails before settling and endpoint evaluation)

#### Scenario: fail stub without message uses the exact default
- **Given** a test document with `beans: {gate: {kind: fail}}` (no `config`) and a route whose `bean: {name: gate, method: check}` step precedes `to: mock:out`, with one input
- **When** `camel test` executes the document
- **Then** execution fails with exit code 2 and the failure output contains exactly `fail bean gate`

#### Scenario: undeclared method is a document error
- **Given** a test document with `beans: {enricher: {kind: echo, methods: [enrich]}}` and a route invoking `bean: {name: enricher, method: transform}`
- **When** `camel test` executes the document
- **Then** execution fails with exit code 2 before the route starts, stating `bean enricher: method transform is not declared`

#### Scenario: omitted methods accepts route invocations
- **Given** a test document with `beans: {gate: {kind: echo}}` (no `methods`) and a route invoking `bean: {name: gate, method: whateverTheRouteUses}`
- **When** `camel test` executes the document
- **Then** the route loads and the stub accepts the invocation (downstream mock records the exchange)

#### Scenario: unknown kind is a document error
- **Given** a test document with `beans: {x: {kind: teleport}}`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 listing the unknown kind and the supported kinds

#### Scenario: setBody without body config is a document error
- **Given** a test document with `beans: {x: {kind: setBody, config: {}}}`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating `body` is required for kind `setBody`

#### Scenario: kind-inappropriate config key is a document error
- **Given** a test document with `beans: {x: {kind: echo, config: {body: y}}}`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating config key `body` is not valid for kind `echo`

#### Scenario: empty methods list is a document error
- **Given** a test document with `beans: {x: {kind: echo, methods: []}}`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating `methods` must be non-empty or omitted

#### Scenario: blank bean name is a document error
- **Given** a test document with `beans: {"  ": {kind: echo}}`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating bean names must be non-blank

#### Scenario: blank methods entry is a document error
- **Given** a test document with `beans: {x: {kind: echo, methods: ["enrich", ""]}}`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating `method names must be non-blank`

#### Scenario: nested unknown field in a bean declaration is a document error
- **Given** a test document with `beans: {x: {kind: echo, metod: [enrich]}}`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 identifying the unknown field

#### Scenario: multiple beans in one document
- **Given** a test document with `beans: {a: {kind: setBody, config: {body: first}}, b: {kind: echo}}` and a route invoking both `bean: {name: a, method: m1}` and `bean: {name: b, method: m2}` before `to: mock:out`, with one input
- **When** `camel test` executes the document
- **Then** both stubs run and `mock:out` records body `first`, count 1

#### Scenario: intercepts and beans compose
- **Given** a test document with both an `intercepts:` block (diverting `kafka:orders` to `mock:orders`) and a `beans:` block used by a second route
- **When** `camel test` executes the document
- **Then** both mechanisms apply independently and both routes' expectations evaluate

#### Scenario: camel run ignores the beans block
- **Given** a `*.test.yaml` file containing an intentionally invalid `beans:` declaration (`beans: {x: {kind: teleport}}`) and a route file loadable by `camel run`
- **When** `camel run` executes the route file
- **Then** `camel run` never reads the test document; neither stdout nor stderr contains the test-document filename, the invalid kind name `teleport` (which exists only in the test document), or the validation fragment `unknown variant`

### Requirement: Reply capture and assertion

`camel test` SHALL capture the reply exchange each `direct:` input's producer
oneshot returns and SHALL support asserting it via an optional per-input
`expectReply: {body?, headers?}` block. Reply `body` SHALL use the dual
grammar of the MODIFIED parsing requirement (all bare scalars — string,
number, boolean, null —, arrays, and non-matcher objects are literal
`equals`; a single-recognized-key object is
that matcher). Reply `headers` values SHALL use the same dual header
grammar as `expects.headers` (literal JSON values stay `equals`; a
sole-key `equals`/`regex`/`exists` map is that matcher). Reply assertions
SHALL be evaluated through the mock component's public
matcher API, not a CLI-private comparison. The reply's message is its
`output` message when the route set one, else its final input message —
body (string or JSON) and/or `headers` (JSON values, matching
`Message.headers`). At least one of `body`/`headers` SHALL be present (an
empty `expectReply` is a document error, exit 2). Replies pair with inputs
by delivery order (delivery is strictly sequential). A failed reply
assertion SHALL be an assertion failure — a `FAIL` line counted into the
summary and exit code 1 — never a document error. Inputs without
`expectReply` SHALL behave exactly as before (reply captured, nothing
asserted). The document-level `expects` relaxation for reply-only documents
is specified in the MODIFIED parsing requirement.

#### Scenario: reply body asserted

- **Given** a test document with a route `from: direct:in` whose steps set the body to `enriched` then `to: mock:out`, and one input `{to: direct:in, body: "x", expectReply: {body: "enriched"}}`
- **When** `camel test` executes the document
- **Then** the reply assertion passes, a PASS line is printed for the reply, and the summary counts it

#### Scenario: reply body mismatch fails with exit 1

- **Given** the same route with an input `expectReply: {body: "wrong"}`
- **When** `camel test` executes the document
- **Then** a FAIL line is printed for the reply, the summary counts one failed, and the exit code is 1

#### Scenario: reply headers asserted

- **Given** a route whose steps set header `stamp` to `yes` then `to: mock:out`, and an input `expectReply: {headers: {stamp: "yes"}}`
- **When** `camel test` executes the document
- **Then** the reply assertion passes

#### Scenario: reply composes with endpoint expectations

- **Given** a document whose route both transforms the body and forwards to `mock:out`, with `expects: {mock:out: {count: 1, bodies: ["enriched"]}}` and `expectReply: {body: "enriched"}` on the input
- **When** `camel test` executes the document
- **Then** both the endpoint expectation and the reply assertion evaluate and both PASS lines print, exit code 0

#### Scenario: absent expectReply keeps behavior

- **Given** a document identical to the body-asserted scenario but without `expectReply`
- **When** `camel test` executes the document
- **Then** no reply line is printed and the result is identical to the pre-change behavior (endpoint expectations only)

#### Scenario: empty expectReply is a document error

- **Given** a test document with an input `expectReply: {}`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating `body` or `headers` must be present

#### Scenario: multiple inputs pair by order

- **Given** a route that appends a per-input marker and two inputs, the first `expectReply: {body: "first-done"}`, the second `expectReply: {body: "second-done"}`
- **When** `camel test` executes the document
- **Then** each reply is matched to its own input's expectation and both pass

#### Scenario: reply-only document

- **Given** a test document with no `expects` block, one route `from: direct:in` whose steps set the body to `done`, and one input `{to: direct:in, body: "x", expectReply: {body: "done"}}`
- **When** `camel test` executes the document
- **Then** the document is valid, the reply assertion passes, and the exit code is 0

#### Scenario: JSON reply body

- **Given** a route whose steps set the body to a JSON document and an input `expectReply: {body: {"status": "ok"}}` (JSON form)
- **When** `camel test` executes the document
- **Then** the reply assertion passes against the JSON body

#### Scenario: bean stub reply

- **Given** a route `from: direct:in` with a `bean: {name: enricher, method: enrich}` step backed by a `beans: {enricher: {kind: setBody, config: {body: enriched}}}` stub, and an input `expectReply: {body: "enriched"}`
- **When** `camel test` executes the document
- **Then** the reply assertion passes against the stub's transformation

#### Scenario: reply body regex matcher passes

- **Given** a route whose steps set the body to `order-42` and an input `expectReply: {body: {regex: "^order-[0-9]+$"}}`
- **When** `camel test` executes the document
- **Then** the reply assertion passes

#### Scenario: reply body jsonSubset matcher passes

- **Given** a route whose steps set a JSON body `{"status": "ok", "ts": 1234}` and an input `expectReply: {body: {jsonSubset: {status: "ok"}}}`
- **When** `camel test` executes the document
- **Then** the reply assertion passes

#### Scenario: reply matcher mismatch names the matcher

- **Given** a route whose steps set the body to `done` and an input `expectReply: {body: {contains: "unfinished"}}`
- **When** `camel test` executes the document
- **Then** a FAIL line names the `contains` matcher, its value, and the received reply body, and the exit code is 1

#### Scenario: reply header regex matcher passes

- **Given** a route whose steps set header `X-Trace` to an 8-char hex value and an input `expectReply: {headers: {X-Trace: {regex: "^[a-f0-9]{8}$"}}}`
- **When** `camel test` executes the document
- **Then** the reply assertion passes

### Requirement: Declarative repository stubs

`camel test` SHALL support a `repositories:` block in the test document with
three optional maps — `cache`, `idempotent`, `claimCheck` — each mapping a
repository name to the stub target `memory`. The runner SHALL register a
`MemoryCacheRepository`, `MemoryIdempotentRepository`, or
`MemoryClaimCheckRepository` under each declared name before routes are
added and compiled, so routes whose steps reference named repositories
(`cache:`, `cache_invalidate:`, `cache_clear:`, `cache_peek_stale:`,
`cache_stats:`, idempotent consumer, claim check) compile and run against
in-memory stubs. Validation SHALL be eager at parse time: unknown registry
kinds, unknown stub targets, blank repository names, and use of the
built-in name `memory` as a stub name SHALL be document errors (exit 2).
Only the literal target `memory` SHALL be valid. Repository names that the
document does not declare SHALL continue to fail route compilation with the
same unknown-repository error production uses.

A stub is lossy by design: a green run under `memory` proves
backend-agnostic decision logic only, and it bypasses production
`Camel.toml` repository registration, so it cannot validate missing or
invalid backend configuration. Surfaces a stub does NOT exercise, by
registry: for cache stubs, prefix purge (`invalidate_prefix` — memory
fails closed, redb range-deletes, redis SCAN+UNLINK), backend-specific
TTL/stale-retention timing fidelity, disk-offload decorator behavior,
backend-specific `stats` fidelity, and backend-failure error paths; for
idempotent and claim-check stubs, persistence semantics and backend-failure
error paths. Coverage of those surfaces belongs to the integration tier.

#### Scenario: cache stub compiles a named-repository route

- **Given** a route `from("direct:in").cache(repository: "persistent",
  key: "k").to("mock:out")` and a test document declaring
  `repositories: { cache: { persistent: memory } }` with two inputs for
  the same key
- **When** `camel test` runs the document
- **Then** the route compiles, the first input takes the miss path, the
  second takes the hit path, and the mock expectations pass

#### Scenario: idempotent stub filters duplicate inputs

- **Given** a route with an idempotent consumer referencing repository
  `redis` and a test document declaring
  `repositories: { idempotent: { redis: memory } }`, with two duplicate
  inputs (same message id) delivered
- **When** `camel test` runs the document
- **Then** the route compiles and the downstream mock endpoint receives
  exactly one exchange — the duplicate is filtered by the in-memory
  idempotent repository

#### Scenario: claimCheck stub round-trips content

- **Given** a test document declaring
  `repositories: { claimCheck: { redb: memory } }` for a route that
  claim-checks a body to the repository `redb` and later restores it
- **When** `camel test` runs the document
- **Then** the route compiles and the restored body equals the checked-in
  body, asserted through the mock expectation

#### Scenario: undeclared repository name still fails route load

- **Given** a route referencing repository `persistant` (typo) and a test
  document declaring `repositories: { cache: { persistent: memory } }`
- **When** `camel test` runs the document
- **Then** route loading fails with the unknown-repository error naming the
  step and the repository, as a document error (exit 2) — identical to a
  run without the `repositories:` block

#### Scenario: unknown registry kind is a document error

- **Given** a test document with `repositories: { blob: { x: memory } }`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 rejecting the unknown field and
  listing the supported registry kinds

#### Scenario: unknown stub target is a document error

- **Given** a test document with
  `repositories: { cache: { persistent: rocksdb } }`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 naming the unsupported target and
  listing the supported target `memory`

#### Scenario: blank repository name is a document error

- **Given** a test document with `repositories: { cache: { "  ": memory } }`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating repository names must be
  non-blank

#### Scenario: stubbing the built-in memory name is a document error

- **Given** a test document with `repositories: { cache: { memory: memory } }`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating `memory` is a built-in
  repository name and cannot be stubbed

#### Scenario: stubbing emits a per-run warning

- **Given** a test document declaring any repository stub
- **When** `camel test` runs the document
- **Then** the run emits a stderr warning with the code
  `R-REPOSITORY-STUB` naming each stubbed registry and repository name, and
  stating that backend semantics (for cache: prefix purge, TTL/stale
  timing, disk offload, stats; for idempotent/claim-check: persistence) and
  backend-failure paths are not exercised and belong to the integration
  tier

#### Scenario: camel run ignores the repositories block

- **Given** a project whose route files are loaded by `camel run` and whose
  test documents declare `repositories:` blocks
- **When** `camel run` boots from the same project
- **Then** runtime repository registration is driven solely by
  `Camel.toml` as before — the block lives only in test documents, which
  `camel run` never parses

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

