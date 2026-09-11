## RENAMED Requirements

- FROM: `v1 byte-identity for omitted binding`
- TO: `v1 behavioral compatibility for omitted binding`

## MODIFIED Requirements

### Requirement: JSON binding mode pipeline

In binding `json`, lowering SHALL inject the L2 step sequence: a
leading media negotiation step carrying the declared `consumes` and
`produces`; then for body-bearing verbs (POST, PUT, PATCH) an
`unmarshal: json` step carrying `request_schema` when present; user
steps (`to` or `steps`); a `marshal: json` step; a `SetHeader` step
setting `Content-Type` to the trimmed declared `produces`; and a final
`SetHeaderIfAbsent` step setting the default success status for the
verb unless `success_status` is declared. Body-less verbs (GET,
DELETE, HEAD, OPTIONS) SHALL NOT receive an unmarshal step.

#### Scenario: POST step sequence

- **GIVEN** a REST POST operation in binding `json` with a `to` target
- **WHEN** the block lowers
- **THEN** the step sequence is exactly
  `[ContentNegotiation(consumes, produces), Unmarshal(json),
  To(target), Marshal(json), SetHeader(Content-Type: <produces>),
  SetHeaderIfAbsent(<verb default status>)]`

#### Scenario: GET receives no unmarshal

- **GIVEN** a REST GET operation in binding `json` with a
  `request_schema`
- **WHEN** the block lowers
- **THEN** the step sequence starts with the media negotiation step
  and contains no unmarshal step

### Requirement: Raw binding mode pipeline

In binding `raw`, lowering SHALL inject no automatic unmarshal step
and no automatic marshal step. The lowered step sequence SHALL be: a
leading media negotiation step carrying the declared `consumes` and
`produces`; user steps (`to` or `steps`); a `SetHeader` step setting
`Content-Type` to the declared `produces` after the user steps; and
the final `SetHeaderIfAbsent` default-status step unchanged from
binding `json`.

#### Scenario: raw step sequence

- **GIVEN** a REST operation with `binding: raw`, `produces: image/png`,
  and a `to` target
- **WHEN** the block lowers
- **THEN** the step sequence is exactly `[ContentNegotiation(consumes,
  produces), To(target), SetHeader(Content-Type: image/png),
  SetHeaderIfAbsent(<verb default status>)]`

#### Scenario: compiled raw route adds no binding processor

- **GIVEN** a YAML REST POST operation with `binding: raw` and exactly
  one user `to` step
- **WHEN** the document parses, lowers, and compiles through the
  standard authoring path
- **THEN** the compiled route's step sequence has exactly four entries
  — the media negotiation step first, then the user `to` step, then
  the injected `Content-Type` and default-status steps — and the
  negotiation step compiles without a `StreamCacheService` wrap and
  without materializing the request stream, so no binding processor
  consumes bytes ahead of the user steps

#### Scenario: raw request body is not materialized before user steps

- **GIVEN** a lowered `binding: raw` route and an exchange whose body
  is the HTTP-provided `Body::Stream`
- **WHEN** the exchange enters the route pipeline
- **THEN** the first user step receives the exchange with the body
  variant the HTTP consumer installed (`Body::Stream`), because the
  leading negotiation step evaluates headers only and the lowering
  injected no unmarshal step to materialize it

### Requirement: v1 behavioral compatibility for omitted binding

A REST operation that omits `binding` SHALL lower to the same consumer
URI (`http://{host}:{port}{full_path}?httpMethod={VERB}`), the same
route id derivation, and the same defaults as before this change; the
step sequence gains exactly one leading media negotiation step. For
requests whose `Content-Type` satisfies the declared `consumes` and
whose `Accept` admits the declared `produces`, all REST behaviors
observable before this change SHALL remain unchanged — same statuses
and same response bytes. Requests that mismatch the declared media,
which previously failed late at unmarshal or succeeded with an
un-negotiated representation, SHALL fail fast per the L2 negotiation
requirements (415 / 406).

#### Scenario: pre-change route lowers with the negotiation prefix

- **GIVEN** a REST operation authored exactly as in v1 (no `binding`,
  `consumes` and `produces` `application/json`)
- **WHEN** the block lowers
- **THEN** the produced route has the identical `from` URI and route
  id as v1, and the step sequence is `[ContentNegotiation(consumes,
  produces), Unmarshal(json)?, To/Steps, Marshal(json),
  SetHeader(Content-Type: application/json), SetHeaderIfAbsent(<default
  status>)]`

