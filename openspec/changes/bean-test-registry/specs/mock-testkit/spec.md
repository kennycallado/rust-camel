## ADDED Requirements

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

## MODIFIED Requirements

### Requirement: Exit codes, reporting, and multi-document execution

`camel test` SHALL execute documents in CLI argument order, sequentially. A
document-level error (unreadable file, parse error, boot failure, or input
delivery failure such as a processor error propagating out of the route)
SHALL be reported and execution SHALL continue with the next document; a
document whose input delivery failed SHALL skip settling and endpoint
evaluation for that document. Exit codes: 0 when every expectation of every
document passes; 1 when any expectation fails or a settle timeout occurs;
2 for misuse, unreadable files, document/route parse errors, and input
delivery failures. When classes coexist, precedence is 2 > 1 > 0. stdout
SHALL carry one `PASS`/`FAIL` line per endpoint per document and a final
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

#### Scenario: input delivery failure exits 2 and skips evaluation
- **Given** a document whose route input delivery fails (for example a bean processor returning an error with no error handler configured)
- **When** `camel test <doc>` runs
- **Then** the failure is reported, no endpoint lines are printed for that document, and the exit code is 2

### Requirement: In-process route execution

`camel test` SHALL boot a `CamelContext` in-process per document, register the
real mock component (plus direct, timer, log, seda), load referenced or inline
routes through the same per-file YAML parser `camel run` uses, and SHALL NOT
start WASM plugins, file-watch, or network servers, and SHALL NOT load user
beans (including WASM or native beans); it MAY register built-in in-process
stub beans declared in the test document's `beans:` block. Route execution
SHALL involve no IPC and no RuntimeBus/QueryBus traffic.

#### Scenario: routes run in-process
- **Given** a test document referencing a route file with `from: direct:start` → `to: mock:result`
- **When** `camel test` executes the document with an input `{to: direct:start, body: "x"}`
- **Then** the exchange reaches the in-process mock endpoint and `mock:result` records body `x`

#### Scenario: self-starting route without inputs
- **Given** a test document whose route uses `timer:tick?period=50&repeatCount=3` → `to: mock:result` and `expects: {mock:result: {count: 3}}`
- **When** `camel test` executes the document with no `inputs`
- **Then** the timer drives 3 exchanges and the count expectation is evaluated against them
