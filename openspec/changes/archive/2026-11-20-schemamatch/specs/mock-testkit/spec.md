# Delta: mock-testkit (change `schemamatch`)

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
`divertCopyTo` (a `mock:` URI with a non-empty endpoint path that
receives a copy while the real send continues). Documents declaring
two or three route sources, or none, SHALL be rejected. Unknown fields
SHALL be rejected, including inside intercept action objects. A scenario
document MAY contain an optional `env` map and an optional ambient
passthrough allowlist, per the integration-tier spec.

Matcher grammar. Body matcher keys: `equals`, `regex`, `contains`,
`startsWith`, `endsWith`, `exists`, `jsonSubset`, `jsonSchema`. Header matcher keys:
`equals`, `regex`, `exists`. `exists` takes no argument (a `null` value).
`jsonSubset` SHALL be an object mapping and applies to bodies only; it is
a document error on a header value. `jsonSchema` SHALL be an object with a
required non-empty string `path` and an optional `type` (`json` or `yaml`)
and applies to bodies only; it is a document error on a header value; an
unknown field inside the object, a missing or empty `path`, a null or
non-string `type`, or an unknown `type` is a document error. Every `jsonSchema`
rejection SHALL name the document field and the key. The `path` SHALL be a
document-relative path, resolved against the unit test document's directory;
an absolute `path` SHALL be rejected at parse, naming the field, the key, and
the path (the absolute escape hatch of ADR-0100 covers `$ref` targets inside
the schema, not the matcher root path). Compilation uses the confined compiler
of the
`json-schema-validation` specification (ADR-0100) through
`SchemaSyntax::Auto`'s extension rule (`.yaml`/`.yml` are YAML, every other
extension is JSON) unless `type` overrides it, and SHALL happen once at
document load, never per assertion. The document SHALL fail closed at load,
naming the schema path, when the schema file is missing or unreadable, when a
`$ref` cannot be resolved, when the schema is invalid, or when `type` names an
unsupported schema language (`xsd`, `protobuf`) — the rejection SHALL name
the rejected value and the
supported set (json, yaml). An object whose sole key is
`predicate` SHALL be rejected with a message stating predicate matchers
are not supported, in every grammar position; in dual-grammar positions
(header values and `expectReply.body`), an object with multiple keys that
merely contains a `predicate` key is a literal value and parses
unchanged (in strict `bodies` entries, multi-key maps are rejected as
usual). Matchers SHALL be validated at
parse time: `regex` patterns SHALL compile, `jsonSchema` documents SHALL
compile their schema at load, and malformed values SHALL fail
parsing with exit code 2 naming the document field and the offending key.

`jsonSchema` matching. A received or replied body that is JSON (or text that
parses as JSON) and violates the schema SHALL fail the matcher (assertion
failure, exit 1); the failure
message SHALL carry the schema path and the first violation in the shape
`{message} at {instance_path}`. A body that is not JSON SHALL be a non-match,
not an error: the failure message SHALL carry the schema path and a note that
the body is not JSON, on the ordered expectation path, on the reply
assertion path, and — as a deterministic diagnostic candidate rendered with
the first unmatched body in receive order — on the anyOrder path, without
changing any matching semantics.

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
stating `jsonSubset` applies to bodies only, a sole `jsonSchema` key on a
header value is a document error stating `jsonSchema` applies to bodies
only, and a sole `predicate` key
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

#### Scenario: jsonSchema on a header value rejected at parse

- **Given** a test document with a header expectation `{jsonSchema: {path: "order.schema.json"}}`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 naming the field and stating `jsonSchema` applies to bodies only

#### Scenario: reply body object without matcher keys stays literal JSON

- **Given** an input with `expectReply: {body: {"status": "ok"}}` (single unrecognized key)
- **When** `camel test` parses and executes the document against the JSON reply body `{"status": "ok"}`
- **Then** parsing succeeds without matcher interpretation and the reply assertion passes by structural equality

#### Scenario: jsonSchema with valid body matches

- **Given** a schema file `order.schema.json` next to the test document requiring a string `id` property, and an expectation `{jsonSchema: {path: "order.schema.json"}}`
- **When** `camel test` parses and executes the document against the JSON body `{"id": "order-1"}`
- **Then** parsing compiles the schema once at load and the expectation passes

#### Scenario: jsonSchema violation fails with the first violation

- **Given** the same schema and expectation, executed against the JSON body `{}`
- **When** the assertion evaluates and the document exits with code 1
- **Then** the failure message names the schema path and the first violation in the shape `{message} at {instance_path}`

#### Scenario: yaml schema selected by file extension

