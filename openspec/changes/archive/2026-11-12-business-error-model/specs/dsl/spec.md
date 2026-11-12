## MODIFIED Requirements

### Requirement: on_exceptions wildcard clause

A declarative `error_handler.on_exceptions` clause with `kind: "*"` SHALL
match every `CamelError` variant. The wildcard SHALL combine with
`message_contains` by conjunction (both must hold). Clause evaluation SHALL
keep first-match-wins order, so a wildcard clause SHALL only receive errors
that no earlier clause matched. All clause features (`handled`, `continued`,
`handled_by`, `retry`, `steps`) SHALL behave with the wildcard exactly as
with a specific kind. `handled_by` is a clause-level field; the `retry`
block carries only redelivery parameters and rejects unknown fields. The
wildcard SHALL also match `CamelError::Business`: a wildcard clause catches
business errors like any other variant, and the business global policy (no
declarative redelivery, breaker success, default dead-letter bypass) applies
to those matches exactly as to specific-kind matches (owner decision
2026-11-12).

#### Scenario: wildcard matches every error kind

- **Given** a route with `error_handler.on_exceptions: [{kind: "*", handled:
  true, handled_by: "direct:shaper"}]`
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
  second clause is `kind: "*", handled: true, handled_by:
  "direct:shaper"`, where the route continues (after the handled error)
  to a step that writes `"recovered"` into the body
- **When** a step fails with `Io("disk")`
- **Then** the route continues to the next step and the final body is
  `"recovered"`, and the `direct:shaper` endpoint receives no exchange
- **When** a step fails with `ValidationError("schema mismatch")`
- **Then** the wildcard clause runs `direct:shaper` and its output is the
  final result

#### Scenario: wildcard narrowed by message_contains

- **Given** a route whose first clause is `kind: "*", message_contains:
  "timeout", handled: true, handled_by: "direct:timeout-shaper"`
  and second clause is `kind: "*", handled: true, handled_by:
  "direct:generic-shaper"`
- **When** a step fails with `Io("connection timeout")`
- **Then** the first clause matches and `direct:timeout-shaper` output is
  the final result, and `direct:generic-shaper` receives no exchange
- **When** a step fails with `Io("disk full")`
- **Then** the first clause does not match, `direct:generic-shaper` runs
  and its output is the final result

#### Scenario: wildcard compiles from the JSON route format

- **Given** a JSON route definition with `error_handler.on_exceptions` containing
  `{"kind": "*", "handled": true, "handled_by": "direct:shaper"}`
- **When** the JSON route compiles
- **Then** compilation succeeds and the resulting policy matches a
  `ValidationError` and a `ProcessorError`

#### Scenario: unknown kind still rejected

- **Given** an `on_exceptions` clause with `kind: "NoSuchKind"`
- **When** the route compiles
- **Then** compilation fails with the existing unknown-kind error, and
  `"*"` is the only newly accepted kind value

#### Scenario: wildcard matches a business error under the global policy

- **Given** a route with `error_handler.on_exceptions: [{kind: "*", handled:
  true, handled_by: "direct:shaper"}]` and a route-level dead-letter channel
- **When** a step fails with `Business { code: "X", kind: "failed", .. }`
- **Then** the wildcard clause matches, `direct:shaper` output is the final
  result, and no declarative redelivery occurs — the business global policy
  applies to the wildcard match exactly as to specific matches

## ADDED Requirements

### Requirement: throw step raises a business error

