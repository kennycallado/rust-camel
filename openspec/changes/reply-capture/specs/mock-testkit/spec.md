## ADDED Requirements

### Requirement: Reply capture and assertion

`camel test` SHALL capture the reply exchange each `direct:` input's producer
oneshot returns and SHALL support asserting it via an optional per-input
`expectReply: {body?, headers?}` block. Reply assertions SHALL be exact-match
against the reply message — the reply's `output` message when the route set
one, else its final input message — body (string or JSON) and/or an exact
`headers` submap (JSON values, matching `Message.headers`). At least one of
`body`/`headers` SHALL be present (an empty `expectReply` is a document
error, exit 2). Replies pair with inputs by delivery order (delivery is
strictly sequential). A failed reply assertion SHALL be an assertion failure
— a `FAIL` line counted into the summary and exit code 1 — never a document
error. Inputs without `expectReply` SHALL behave exactly as before (reply
captured, nothing asserted). The document-level `expects` relaxation for
reply-only documents is specified in the MODIFIED parsing requirement.

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

## MODIFIED Requirements

### Requirement: Exit codes, reporting, and multi-document execution

`camel test` SHALL execute documents in CLI argument order, sequentially. A
document-level error (unreadable file, parse error, boot failure, or input
delivery failure such as a processor error propagating out of the route)
SHALL be reported and execution SHALL continue with the next document; a
document whose input delivery failed SHALL skip settling, endpoint
evaluation, and reply evaluation for that document. Exit codes: 0 when every
expectation of every document passes; 1 when any expectation (endpoint or
reply) fails or a settle timeout occurs; 2 for misuse, unreadable files,
document/route parse errors, and input delivery failures. When classes
coexist, precedence is 2 > 1 > 0. stdout SHALL carry one `PASS`/`FAIL` line
per endpoint per document, one `PASS`/`FAIL` line per asserted reply per
document, and a final `N passed, M failed` summary.

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

### Requirement: Declarative test document parsing

`camel test` SHALL accept one or more test documents (`*.test.yaml`). A test
document SHALL contain exactly one route source: `routeFiles` (paths to
route YAML files, resolved relative to the test document's directory),
`routeFilesFromRoot` (paths resolved against the nearest ancestor
`Camel.toml` directory), or inline `routes` (same schema as route files).
It MAY contain optional `inputs` (each input MAY declare an optional
`expectReply` block, see Reply capture and assertion) and SHALL contain a
mandatory non-empty `expects` map keyed by mock endpoint name — except that
a document MAY omit `expects` (or leave it empty) when at least one input
declares `expectReply`; a document with neither endpoint expectations nor any
`expectReply` SHALL be rejected. It MAY contain an optional
`intercepts` map keyed by real endpoint URI (any scheme except `mock:`),
where each key is used verbatim (no trimming or normalization; query
parameters are significant and participate in exact matching) and each value
carries exactly one action: `skipTo` (a `mock:` URI with a non-empty
endpoint path that replaces the send before component resolution) or
`divertCopyTo` (a `mock:` URI with a non-empty endpoint path that receives
a copy while the real send continues). Documents declaring
two or three route sources, or none, SHALL be rejected. Unknown fields
SHALL be rejected, including inside intercept action objects.

#### Scenario: valid document with reference and expectations

- **Given** a test document with `routeFiles: [config/routes.yaml]` and `expects: {mock:result: {count: 3}}`
- **When** `camel test` parses the document
- **Then** parsing succeeds, `config/routes.yaml` resolves relative to the test document's directory, and one expectation set exists for endpoint `result`

#### Scenario: unknown field rejected

- **Given** a test document containing an unknown top-level field
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 and a message naming the unknown field

#### Scenario: empty expects rejected without expectReply

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