#### Scenario: well-formed request behavior is unchanged

- **GIVEN** a v1-authored REST operation serving JSON
- **WHEN** a request arrives with a satisfying `Content-Type` and an
  `Accept` that admits the declared `produces`
- **THEN** the response status and bytes are identical to the
  pre-change behavior

#### Scenario: pin suites stay green as the internal regression net

- **GIVEN** the REST-related test suites present before this change,
  with their lowered-sequence assertions updated to include the
  negotiation prefix
- **WHEN** they run after this change
- **THEN** they pass, and no assertion other than the leading-step
  prefix required modification

## ADDED Requirements

### Requirement: REST request media enforcement

A lowered REST operation with a declared `consumes` SHALL reject, with
`CamelError::UnsupportedMediaType` mapped by the HTTP finalizer to
status 415, any request whose `Content-Type` header does not satisfy
the declaration. A `Content-Type` satisfies a declaration when the
`type/subtype` pair is equal ignoring case and parameters, or when
both the header value and the declaration are JSON-family media
(`application/json` or any `+json` suffix). Verbs without a request
body SHALL skip the request media check. An absent `Content-Type`
SHALL be permissive.

#### Scenario: unsupported request media type rejected with 415

- **GIVEN** a lowered REST POST operation declaring
  `consumes: application/json`
- **WHEN** a request arrives with `Content-Type: text/plain`
- **THEN** the pipeline fails with `CamelError::UnsupportedMediaType`
  carrying the consumed and declared values, and the HTTP finalizer
  replies with status 415 and a JSON error body

#### Scenario: parameters are ignored when matching Content-Type

- **GIVEN** a lowered REST operation declaring
  `consumes: application/json`
- **WHEN** a request arrives with
  `Content-Type: application/json; charset=utf-8`
- **THEN** the request media check passes and the pipeline continues

#### Scenario: structured syntax suffix satisfies a JSON declaration

- **GIVEN** a lowered REST operation declaring
  `consumes: application/json`
- **WHEN** a request arrives with
  `Content-Type: application/vnd.api+json`
- **THEN** the request media check passes, per the L1 JSON-essence
  rule

#### Scenario: body-less verbs skip the request media check

- **GIVEN** a lowered REST GET or DELETE operation declaring
  `consumes: application/json`
- **WHEN** the request carries a non-matching or absent `Content-Type`
- **THEN** the request media check does not reject the exchange

#### Scenario: malformed Content-Type under declared consumes is rejected with 415

- **GIVEN** a lowered REST operation with a body-ful verb declaring
  `consumes`
- **WHEN** the request carries a `Content-Type` that fails to parse as
  a concrete media type, including a wildcard value
- **THEN** the pipeline fails with `UnsupportedMediaType` and the HTTP
  finalizer replies with status 415

#### Scenario: Content-Type match is case-insensitive

- **GIVEN** a lowered REST operation declaring
  `consumes: application/json`
- **WHEN** a request arrives with `Content-Type: APPLICATION/JSON`
- **THEN** the request media check passes

### Requirement: REST response representation negotiation

A lowered REST operation with a declared `produces` SHALL reject, with
`CamelError::NotAcceptable` mapped by the HTTP finalizer to status
406, any request whose `Accept` header admits no acceptable entry for
the declaration. Entry matching SHALL follow media-range precedence:
among the entries that satisfy the declaration, the most specific one
governs — an exact `type/subtype` match over a JSON-essence match over
`type/*` over `*/*` — and the request is acceptable only when that
governing entry carries quality `q > 0`. When several entries of equal
governing specificity satisfy the declaration, the lowest quality
among them SHALL govern (fail closed). An `Accept` entry satisfies a
declaration when its `type/subtype` pair is equal ignoring case and
parameters, when both are JSON-family media, when the entry is `*/*`,
or when the entry is `type/*` with the same type as the declaration.
An absent `Accept` header SHALL be permissive. A malformed `Accept`
header SHALL be treated as `*/*`.

#### Scenario: no acceptable representation rejected with 406

- **GIVEN** a lowered REST operation declaring
  `produces: application/json`
- **WHEN** a request arrives with `Accept: application/xml`
- **THEN** the pipeline fails with `CamelError::NotAcceptable`
  carrying the accept and produced values, and the HTTP finalizer
  replies with status 406 and a JSON error body

#### Scenario: explicit quality zero rejects an otherwise matching entry

