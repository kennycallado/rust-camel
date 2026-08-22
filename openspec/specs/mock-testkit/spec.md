# mock-testkit Specification

## Purpose
TBD - created by archiving change mock-declarative-testkit. Update Purpose after archive.
## Requirements
### Requirement: Declarative test document parsing

`camel test` SHALL accept one or more test documents (`*.test.yaml`). A test
document SHALL contain exactly one route source: `routeFiles` (paths to
route YAML files, resolved relative to the test document's directory),
`routeFilesFromRoot` (paths resolved against the nearest ancestor
`Camel.toml` directory), or inline `routes` (same schema as route files).
It MAY contain optional `inputs` and SHALL contain a mandatory non-empty
`expects` map keyed by mock endpoint name. Documents declaring two or three
route sources, or none, SHALL be rejected. Unknown fields SHALL be rejected.

#### Scenario: valid document with reference and expectations

- **Given** a test document with `routeFiles: [config/routes.yaml]` and `expects: {mock:result: {count: 3}}`
- **When** `camel test` parses the document
- **Then** parsing succeeds, `config/routes.yaml` resolves relative to the test document's directory, and one expectation set exists for endpoint `result`

#### Scenario: unknown field rejected

- **Given** a test document containing an unknown top-level field
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 and a message naming the unknown field

#### Scenario: empty expects rejected

- **Given** a test document with `expects: {}` or no `expects` key
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating that `expects` is mandatory

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

### Requirement: In-process route execution

`camel test` SHALL boot a `CamelContext` in-process per document, register the
real mock component (plus direct, timer, log, seda), load referenced or inline
routes through the same per-file YAML parser `camel run` uses, and SHALL NOT
start beans, WASM plugins, file-watch, or network servers. Route execution
SHALL involve no IPC and no RuntimeBus/QueryBus traffic.

#### Scenario: routes run in-process
- **Given** a test document referencing a route file with `from: direct:start` → `to: mock:result`
- **When** `camel test` executes the document with an input `{to: direct:start, body: "x"}`
- **Then** the exchange reaches the in-process mock endpoint and `mock:result` records body `x`

#### Scenario: self-starting route without inputs
- **Given** a test document whose route uses `timer:tick?period=50&repeatCount=3` → `to: mock:result` and `expects: {mock:result: {count: 3}}`
- **When** `camel test` executes the document with no `inputs`
- **Then** the timer drives 3 exchanges and the count expectation is evaluated against them

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
`expect_minimum_count`, `bodies` → ordered `expect_body`, `headers` →
`expect_header`), and evaluate with `try_assert_satisfied()`. `count` and
`minCount` in the same entry SHALL be a document error (exit 2). Assertion
failures SHALL be reported without aborting remaining endpoints.

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
document-level error (unreadable file, parse error, boot failure) SHALL be
reported and execution SHALL continue with the next document. Exit codes: 0
when every expectation of every document passes; 1 when any expectation fails
or a settle timeout occurs; 2 for misuse, unreadable files, or document/route
parse errors. When classes coexist, precedence is 2 > 1 > 0. stdout SHALL
carry one `PASS`/`FAIL` line per endpoint per document and a final
`N passed, M failed` summary.

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

