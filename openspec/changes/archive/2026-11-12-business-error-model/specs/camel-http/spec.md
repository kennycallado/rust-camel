## MODIFIED Requirements

### Requirement: JSON error replies preserve existing behavior

The HTTP error finalizer SHALL construct JSON replies for
`TypeConversionFailed`, `ValidationError`, `UnsupportedMediaType`, and
`NotAcceptable` through one private helper. `ValidationError` SHALL map to
400, `UnsupportedMediaType` to 415 (media negotiation gate, REST lowering),
and `NotAcceptable` to 406 (same gate). `TypeConversionFailed` SHALL map to
500 with the JSON error code `conversion_failed` and a log line that does not
claim a bad request (ADR-0096 fault blame: a conversion failure on data that
is not by definition request-borne is a server fault).
`Unauthenticated`/`Unauthorized` keep their `401`/`403` mappings and
`ConsumerStopping` keeps its `503 Service Unavailable` mapping. The remaining
fallback arm SHALL carve out `CamelError::Business`: a business error SHALL
map through the route's error mapping per the requirement "business error
HTTP mapping with safe closure" (built-in default kind map, then 500), never
through this fallback arm; every other remaining error maps to
`500 Internal Server Error`. The reply arm SHALL also never answer 2xx for an
exchange that still carries an error state (bd `rc-7lkp2`): when the route
result is an `Ok` exchange whose error is present, the finalizer SHALL treat
it as an error and map it like any other pipeline error — through the
EXISTING variant mappings for non-business errors (`ValidationError` 400,
`ConsumerStopping` 503, and so on) and through the route's error mapping for
a business error — never returning a success status. The never-2xx guarantee
scopes to errors not successfully handled: a handled (and therefore cleared)
error that completed the pipeline is the error handler's documented outcome,
not a finalizer input.

#### Scenario: Type conversion failure is a server error

- **GIVEN** `pipeline_error_to_reply` receives `TypeConversionFailed("bad")`
- **WHEN** it maps the error to an HTTP reply
- **THEN** the reply has status 500, `Content-Type: application/json`, and
  JSON fields `error: "conversion_failed"` and `message: "bad"`; neither the
  JSON error code nor the log line claims a bad request

#### Scenario: Consumer shutdown remains 503

- **GIVEN** `pipeline_error_to_reply` receives `CamelError::ConsumerStopping`
- **WHEN** it maps the error to an HTTP reply
- **THEN** the reply has status 503 and the shutdown mapping is unchanged

#### Scenario: Validation failure remains a bad request

- **GIVEN** `pipeline_error_to_reply` receives `ValidationError("invalid")`
- **WHEN** it maps the error to an HTTP reply
- **THEN** the reply has status 400, `Content-Type: application/json`, and
  JSON fields `error: "validation_error"` and `message: "invalid"`

#### Scenario: Unsupported media remains 415

- **GIVEN** `pipeline_error_to_reply` receives `UnsupportedMediaType` with
  consumed and declared media values
- **WHEN** it maps the error to an HTTP reply
- **THEN** the reply has status 415, `Content-Type: application/json`, and
  message `consumed {consumed}, declared {declared}`

#### Scenario: Unacceptable media remains 406

- **GIVEN** `pipeline_error_to_reply` receives `NotAcceptable` with accept and
  produced media values
- **WHEN** it maps the error to an HTTP reply
- **THEN** the reply has status 406, `Content-Type: application/json`, and
  message `accept {accept}, produced {produced}`

#### Scenario: Empty messages remain valid JSON

- **GIVEN** the helper receives an empty message
- **WHEN** it serializes the reply
- **THEN** the JSON message field is an empty string and the reply remains
  application/json

#### Scenario: business errors leave the fallback arm

- **GIVEN** the finalizer receives `CamelError::Business { kind: "conflict", .. }`
- **WHEN** it maps the error to an HTTP reply
- **THEN** the reply follows the business mapping (409 from the built-in
  default kind map), not the fallback 500

#### Scenario: an Ok exchange carrying an error never answers 2xx

- **GIVEN** a handler-configured route whose boundary path returns
  `Ok(exchange)` with the error still present on the exchange (the
  `rc-7lkp2` path)
- **WHEN** the reply arm produces the response
- **THEN** the reply maps the carried error like any other pipeline error —
  the route's error mapping for a business error, the existing variant
  mappings for non-business errors, 500 otherwise — and is never 2xx

#### Scenario: Ok exchange carrying ValidationError maps through the existing mapping

- **GIVEN** an `Ok` exchange whose error is `ValidationError("invalid")` (the
  `rc-7lkp2` path)
- **WHEN** the reply arm produces the response
- **THEN** the reply has status 400 — the existing variant mapping, reused

#### Scenario: Ok exchange carrying ConsumerStopping maps to 503

- **GIVEN** an `Ok` exchange whose error is `ConsumerStopping` (the
  `rc-7lkp2` path)
- **WHEN** the reply arm produces the response
- **THEN** the reply has status 503 — the existing shutdown mapping, reused

## ADDED Requirements

### Requirement: business error HTTP mapping with safe closure

