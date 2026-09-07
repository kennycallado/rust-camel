# Delta: mock-testkit

## ADDED Requirements

### Requirement: Arrival sequence assertion

`camel test` SHALL support a top-level `sequence:` key in the test
document: a list of `mock:` endpoint references in expected arrival
order. The parser SHALL normalize entries to bare endpoint names exactly
as `expects` keys are normalized, and SHALL reject the block as a
document error (exit 2) when it holds fewer than two entries or an entry
lacks the `mock:` scheme or names an empty endpoint path. The mock
component SHALL stamp a component-wide strictly-increasing arrival index
on every recorded exchange. After settling and after per-endpoint
expectation evaluation, the runner SHALL collect every arrival at the
endpoints listed in `sequence:` — as `(arrival index, endpoint name)`
pairs ordered by index — and SHALL require that projection to equal the
declared list exactly; arrivals at unlisted endpoints SHALL be ignored.
A mismatch SHALL fail as a verdict-class assertion (exit 1) naming the
first divergence: position, expected name, and actual name (or that the
arrivals ran out). `camel run` SHALL NOT read the `sequence:` block.
The testing documentation SHALL state that cross-endpoint order is
deterministic only between causally-ordered sends.

#### Scenario: causally-ordered sends pass

- **Given** a route `from: direct:start` → `to: mock:probe-a` → `to: mock:probe-b`, a document with one input to `direct:start`, `expects: {mock:probe-a: {count: 1}, mock:probe-b: {count: 1}}`, and `sequence: [mock:probe-a, mock:probe-b]`
- **When** `camel test` executes the document
- **Then** the process exits 0 and the sequence assertion passes (route sends are awaited in step order, so their arrivals are causally ordered; divert-copy probes deliver detached wire-tap copies, and the sequence over them reports happened order only)

#### Scenario: reversed declaration fails naming the first divergence

- **Given** the same route and document as the passing scenario, but `sequence: [mock:probe-b, mock:probe-a]`
- **When** `camel test` executes the document
- **Then** the process exits 1 with a verdict-class failure whose message names position 0, expected `probe-b`, and actual `probe-a`

#### Scenario: repeats and narrowing

- **Given** a route that sends to `mock:probe-a`, then `mock:probe-b`, then `mock:probe-a` again, and a document with `sequence: [mock:probe-a, mock:probe-b, mock:probe-a]` plus traffic to a third endpoint `mock:noise`
- **When** `camel test` executes the document
- **Then** the sequence assertion passes: repeated entries match consecutive arrivals at the same endpoint, and arrivals at the unlisted `mock:noise` are ignored

#### Scenario: sequence grammar errors are document errors

- **Given** a document with `sequence: [mock:only]`
- **When** `camel test` parses the document
- **Then** parsing fails with a document error (exit 2) stating the sequence needs at least two entries; a document whose sequence entry is `direct:x` fails the same way naming the offending ref

#### Scenario: global arrival indices are strictly increasing

- **Given** a `MockComponent` with two endpoints hit in interleaved order
- **When** the recorded arrival indices of both endpoints are read
- **Then** the merged indices are strictly increasing, each endpoint's own indices preserve its arrival order, bounded retention truncates indices in lockstep with exchanges, and resetting an endpoint does not reset the component-wide counter

#### Scenario: camel run non-interference for sequence

- **Given** a project whose `*.test.yaml` declares a `sequence:` block
- **When** `camel run` starts with the project's production routes
- **Then** production behavior is identical to a project without the block

## MODIFIED Requirements

### Requirement: Declarative intercept application

`parse_test_document` SHALL construct the Stage A `InterceptRules` from
the document's `intercepts` map and store them on the parsed document.
The runner SHALL apply the stored rules through the camel-core builder
surface before any route registration or start, so the Stage A freeze
contract holds by construction. `skipTo` SHALL replace the send before
component resolution (the real component need not be registered).
`divertCopyTo` SHALL deliver a pre-send copy to the mock while the real
send continues (the real component must be registered in the lean boot
set). Intercept targets and `expects` keys SHALL each resolve to mock
endpoint names (expects keys by parse-time normalization; `mock:` URIs
by endpoint path), so both surfaces address the same endpoint. The
driver SHALL lock the divert-copy weave with an end-to-end driver test:
the copy is recorded AND the real send proceeds. `camel run` SHALL NOT
read the `intercepts` block.

#### Scenario: skip exercises a route referencing an unregistered component

- **Given** a route `from: direct:start` → `to: kafka:orders` and a document with `intercepts: {kafka:orders: {skipTo: mock:orders}}`, one input to `direct:start`, and `expects: {mock:orders: {count: 1}}`
- **When** `camel test` executes the document
- **Then** the exchange reaches `mock:orders`, the expectation passes, and the process exits 0 without any kafka component registered

#### Scenario: divert copies to the mock while the real endpoint receives traffic

- **Given** a route `from: direct:start` → `to: seda:audit` → `to: mock:sink`, a route `from: seda:audit` → `to: mock:drained`, and a document with `intercepts: {seda:audit: {divertCopyTo: mock:audit}}` plus `expects: {mock:audit: {count: 1}, mock:drained: {count: 1}}`
- **When** `camel test` executes the document with one input
- **Then** the mock copy records the exchange AND the real `seda:audit` queue still delivers to `mock:drained`, both expectations pass

#### Scenario: divert copy locked by a driver test

- **Given** the divert scenario above expressed as a driver-level test in `camel-cli/src/commands/test/driver_tests.rs`
- **When** the driver test suite runs
- **Then** the copy recorded at the divert target and the delivery to the real endpoint's consumer are both asserted end to end

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
