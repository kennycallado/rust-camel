# mock-testkit Delta — declarative-intercepts

## MODIFIED Requirements

### Requirement: Declarative test document parsing

`camel test` SHALL accept one or more test documents (`*.test.yaml`). A test
document SHALL contain exactly one route source: `routeFiles` (paths to
route YAML files, resolved relative to the test document's directory),
`routeFilesFromRoot` (paths resolved against the nearest ancestor
`Camel.toml` directory), or inline `routes` (same schema as route files).
It MAY contain optional `inputs` and SHALL contain a mandatory non-empty
`expects` map keyed by mock endpoint name. It MAY contain an optional
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

## ADDED Requirements

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