The HTTP error finalizer `pipeline_error_to_reply` SHALL accept an optional
error mapping and SHALL map `CamelError::Business` errors through it. The
mapping type `HttpErrorMapping { codes: <code-to-status>, kinds:
<kind-to-status> }` SHALL live in camel-api and ride the ROUTE as a separate
route-level field (`RouteDslRoute.http_error_mapping` in the DSL AST, carried
through the route translation into the compiled route), NOT as a field of
`ErrorHandlerConfig` — the mapping is orthogonal to the error handler, which
stays untouched (a route may carry a dead-letter channel and a mapping at the
same time). The DSL AST field SHALL be `#[serde(skip_deserializing)]`,
schema-skipped, and `#[ts(skip)]`: plain-route YAML authoring is deferred to the phase-3
`errors:` catalog, and the rest block's `error_mapping` is the only
document-level authoring path in this scope. Table keys SHALL be exact,
non-blank, case-sensitive strings; values SHALL be client/server error
statuses (400-599). Load-time validation SHALL reject, with a typed error:
blank keys and every value outside 400-599 — including informational,
success, and redirect statuses such as 101 and 304. Because programmatic
construction can bypass document validation, the finalizer SHALL also defend:
a mapping entry whose value falls outside 400-599 SHALL be treated as absent
at reply time, and normal status resolution SHALL resume — the remaining
tables, then the built-in default kind map, then 500. Carriage: the
route-to-consumer seam is trusted registration configuration — the route's
mapping travels as an immutable snapshot on the consumer's registration
context (`ConsumerContext`), captured per registration (start and resume) and
passed to `pipeline_error_to_reply` per invocation; no global or ambient
lookup SHALL be used. Status resolution precedence: explicit code map, then
explicit kind map, then the built-in default kind map (`invalid-argument` 400,
`conflict` 409, `not-found` 404, `forbidden` 403, `failed` 500 — kebab-case,
matching the existing error kind strings), then 500. A business error SHALL
NEVER produce a 2xx reply. The reply body for a business error SHALL be
exactly `{"error": <code>, "kind": <kind>}` as JSON; the `message` and `data`
payload SHALL NOT appear in the reply body (ADR-0032: runtime exchange data is
untrusted; `code` and `kind` are operator-authored). No mapping configuration
authorizes `message` or `data` disclosure. Non-business errors keep their
existing variant mappings unchanged. A route without a mapping still maps
business errors through the default kind map and the 500 closure.

#### Scenario: code mapping wins over kind mapping

- **GIVEN** a mapping `{codes: {"ORDER_NOT_FOUND": 404}, kinds: {"not-found": 410}}`
- **WHEN** `pipeline_error_to_reply` receives `Business { code: "ORDER_NOT_FOUND", kind: "not-found", .. }`
- **THEN** the reply has status 404 and body `{"error": "ORDER_NOT_FOUND", "kind": "not-found"}`

#### Scenario: kind mapping applies when no code matches

- **GIVEN** a mapping `{kinds: {"conflict": 409}}`
- **WHEN** the finalizer receives `Business { code: "STALE", kind: "conflict", .. }`
- **THEN** the reply has status 409

#### Scenario: built-in default kind map applies without any explicit mapping

- **GIVEN** no explicit mapping is configured
- **WHEN** the finalizer receives `Business { kind: "invalid-argument", .. }`
  and, in a second request, `Business { kind: "forbidden", .. }`
- **THEN** the replies have status 400 and 403 respectively

#### Scenario: unmapped kind falls back to 500 and never 2xx

- **GIVEN** no explicit mapping is configured
- **WHEN** the finalizer receives `Business { kind: "custom-kind", .. }`
- **THEN** the reply has status 500 and the body is `{"error": <code>, "kind": "custom-kind"}`

#### Scenario: mapped 2xx, 1xx, 3xx, and out-of-range statuses fail at load time

- **GIVEN** mappings containing `{"not-found": 200}`, `{"not-found": 101}`,
  `{"not-found": 304}`, and `{"not-found": 700}` in four route documents
- **WHEN** each route loads
- **THEN** loading fails with a typed error rejecting the non-400-599 value

#### Scenario: blank table keys fail at load time

- **GIVEN** a mapping containing a blank code key or a blank kind key
- **WHEN** the route loads
- **THEN** loading fails with a typed error naming the blank key

#### Scenario: programmatic out-of-domain entries are treated as absent and normal precedence resumes

- **GIVEN** a mapping constructed programmatically (bypassing document
  validation) with an invalid code entry and an invalid `not-found` kind entry,
  plus a valid explicit kind entry for a second error
- **WHEN** the finalizer receives a business error whose code entry is invalid
  but whose kind has a valid explicit entry
- **THEN** the reply uses the valid kind mapping's status
- **WHEN** the finalizer receives a business error of kind `not-found` whose
  kind entry is invalid (204)
- **THEN** the invalid entry is absent, the built-in default kind map decides,
  and the reply is 404
- **WHEN** the finalizer receives a business error of an unknown kind whose
  kind entry is invalid
- **THEN** the reply is 500

#### Scenario: message and data never reach the reply body

- **GIVEN** a business error whose `message` embeds exchange data and whose
  `data` carries structured fields
- **WHEN** the finalizer produces any reply for it
- **THEN** the body contains only the `error` (code) and `kind` fields, and
  neither the message text nor any data field appears in the body

#### Scenario: mapping coexists with a dead-letter error handler

- **GIVEN** a route with `error_handler` carrying a dead-letter channel AND a
  route-level mapping (`kinds: {"not-found": 404}`)
- **WHEN** a business error propagates to the transport
- **THEN** the reply maps through the mapping (404) even though the error
  handler is configured, because the mapping is a separate route-level field
  and business errors bypass default dead-letter dispatch
