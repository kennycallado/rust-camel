# rest-dsl Delta

## MODIFIED Requirements

### Requirement: OpenAPI generation for REST bindings

OpenAPI generation SHALL key request body and response content by the
declared `consumes` and `produces` strings. In binding `json`, schemas come
from `request_schema`/`response.schema` with the existing weak-stub
`type: object` fallback and warnings. In binding `form`, request body and
response content SHALL use authored schemas with the same weak-stub
fallback and warnings exactly as binding `json`. In binding `raw`, request
and response content SHALL use a binary string schema (`type: string`,
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

#### Scenario: form binding uses authored schemas like json

- **GIVEN** a REST POST operation with `binding: form`,
  `consumes: application/x-www-form-urlencoded`,
  `produces: application/x-www-form-urlencoded`, and a `request_schema`
- **WHEN** OpenAPI is generated
- **THEN** the request body content under
  `application/x-www-form-urlencoded` carries the authored schema (not a
  binary schema), matching binding `json` behavior including the
  weak-stub fallback and warnings

### Requirement: REST operation binding mode declaration

The REST DSL SHALL support an optional `binding` field on each REST operation
with the closed value set `json`, `raw`, and `form`. A missing `binding`
SHALL mean `json`. An unknown `binding` value SHALL fail document
deserialization with an unknown-variant error. The effective binding mode
SHALL be determined only by this field; it SHALL NOT be inferred from
`consumes`, `produces`, or any media type.

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

#### Scenario: form binding declared explicitly

- **GIVEN** a REST operation with `binding: form`,
  `consumes: application/x-www-form-urlencoded`, and
  `produces: application/x-www-form-urlencoded`
- **WHEN** the document deserializes and the block lowers
- **THEN** lowering succeeds and the operation uses the form binding
  pipeline

## ADDED Requirements

### Requirement: Form binding mode media acceptance

In binding `form`, lowering SHALL accept a `consumes` or `produces` value
whose media base (after outer trimming and removal of `;`-separated
parameters) is `application/x-www-form-urlencoded`, case-insensitive. Any
`;`-separated parameter SHALL be at most one `charset` parameter whose value
is `utf-8`, case-insensitive. A value with any other base, any other
parameter, or a non-UTF-8 `charset` value SHALL fail route load with a
`CamelError::RouteError` that names the operation, the offending field and
value, and the expected media.

#### Scenario: exact form media loads

- **GIVEN** a REST operation in binding `form` with
  `consumes: application/x-www-form-urlencoded` and
  `produces: application/x-www-form-urlencoded`
- **WHEN** the block lowers
- **THEN** lowering succeeds

#### Scenario: charset parameter accepted

- **GIVEN** a REST operation in binding `form` with
  `consumes: application/x-www-form-urlencoded; charset=utf-8`
- **WHEN** the block lowers
- **THEN** lowering succeeds

#### Scenario: non-form media fails with a precise error

- **GIVEN** a REST operation in binding `form` with
  `consumes: application/json`
- **WHEN** the block lowers
- **THEN** lowering fails with a `RouteError` whose message contains the
  operation label, the string `consumes`, the offending value, and the
  expected form media type

#### Scenario: non-UTF-8 charset fails

- **GIVEN** a REST operation in binding `form` with
  `produces: application/x-www-form-urlencoded; charset=iso-8859-1`
- **WHEN** the block lowers
- **THEN** lowering fails with a `RouteError` naming the operation and the
  offending value

#### Scenario: request charset parameter is gate-opaque and fails at unmarshal

- **GIVEN** a lowered REST operation in binding `form` and a request whose
  `Content-Type` is
  `application/x-www-form-urlencoded; charset=iso-8859-1`
- **WHEN** the request passes the media negotiation gate and is unmarshalled
- **THEN** the gate accepts (parameters are opaque to it) and the
  `form` unmarshal fails closed on non-UTF-8 percent-decoded bytes (e.g.
  `%E9`)

### Requirement: Form binding mode pipeline

In binding `form`, lowering SHALL inject the same step sequence as binding
`json` except that the data format is `form`: a leading media negotiation
step carrying the declared `consumes` and `produces`; for body-bearing verbs
(POST, PUT, PATCH) an `unmarshal: form` step carrying `request_schema` when
present; user steps (`to` or `steps`); a `marshal: form` step; a `SetHeader`
step setting `Content-Type` to the trimmed declared `produces`; and a final
`SetHeaderIfAbsent` step setting the default success status for the verb
unless `success_status` is declared. `request_schema` and `response.schema`
SHALL be accepted in binding `form`: the form body becomes a JSON object
before schema validation, so validation SHALL run on the JSON produced by
the form unmarshal, ordered form-parse then schema-validate.

#### Scenario: POST step sequence

- **GIVEN** a REST POST operation in binding `form` with a `to` target
- **WHEN** the block lowers
- **THEN** the step sequence is exactly
  `[ContentNegotiation(consumes, produces), Unmarshal(form),
  To(target), Marshal(form), SetHeader(Content-Type: <produces>),
  SetHeaderIfAbsent(<verb default status>)]`

#### Scenario: request schema allowed and validated after form parse

- **GIVEN** a REST POST operation in binding `form` with a `request_schema`
- **WHEN** the block lowers and a request arrives
- **THEN** lowering succeeds, the unmarshal step carries the schema, and
  schema validation runs against the JSON object produced by the form
  unmarshal

#### Scenario: response schema allowed

- **GIVEN** a REST operation in binding `form` with a `response` containing
  a `schema`
- **WHEN** the block lowers
- **THEN** lowering succeeds

#### Scenario: GET receives no unmarshal

- **GIVEN** a REST GET operation in binding `form`
- **WHEN** the block lowers
- **THEN** the step sequence starts with the media negotiation step and
  contains no unmarshal step
