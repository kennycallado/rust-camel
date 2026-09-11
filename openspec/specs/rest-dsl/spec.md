# rest-dsl Specification

## Purpose
TBD - created by archiving change add-rest-raw-binding. Update Purpose after archive.
## Requirements
### Requirement: REST operation binding mode declaration

The REST DSL SHALL support an optional `binding` field on each REST operation
with the closed value set `json` and `raw`. A missing `binding` SHALL mean
`json`. An unknown `binding` value SHALL fail document deserialization with
an unknown-variant error. The effective binding mode SHALL be determined
only by this field; it SHALL NOT be inferred from `consumes`, `produces`, or
any media type.

#### Scenario: omitted binding defaults to json

- **GIVEN** a REST operation without a `binding` field
- **WHEN** the document deserializes and the block lowers
- **THEN** the operation behaves exactly as binding `json`

#### Scenario: invalid binding value fails at deserialize time

- **GIVEN** a REST operation with `binding: yaml`
- **WHEN** the document deserializes
- **THEN** deserialization fails with an unknown-variant error before any
  route lowering runs

#### Scenario: binding is not inferred from media type

- **GIVEN** a REST operation with `consumes: application/xml`,
  `produces: application/xml`, and no `binding` field
- **WHEN** the block lowers
- **THEN** lowering fails with the json-mode media error and does NOT
  silently treat the operation as raw

### Requirement: JSON binding mode media acceptance

In binding `json`, lowering SHALL accept a `consumes` or `produces` value
whose media essence is JSON: after outer trimming and removal of
`;`-separated parameters, the subtype (case-insensitive) is `json` or ends
with `+json`. Values whose essence is not JSON SHALL fail route load with a
`CamelError::RouteError` that names the operation, the offending field and
value, and the `binding: raw` remedy.

#### Scenario: parameterized JSON media loads

- **GIVEN** a REST operation in binding `json` with
  `consumes: application/json; charset=utf-8` and
  `produces: application/json; charset=utf-8`
- **WHEN** the block lowers
- **THEN** lowering succeeds

#### Scenario: JSON suffix media loads

- **GIVEN** a REST operation in binding `json` with
  `produces: application/problem+json`
- **WHEN** the block lowers
- **THEN** lowering succeeds

#### Scenario: non-JSON media in json mode fails with a precise error

- **GIVEN** a REST operation in binding `json` with
  `consumes: application/xml`
- **WHEN** the block lowers
- **THEN** lowering fails with a `RouteError` whose message contains the
  operation label, the string `consumes`, the offending value, and mentions
  `raw`

### Requirement: JSON binding mode pipeline

In binding `json`, lowering SHALL inject the v1 step sequence: for
body-bearing verbs (POST, PUT, PATCH) an `unmarshal: json` step carrying
`request_schema` when present; user steps (`to` or `steps`); a
`marshal: json` step; a `SetHeader` step setting `Content-Type` to the
trimmed declared `produces`; and a final `SetHeaderIfAbsent` step setting
the default success status for the verb unless `success_status` is declared.
Body-less verbs (GET, DELETE, HEAD, OPTIONS) SHALL NOT receive an unmarshal
step.

#### Scenario: POST step sequence

- **GIVEN** a REST POST operation in binding `json` with a `to` target
- **WHEN** the block lowers
- **THEN** the step sequence is exactly `[Unmarshal(json), To(target),
  Marshal(json), SetHeader(Content-Type: <produces>),
  SetHeaderIfAbsent(<verb default status>)]`

#### Scenario: GET receives no unmarshal

- **GIVEN** a REST GET operation in binding `json` with a `request_schema`
- **WHEN** the block lowers
- **THEN** the step sequence contains no unmarshal step

### Requirement: Raw binding mode media acceptance