- **Given** a schema file `order.schema.yaml` next to the test document and an expectation `{jsonSchema: {path: "order.schema.yaml"}}`
- **When** `camel test` parses and executes the document against a conforming JSON body
- **Then** parsing succeeds (extension-based YAML parsing) and the expectation passes

#### Scenario: explicit type overrides the extension rule

- **Given** a file named `order.schema.txt` containing YAML-only schema content (invalid when parsed as JSON) and an expectation `{jsonSchema: {path: "order.schema.txt", type: "yaml"}}`
- **When** `camel test` parses and executes the document against a conforming JSON body
- **Then** parsing succeeds (the explicit `type` selects the YAML parser, which the extension rule alone would not) and the expectation passes

#### Scenario: jsonSchema with non-JSON body is a non-match

- **Given** the valid schema and expectation of the matching scenario
- **When** the document executes against a body of `not json at all` and exits with code 1
- **Then** the matcher fails as a non-match — the failure message names the schema path and reports the body is not JSON — and no error is raised by the matcher itself

#### Scenario: jsonSchema on expectReply.body asserts the reply

- **Given** the valid schema of the matching scenario referenced from `expectReply: {body: {jsonSchema: {path: "order.schema.json"}}}`
- **When** `camel test` parses and executes the document and the route replies with a body violating the schema
- **Then** the reply assertion fails with the schema path and the first violation in the established shape

#### Scenario: jsonSchema failure under anyOrder diagnoses an unconsumed body

- **Given** a document with `bodies` declared with `anyOrder`, two identical `jsonSchema` expectations for the same schema, and received bodies `[valid, invalid]` in that order
- **When** the assertions evaluate (the first expectation consumes the valid body; the second finds no match)
- **Then** the failure names the schema path and the violation of the `invalid` body — the first unmatched body in receive order — and matching semantics are unchanged

#### Scenario: jsonSchema missing file rejected at load

- **Given** a test document with an expectation `{jsonSchema: {path: "missing.schema.json"}}` and no such file in the document's directory
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 naming the schema path

#### Scenario: jsonSchema unreadable file rejected at load

- **Given** a schema file in the document's directory that exists but cannot be read by the process (e.g. a directory at the declared path)
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 naming the schema path

#### Scenario: jsonSchema unresolvable ref rejected at load

- **Given** a schema file whose `$ref` points at a file outside the schema's directory
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 naming the schema path and the confinement rejection (per the `json-schema-validation` specification)

#### Scenario: jsonSchema ref to a missing file inside the directory rejected at load

- **Given** a schema file whose `$ref: "defs.json#/$defs/x"` names a file that does not exist inside the schema's directory
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 naming the schema path and the unresolved reference

#### Scenario: jsonSchema invalid schema rejected at load

- **Given** a schema file containing malformed JSON, or a syntactically valid document whose keywords do not compile (e.g. `maximum: "five"`)
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 naming the schema path and the parse or compile error

#### Scenario: jsonSchema unsupported type rejected at load

- **Given** a test document with an expectation `{jsonSchema: {path: "order.xsd", type: "xsd"}}`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 naming the field, the key, the rejected value (`xsd`), the supplied path, and the supported set (json, yaml)

#### Scenario: jsonSchema absolute path rejected at parse

- **Given** a test document with an expectation `{jsonSchema: {path: "/abs/order.schema.json"}}`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 naming the field, the key, and the absolute path, stating the path must be relative to the document's directory

#### Scenario: jsonSchema relative path resolves against the document directory

- **Given** a test document at `docs/case.test.yaml` with an expectation `{jsonSchema: {path: "schemas/order.schema.json"}}`, the schema at `docs/schemas/order.schema.json`, and the process current directory set elsewhere (e.g. the repository root)
- **When** `camel test` parses the document
- **Then** parsing succeeds — the schema path resolved against the document's directory, not the process current directory

#### Scenario: jsonSchema unknown field inside the object rejected

- **Given** a test document with an expectation `{jsonSchema: {path: "order.schema.json", format: "draft7"}}`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 naming the field (`bodies` or `expectReply.body`) and the unknown field (`format`)

#### Scenario: jsonSchema without path rejected

- **Given** a test document with an expectation `{jsonSchema: {type: "json"}}` or `{jsonSchema: "order.schema.json"}` (non-object payload)
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 naming the field and the key, stating `jsonSchema` requires an object with a `path`

#### Scenario: jsonSchema with non-string type rejected

- **Given** a test document with an expectation `{jsonSchema: {path: "order.schema.json", type: 42}}` or `{jsonSchema: {path: "order.schema.json", type: null}}`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 naming the field and the key, stating `type` must be `json` or `yaml`

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
