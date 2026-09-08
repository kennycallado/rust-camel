# Delta Spec: on-exceptions-wildcard (dsl)

## ADDED Requirements

### Requirement: on_exceptions wildcard clause

A declarative `error_handler.on_exceptions` clause with `kind: "*"` SHALL
match every `CamelError` variant. The wildcard SHALL combine with
`message_contains` by conjunction (both must hold). Clause evaluation SHALL
keep first-match-wins order, so a wildcard clause SHALL only receive errors
that no earlier clause matched. All clause features (`handled`, `continued`,
`retry.handled_by`, `steps`) SHALL behave with the wildcard exactly as with a
specific kind.

#### Scenario: wildcard matches every error kind

- **Given** a route with `error_handler.on_exceptions: [{kind: "*", handled:
  true, retry: {handled_by: "direct:shaper"}}]`
- **When** a route step fails with `ValidationError("schema mismatch")`, and
  a second request's step fails with `ProcessorError("boom")`
- **Then** in both cases the `direct:shaper` handler route runs and its
  output exchange is the final result (error cleared, pipeline
  `Completed`)

#### Scenario: wildcard with handled true owns the full HTTP response

- **Given** an HTTP consumer route with the wildcard clause above and a
  handler route that sets body to `"shaped"`, sets the string-valued header
  `X-Custom: "yes"`, and sets `CamelHttpResponseCode: 422`
- **When** the route's validator step fails with `ValidationError`
- **Then** the HTTP response has status 422, body `"shaped"`, and the
  `X-Custom` header with value `"yes"`

#### Scenario: specific clause takes precedence over wildcard

- **Given** a route whose first clause is `kind: "Io", continued: true` and
  second clause is `kind: "*", handled: true, retry: {handled_by:
  "direct:shaper"}}`, where the route continues (after the handled error)
  to a step that writes `"recovered"` into the body
- **When** a step fails with `Io("disk")`
- **Then** the route continues to the next step and the final body is
  `"recovered"`, and the `direct:shaper` endpoint receives no exchange
- **When** a step fails with `ValidationError("schema mismatch")`
- **Then** the wildcard clause runs `direct:shaper` and its output is the
  final result

#### Scenario: wildcard narrowed by message_contains

- **Given** a route whose first clause is `kind: "*", message_contains:
  "timeout", handled: true, retry: {handled_by: "direct:timeout-shaper"}}`
  and second clause is `kind: "*", handled: true, retry: {handled_by:
  "direct:generic-shaper"}}`
- **When** a step fails with `Io("connection timeout")`
- **Then** the first clause matches and `direct:timeout-shaper` output is
  the final result, and `direct:generic-shaper` receives no exchange
- **When** a step fails with `Io("disk full")`
- **Then** the first clause does not match, `direct:generic-shaper` runs
  and its output is the final result

#### Scenario: wildcard compiles from the JSON route format

- **Given** a JSON route definition with `error_handler.on_exceptions` containing
  `{"kind": "*", "handled": true, "retry": {"handled_by": "direct:shaper"}}`
- **When** the JSON route compiles
- **Then** compilation succeeds and the resulting policy matches a
  `ValidationError` and a `ProcessorError`

#### Scenario: unknown kind still rejected

- **Given** an `on_exceptions` clause with `kind: "NoSuchKind"`
- **When** the route compiles
- **Then** compilation fails with the existing unknown-kind error, and
  `"*"` is the only newly accepted kind value