The declarative DSL (YAML and JSON route formats) SHALL accept a `throw` step
with fields `code` (required), `kind` (optional, default `"failed"`), `message`
(optional Simple template, default the `code` value), and `data` (optional
mapping interpolated at runtime). `code` and `kind` SHALL be literal strings —
never templates — and SHALL be non-blank after trimming; a blank or absent
`code` and an explicitly blank `kind` SHALL fail at load time with a typed
error. `kind` values SHOULD use the kebab-case business kind vocabulary
(`invalid-argument`, `conflict`, `not-found`, `forbidden`, `failed`) that the
built-in HTTP default kind map recognizes; the value itself stays a free
string. `message` resolves through the same expression path as `set_header`
(`ValueSourceDef`). `data` SHALL be an object mapping; its string leaves are
Simple templates evaluated at runtime against the current exchange, its
non-string leaves (numbers, booleans, null) pass through unchanged, and nested
objects and arrays interpolate recursively; the resolved document SHALL be a
JSON value. Compile-time (load) validation covers the structure: blank
`code`/`kind` and a non-object `data`. Runtime evaluation failures of the
`message` or `data` templates SHALL fail the step with a typed error at
exchange time, per the language evaluation-error rule
(`language-value-boundary`); they SHALL NOT fail at load time. The step SHALL
compile to the late-bound carrier `BuilderStep::DeclarativeThrow` (the
`BuilderStep::DeclarativeSetHeader` precedent) resolved by the core step
compiler, and return
`Err(CamelError::Business { code, kind, message, data })`. The `data` payload
is diagnostics-only: it SHALL NOT be written to the exchange body or to any
exchange property.

#### Scenario: throw raises the business error with resolved templates

- **GIVEN** a route step
  `- throw: { code: "ORDER_NOT_FOUND", kind: "not-found", message: "Order ${header.id} not found", data: { orderId: "${header.id}" } }`
  with header `id` set to `"A-1"`
- **WHEN** the step executes
- **THEN** the step fails with `Business { code: "ORDER_NOT_FOUND", kind: "not-found", message: "Order A-1 not found", data: {"orderId": "A-1"} }`

#### Scenario: defaults apply when kind and message are absent

- **GIVEN** a route step `- throw: { code: "STOCK_LOW" }`
- **WHEN** the step executes
- **THEN** the raised error carries `kind: "failed"` and `message: "STOCK_LOW"`

#### Scenario: blank or missing code fails at load time

- **GIVEN** a route document with `- throw: { code: "  " }` and, in a second
  document, a `throw` step with no `code` field
- **WHEN** each route loads
- **THEN** loading fails with a typed error naming the blank or missing `code`

#### Scenario: non-object data fails at load time

- **GIVEN** a route step whose `data` is a string or an array instead of an object
- **WHEN** the route loads
- **THEN** loading fails with a typed error naming the `data` field type

#### Scenario: nested data with quotes and non-string leaves resolves at runtime

- **GIVEN** a route step whose `data` is
  `{ order: { id: "${header.id}", note: "say \"hi\"" }, attempts: 3, priority: true }`
  with header `id` set to `A"1`
- **WHEN** the step executes
- **THEN** the raised error's `data` equals
  `{"order": {"id": "A\"1", "note": "say \"hi\""}, "attempts": 3, "priority": true}`

#### Scenario: failed coercion in a data leaf fails the step at runtime

- **GIVEN** a route step whose `data` leaf is `"${int(header.absent)}"` with no
  `absent` header set
- **WHEN** the step executes
- **THEN** the step fails with a typed evaluation error at exchange time — the
  route still loaded, and no business error is raised; a plain
  `"${header.absent}"` leaf would resolve to null per the canonical
  absent-header rule and would not fail

#### Scenario: throw compiles from the JSON route format

- **GIVEN** a JSON route definition with a `throw` step carrying `code` and `kind`
- **WHEN** the route compiles
- **THEN** compilation succeeds and the step raises the same business error at runtime

### Requirement: Business joins the exception-kind vocabulary

The `on_exceptions` kind vocabulary SHALL accept `Business` as a kind value
matching only `CamelError::Business` errors, per the existing vocabulary rule
("on_exceptions exception-kind vocabulary coverage": every data-plane variant is
matchable). The vocabulary-variant guard table SHALL classify the `Business`
variant as matchable under the `Business` kind value. `business_kind` payload
matching lives in the clause fields, not in the `kind` vocabulary: `kind`
continues to name variants only. Extending `on_exceptions` clauses with a
`business_kind` payload matcher is deferred to the phase-3 `errors:` catalog
(owner decision 2026-11-12).

#### Scenario: kind Business is accepted and matches only business errors

- **GIVEN** an `on_exceptions` clause `[{kind: "Business", handled: true}]`
- **WHEN** a step fails with `Business { .. }`
- **THEN** the clause matches and handles the error
- **WHEN** a step fails with `ValidationError("bad")`
- **THEN** the clause does not match