- **GIVEN** a lowered REST operation declaring
  `produces: application/json`
- **WHEN** a request arrives with `Accept: application/json;q=0`
- **THEN** the pipeline fails with `NotAcceptable` and the finalizer
  replies with status 406

#### Scenario: a more specific quality-zero entry outranks an accepting wildcard

- **GIVEN** a lowered REST operation declaring
  `produces: application/json`
- **WHEN** a request arrives with
  `Accept: application/json;q=0, */*;q=1`
- **THEN** the exact `type/subtype` entry outranks the `*/*` entry per
  media-range precedence, its `q=0` governs, and the finalizer replies
  with status 406

#### Scenario: duplicate equal-specificity entries fail closed

- **GIVEN** a lowered REST operation declaring
  `produces: application/json`
- **WHEN** a request arrives with
  `Accept: application/json;q=0, application/json;q=1`
- **THEN** the two entries tie at exact-match specificity, the lowest
  quality governs, and the finalizer replies with status 406

#### Scenario: quality defaults to one when absent

- **GIVEN** a lowered REST operation declaring
  `produces: application/json`
- **WHEN** a request arrives with `Accept: application/json` carrying
  no `q` parameter
- **THEN** the entry is acceptable and the pipeline continues

#### Scenario: wildcards accept any declaration

- **GIVEN** a lowered REST operation declaring a `produces` value
- **WHEN** a request arrives with `Accept: */*`
- **THEN** the negotiation passes; and WHEN a request arrives with
  `Accept: application/*` against `produces: application/json`
- **THEN** the negotiation also passes

#### Scenario: multiple Accept entries admit one match

- **GIVEN** a lowered REST operation declaring
  `produces: application/json`
- **WHEN** a request arrives with
  `Accept: text/html, application/xhtml+xml, application/json;q=0.9`
- **THEN** the negotiation passes, because the exact-match entry with
  `q > 0` governs

#### Scenario: Accept matching is case-insensitive and parameter-blind

- **GIVEN** a lowered REST operation declaring
  `produces: application/json`
- **WHEN** a request arrives with
  `Accept: APPLICATION/JSON; charset=utf-8`
- **THEN** the negotiation passes, because case and the charset
  parameter are ignored

#### Scenario: malformed Accept header is treated as permissive

- **GIVEN** a lowered REST operation declaring a `produces` value
- **WHEN** a request arrives with an `Accept` header that fails to
  parse as a media range list
- **THEN** the negotiation passes, treated as `Accept: */*`

#### Scenario: absent Accept header is permissive

- **GIVEN** a lowered REST operation declaring a `produces` value
- **WHEN** the request carries no `Accept` header
- **THEN** the negotiation passes and the pipeline continues

### Requirement: Negotiation pipeline position and body neutrality

The media negotiation step SHALL be the first step of every lowered
REST operation pipeline, preceding request unmarshal, and SHALL
evaluate exclusively exchange headers. It SHALL NOT poll, materialize,
re-wrap, cache, or replace the exchange body, and SHALL NOT alter
`StreamMetadata` or inject a `StreamCacheService`. The HTTP registry
and REST matching SHALL remain media-blind: `RestEndpoint` carries
method and path only, and no media field is added to the registry or
to `rest_match`. The L1 raw-binding and L3 streaming contract
scenarios SHALL remain green under this requirement.

#### Scenario: negotiation precedes unmarshal

- **GIVEN** a lowered REST POST operation with binding `json` and a
  request whose media both mismatches the declaration and carries a
  body
- **WHEN** the pipeline runs
- **THEN** the negotiation step fails the exchange before any
  unmarshal step executes

#### Scenario: negotiation never polls the request stream

- **GIVEN** a lowered REST operation whose exchange body is a stream
  that records every poll
- **WHEN** the negotiation step runs to completion
- **THEN** the poll recorder shows zero polls, the exchange body
  variant is unchanged, and its `StreamMetadata` is untouched

#### Scenario: registry stays media-blind

- **GIVEN** the camel-http REST routing types
- **WHEN** media negotiation is enforced for lowered REST operations
- **THEN** `RestEndpoint` still carries method and path only, and no
  media field is added to the registry or to `rest_match`

#### Scenario: L1 and L3 contracts remain green

- **GIVEN** the archived raw-binding and streaming contract test
  batteries
- **WHEN** the full test suite runs with negotiation enforced
- **THEN** every raw-binding and streaming scenario still passes
