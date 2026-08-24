## MODIFIED Requirements

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