In binding `raw`, lowering SHALL accept arbitrary declared `consumes` and
`produces` media strings, JSON or not, provided the value is a valid media
declaration: after outer trimming, it has the form `type "/" subtype`,
optionally followed by `;`-separated parameters, where `type` and `subtype`
are non-empty and consist only of RFC 9110 token characters (alphanumerics
and `` !#$%&'*+-.^_`|~ ``). Parameters are opaque and ignored by the check.
An invalid value SHALL fail route load with a precise `RouteError` naming
the operation, the offending field, and the value. The trimmed value is the
value lowering uses everywhere downstream: the injected response
`Content-Type` and the OpenAPI content key.

#### Scenario: non-JSON media loads in raw mode

- **GIVEN** a REST operation with `binding: raw`,
  `consumes: application/octet-stream`, and `produces: image/png`
- **WHEN** the block lowers
- **THEN** lowering succeeds

#### Scenario: media parameters are accepted in raw mode

- **GIVEN** a REST operation with `binding: raw` and
  `produces: text/plain; charset=utf-8`
- **WHEN** the block lowers
- **THEN** lowering succeeds (the parameter suffix is opaque)

#### Scenario: outer whitespace is trimmed, not rejected

- **GIVEN** a REST operation with `binding: raw` and
  `produces: " image/png "` (outer spaces)
- **WHEN** the block lowers
- **THEN** lowering succeeds, the injected `Content-Type` step value is
  exactly `image/png`, and OpenAPI generation keys the response content by
  `image/png`

#### Scenario: missing separator fails in raw mode

- **GIVEN** a REST operation with `binding: raw` and `produces: png`
- **WHEN** the block lowers
- **THEN** lowering fails with a `RouteError` naming the operation and the
  offending value

#### Scenario: empty type or subtype fails in raw mode

- **GIVEN** a REST operation with `binding: raw` and `produces: /png`
- **WHEN** the block lowers
- **THEN** lowering fails with a `RouteError` naming the operation and the
  offending value

#### Scenario: whitespace inside the base form fails in raw mode

- **GIVEN** a REST operation with `binding: raw` and
  `produces: image / png`
- **WHEN** the block lowers
- **THEN** lowering fails with a `RouteError` naming the operation and the
  offending value

### Requirement: Raw binding mode pipeline

In binding `raw`, lowering SHALL inject no automatic unmarshal step and no
automatic marshal step. The lowered step sequence SHALL be: user steps (`to`
or `steps`), a `SetHeader` step setting `Content-Type` to the declared
`produces` after the user steps, and the final `SetHeaderIfAbsent`
default-status step unchanged from binding `json`.

#### Scenario: raw step sequence

- **GIVEN** a REST operation with `binding: raw`, `produces: image/png`,
  and a `to` target
- **WHEN** the block lowers
- **THEN** the step sequence is exactly `[To(target),
  SetHeader(Content-Type: image/png), SetHeaderIfAbsent(<verb default
  status>)]`

#### Scenario: compiled raw route adds no binding processor

- **GIVEN** a YAML REST POST operation with `binding: raw` and exactly one
  user `to` step
- **WHEN** the document parses, lowers, and compiles through the standard
  authoring path
- **THEN** the compiled route's step sequence has exactly three entries —
  the user `to` step first, then the injected `Content-Type` and
  default-status steps — because the absence of an unmarshal step means no
  `StreamCacheService`-wrapped processor is compiled ahead of the user
  steps

#### Scenario: raw request body is not materialized before user steps

- **GIVEN** a lowered `binding: raw` route and an exchange whose body is
  the HTTP-provided `Body::Stream`
- **WHEN** the exchange enters the route pipeline
- **THEN** the first user step receives the exchange with the body variant
  the HTTP consumer installed (`Body::Stream`), since lowering injected no
  unmarshal step to materialize it

Note: single-consumption stream ownership, metadata preservation, and
reply-stream semantics are the streaming contract owned by the follow-up
L3 change and are deliberately not asserted here.

### Requirement: Raw binding schema rejection

In binding `raw`, a `request_schema` or a `response.schema` on the operation
SHALL fail route load with a precise `RouteError` explaining that schema
validation requires binding `json`. A `response` declaring only
`description` and/or `headers` SHALL be accepted in binding `raw`.

#### Scenario: request_schema with raw fails load

- **GIVEN** a REST operation with `binding: raw` and a `request_schema`
- **WHEN** the block lowers
- **THEN** lowering fails with a `RouteError` naming the operation and
  `request_schema`

#### Scenario: response.schema with raw fails load

- **GIVEN** a REST operation with `binding: raw` and a `response` containing
  a `schema`
- **WHEN** the block lowers
- **THEN** lowering fails with a `RouteError` naming the operation and
  `response.schema`

#### Scenario: header-only response with raw loads

- **GIVEN** a REST operation with `binding: raw` and a `response` containing
  only `headers`
- **WHEN** the block lowers
- **THEN** lowering succeeds

### Requirement: v1 byte-identity for omitted binding

A REST operation that omits `binding` SHALL lower to the same consumer URI
and the same step sequence as it did before this change: same `from` URI
(`http://{host}:{port}{full_path}?httpMethod={VERB}`), same injected steps,
same route id derivation, same defaults. All REST behaviors observable
before this change SHALL remain unchanged for such operations.

#### Scenario: pre-change route lowers identically

- **GIVEN** a REST operation authored exactly as in v1 (no `binding`,
  `consumes` and `produces` `application/json`)
- **WHEN** the block lowers
- **THEN** the produced route equals the v1 lowering: identical `from` URI,
  identical step sequence `[Unmarshal(json)?, To/Steps, Marshal(json),
  SetHeader(Content-Type: application/json), SetHeaderIfAbsent(<default
  status>)]`, and identical route id

#### Scenario: existing test pins stay green

- **GIVEN** the REST-related test suites present before this change
- **WHEN** they run after this change
- **THEN** they pass without modification of their assertions

### Requirement: OpenAPI generation for REST bindings

OpenAPI generation SHALL key request body and response content by the
declared `consumes` and `produces` strings. In binding `json`, schemas come
from `request_schema`/`response.schema` with the existing weak-stub
`type: object` fallback and warnings. In binding `raw`, request and response
content SHALL use a binary string schema (`type: string`,
`format: binary`), and the weak-stub warnings SHALL NOT be emitted. A raw
operation carrying `request_schema` or `response.schema` SHALL produce a
warning that route load rejects that combination. A 204 success response
SHALL remain contentless (no `content` key) in binding `raw` exactly as in
binding `json`, and the existing 204-ignores-`response.schema` warning
SHALL take precedence over the raw-schema warning.

#### Scenario: raw response emits binary schema

- **GIVEN** a REST operation with `binding: raw` and
  `produces: application/octet-stream`
- **WHEN** OpenAPI is generated
- **THEN** the success response content under `application/octet-stream`
  has schema `{"type": "string", "format": "binary"}` and no weak-stub
  warning is recorded

#### Scenario: raw request body emits binary schema

- **GIVEN** a REST POST operation with `binding: raw` and
  `consumes: text/plain`
- **WHEN** OpenAPI is generated
- **THEN** the request body content under `text/plain` has schema
  `{"type": "string", "format": "binary"}`

#### Scenario: raw with schema warns

- **GIVEN** a REST operation with `binding: raw` and a `response.schema`
- **WHEN** OpenAPI is generated
- **THEN** generation emits a warning that binding `raw` rejects schemas at
  route load, and the response still uses the binary schema

#### Scenario: raw 204 response stays contentless

- **GIVEN** a REST operation with `binding: raw`,
  `produces: application/octet-stream`, and `success_status: 204`
- **WHEN** OpenAPI is generated
- **THEN** the `204` response object carries no `content` key

#### Scenario: raw 204 with schema keeps the 204 warning only

- **GIVEN** a REST operation with `binding: raw`, `success_status: 204`,
  and a `response.schema`
- **WHEN** OpenAPI is generated
- **THEN** the recorded warning is the 204-ignores-`response.schema`
  warning, and no raw-schema warning is additionally stacked

### Requirement: REST binding authoring parity

The YAML and JSON authoring paths SHALL accept the `binding` field
identically: both deserialize the same REST structures and both lower REST
blocks through the same expansion helper, so a `binding: raw` operation
behaves the same regardless of authoring format.

#### Scenario: JSON-authored raw operation lowers like YAML-authored

- **GIVEN** equivalent REST blocks authored in YAML and in JSON with
  `binding: raw` and non-JSON media
- **WHEN** both documents parse and lower
- **THEN** both produce the same lowered route id, `from` URI, and step
  sequence

### Requirement: Raw binding stream ownership and metadata

In binding `raw`, the request `Body::Stream` SHALL reach the first user step
and every injected binding-independent step unconsumed: the lowered pipeline
SHALL NOT poll, materialize, re-wrap, or cache the request stream, and SHALL
NOT alter its `StreamMetadata`. The HTTP consumer SHALL record the request
`Content-Type` and `Content-Length`, when present, in the `StreamMetadata`
of the stream it installs on the exchange. A raw operation SHALL be free to
reply with the original request stream or with a new `Body::Stream`, and
both forms SHALL reach the wire intact under the response `Content-Type`
the route supplies.

#### Scenario: pipeline never polls the request stream

- **GIVEN** a lowered binding `raw` route and an exchange whose body is a
  stream that records every poll
- **WHEN** the compiled step sequence runs to completion
- **THEN** the poll recorder shows zero polls and the exchange body variant
  is still `Body::Stream`

#### Scenario: pipeline preserves stream identity

- **GIVEN** a lowered binding `raw` route, an exchange whose body is a
  stream, and a kept identity handle to that stream's mutex
- **WHEN** the compiled step sequence runs to completion
- **THEN** the stream mutex reachable from the exchange body is the same
  allocation as the identity handle, proving no replacement `StreamBody`
  was installed

#### Scenario: request stream stays single-consumption after the pipeline

- **GIVEN** a binding `raw` route whose compiled steps have run to
  completion over a one-chunk stream body
- **WHEN** the body is consumed once and then consumed again
- **THEN** the first consumption yields exactly the stream's bytes and the
  second fails with `CamelError::AlreadyConsumed`

#### Scenario: request stream metadata is preserved end-to-end

- **GIVEN** a binding `raw` route and an exchange whose stream metadata
  declares a content type and size hint
- **WHEN** the compiled step sequence runs to completion
- **THEN** the metadata is byte-for-byte unchanged, and independently, a
  real HTTP request carrying `Content-Type` and `Content-Length` arrives at
  the route boundary as `Body::Stream` whose metadata carries both values

#### Scenario: reply body may be the original or a new stream

- **GIVEN** a binding `raw` operation whose processing leaves the original
  request stream as the reply body, and another that replaces the reply
  body with a newly generated `Body::Stream`
- **WHEN** the HTTP reply is finalized for both
- **THEN** both responses deliver their full payload on the wire under the
  route-supplied response `Content-Type`

### Requirement: Raw binding stream error and limit contract

In binding `raw`, a second consumption attempt on a stream already consumed
during route processing SHALL fail with `CamelError::AlreadyConsumed` and
SHALL propagate as a pipeline error, never as a panic. A reply whose
`Body::Stream` is already consumed SHALL produce an HTTP 500 response with
an empty body. Request size limits SHALL fail closed: a request with
`Content-Length` over `max_request_body` SHALL be rejected 413 before the
stream opens, and a chunked request whose accumulated bytes exceed
`max_request_body` SHALL yield the cap error when the stream is consumed.
Response limits SHALL apply to materialized reply bytes only: an over-cap
materialized reply SHALL be replaced with an HTTP 500 carrying the message
`Response body exceeds configured limit`, while a streamed reply SHALL NOT
be byte-capped by `max_response_body`. A client disconnect during a streamed
reply SHALL NOT fail the consumer: subsequent requests on the same server
SHALL be served.

#### Scenario: in-route double consumption fails with AlreadyConsumed

- **GIVEN** a binding `raw` route processing an exchange whose request
  stream has already been fully consumed once during the route
- **WHEN** route processing consumes the stream a second time
- **THEN** the consumption fails with `CamelError::AlreadyConsumed` and the
  failure propagates as an error, not a panic

#### Scenario: consumed reply stream returns 500

- **GIVEN** a binding `raw` route whose reply body is a `Body::Stream`
  whose underlying stream was already consumed
- **WHEN** the HTTP reply is finalized
- **THEN** the response status is 500 and the response body is empty

#### Scenario: chunked request over the cap fails closed on consumption

- **GIVEN** a raw HTTP endpoint with `max_request_body` set to N and a
  chunked request (no `Content-Length`) whose total bytes exceed N
- **WHEN** the route consumes the request stream
- **THEN** consumption fails with the cap error naming the configured limit

#### Scenario: materialized reply bytes over the cap are replaced with 500

- **GIVEN** a raw HTTP endpoint with `max_response_body` set to N and a
  route that replies with materialized bytes longer than N
- **WHEN** the HTTP reply is finalized
- **THEN** the response status is 500 and the body is the message
  `Response body exceeds configured limit`

#### Scenario: streamed replies are not byte-capped

- **GIVEN** a raw HTTP endpoint with `max_response_body` set to N and a
  route that replies with a `Body::Stream` longer than N
- **WHEN** the HTTP reply is finalized
- **THEN** the response succeeds and delivers every stream byte

#### Scenario: client disconnect during a streamed reply does not fail the consumer

- **GIVEN** a raw HTTP endpoint streaming a reply that has delivered at
  least one chunk
- **WHEN** the client drops the connection mid-reply and a new request is
  sent to the same server
- **THEN** the new request is served successfully

