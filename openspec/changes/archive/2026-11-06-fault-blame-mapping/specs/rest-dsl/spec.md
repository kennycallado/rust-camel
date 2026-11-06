# rest-dsl delta — fault-blame-mapping

Implements ADR-0096 request-borne blame at the REST binding seam.

## ADDED Requirements

### Requirement: REST binding seam carries request-borne blame
The REST lowering SHALL mark the binding-injected unmarshal step as the
request-borne seam, and the step compiler SHALL raise
`CamelError::ValidationError` — not `CamelError::TypeConversionFailed` — when
that step's format conversion or schema check fails (ADR-0096: the binding is
the route author's declared request contract, so its failing-step input is by
definition the request). The seam marker SHALL NOT be authorable in YAML and
SHALL NOT change the rendered lowered step sequence. A user-authored
`unmarshal` step placed mid-route SHALL keep
`CamelError::TypeConversionFailed` and inherit the 500 default. The metric
label move at the seam (`type_conversion` → `validation`) SHALL be documented
and pinned by tests. An `on_exceptions` clause with
`kind: TypeConversionFailed` SHALL NOT match a binding-seam failure; a
`ValidationError` clause SHALL match it.

#### Scenario: binding seam unmarshal failure stays a bad request

- **GIVEN** a lowered REST POST operation in binding `json` and a malformed
  JSON request body
- **WHEN** the binding-injected unmarshal step fails
- **THEN** the step raises `CamelError::ValidationError` and the HTTP
  finalizer replies with status 400 and JSON field
  `error: "validation_error"`

#### Scenario: mid-route unmarshal failure is a server error

- **GIVEN** a REST operation whose user-authored steps include an `unmarshal`
  step that fails on pipeline data (not the request body)
- **WHEN** that unmarshal step fails
- **THEN** the step raises `CamelError::TypeConversionFailed` and the HTTP
  finalizer replies with status 500 (the seam distinction, not step
  direction)

#### Scenario: binding seam schema mismatch is unchanged

- **GIVEN** a REST operation carrying a `request_schema` and a request body
  that parses but violates the schema
- **WHEN** the schema check fails
- **THEN** the step raises `CamelError::ValidationError` and the HTTP
  finalizer replies with status 400, as before this change

#### Scenario: metric labels move only at the seam

- **GIVEN** `CamelError::classify()` over the variants the change touches
- **WHEN** the classification is asserted
- **THEN** `TypeConversionFailed` classifies `type_conversion`,
  `ValidationError` classifies `validation`, and a binding-seam failure is
  counted under `validation` where it previously counted `type_conversion`

#### Scenario: error handler kinds at the seam

- **GIVEN** an ingress REST route with an `on_exceptions` clause
- **WHEN** the binding-injected unmarshal fails on a malformed body
- **THEN** a `kind: TypeConversionFailed` clause does not match and a
  `kind: ValidationError` clause matches; a mid-route `unmarshal` failure
  still matches `kind: TypeConversionFailed`

## MODIFIED Requirements

### Requirement: XML binding mode pipeline

In binding `xml`, lowering SHALL inject the same step sequence as binding
`json` except that the data format is `xml`: a leading media negotiation step
carrying the declared `consumes` and `produces`; for body-bearing verbs (POST,
PUT, PATCH) an `unmarshal: xml` step carrying `request_schema` when present;
user steps (`to` or `steps`); an internal response-marshal step for the `xml`
data format (rendered `ResponseMarshal` in the lowered AST and not authorable
in YAML; an authored `marshal: xml` step still renders `Marshal`); a
`SetHeader` step setting `Content-Type` to the trimmed declared `produces`;
and a final `SetHeaderIfAbsent` step setting the default success status for
the verb unless `success_status` is declared. The binding-injected unmarshal
step is the request-borne seam (see `rest-dsl`, REST binding seam carries
request-borne blame): a failure of its format conversion or schema check
SHALL raise `CamelError::ValidationError` at the seam and keep the 400
mapping through the `validation_error` reply, while the XML data format's own
raise sites keep `CamelError::TypeConversionFailed` unchanged.
`request_schema` and `response.schema` SHALL be accepted in binding `xml`.
The XML body SHALL become `Body::Json` before schema validation and before
the first user step, so request validation is ordered XML-unmarshal, then
JSON, then schema-validate. The JSON projection SHALL follow `xml_convert`:
the root element becomes a single-key top-level object, attributes become
`@name` string keys, mixed text becomes `#text`, repeated child elements
become arrays, empty elements become `null`, `xmlns` declarations are
dropped, and leaf text is always a JSON string — authored schemas SHALL type
leaves as strings. A malformed XML body SHALL fail closed at the seam before
the first user step, reported by the HTTP layer as 400. A `DOCTYPE` SHALL NOT
be a parse error by itself, but an entity reference outside the predefined
set SHALL fail closed instead of being expanded. An empty request body SHALL
fail closed at the seam before the first user step. The declared
`response.schema` SHALL remain declared metadata for OpenAPI generation; no
runtime response validation SHALL be added. The injected response-marshal
step SHALL require a top-level JSON object with exactly one root element key;
zero or multiple element keys, a top-level array, or a top-level scalar SHALL
fail closed with `ProcessorErrorWithSource` wrapping a `TypeConversionFailed`
(raised `ProcessorErrorWithSource`, reported by the HTTP finalizer as 500; a
user-authored `marshal:` or mid-route `unmarshal` step keeps
`TypeConversionFailed`, reported as 500 under ADR-0096). An empty response
body SHALL pass through the response-marshal step unchanged, matching binding
`json`. A user step may produce `Body::Xml` or valid-UTF-8 `Body::Bytes`; the
response-marshal step SHALL pass those bodies through unchanged (see
`data-formats`, XML marshal body acceptance). OpenAPI generation SHALL treat
binding `xml` exactly as binding `json` for schema selection: request and
response content keyed by the declared XML media strings, authored schemas
when present, and the same weak-stub `type: object` fallback with warnings
when absent.

#### Scenario: POST step sequence

- **GIVEN** a REST POST operation in binding `xml` with a `to` target
- **WHEN** the block lowers
- **THEN** the step sequence is exactly
  `[ContentNegotiation(consumes, produces), Unmarshal(xml),
  To(target), ResponseMarshal(xml), SetHeader(Content-Type: <produces>),
  SetHeaderIfAbsent(<verb default status>)]`

#### Scenario: GET receives no unmarshal

- **GIVEN** a REST GET operation in binding `xml`
- **WHEN** the block lowers
- **THEN** the step sequence starts with the media negotiation step and
  contains no unmarshal step

#### Scenario: request schema allowed and validated after XML unmarshal

- **GIVEN** a REST POST operation in binding `xml` with a `request_schema`
- **WHEN** the block lowers and a well-formed XML request arrives
- **THEN** lowering succeeds, the unmarshal step carries the schema, and
  schema validation runs against the JSON produced by the XML unmarshal

#### Scenario: response schema allowed

- **GIVEN** a REST operation in binding `xml` with a `response` containing
  a `schema`
- **WHEN** the block lowers
- **THEN** lowering succeeds and the schema is used by OpenAPI generation only

#### Scenario: XML request reaches user steps as JSON

- **GIVEN** a lowered REST POST operation in binding `xml` and a request whose
  body is a well-formed XML document
- **WHEN** the exchange enters the route pipeline
- **THEN** the first user step receives `Body::Json` containing the JSON
  projection of the document

#### Scenario: JSON response leaves as XML with declared media

- **GIVEN** a lowered REST operation in binding `xml` whose user steps leave a
  `Body::Json` object with exactly one root element
- **WHEN** the exchange reaches the response steps
- **THEN** the body is XML text and the `Content-Type` header equals the
  trimmed declared `produces`

#### Scenario: malformed XML fails closed

- **GIVEN** a lowered REST POST operation in binding `xml` and a request body
  that is not well-formed XML
- **WHEN** the exchange enters the route pipeline
- **THEN** the binding-injected unmarshal step fails closed at the seam with
  `CamelError::ValidationError` — reclassified from the XML format's
  `TypeConversionFailed` — without reaching the first user step

#### Scenario: entity reference fails closed

- **GIVEN** a lowered REST POST operation in binding `xml` and a request body
  carrying a `DOCTYPE` with an internal entity declaration and a reference to
  that entity
- **WHEN** the exchange enters the route pipeline
- **THEN** the unmarshal fails closed — the XML format raises its typed
  conversion error because the entity is not in the predefined set, the seam
  reclassifies it to `CamelError::ValidationError`, and the declared
  replacement text never appears in any JSON projection

#### Scenario: DOCTYPE without entity references is not materialized

- **GIVEN** a lowered REST POST operation in binding `xml` and a request body
  carrying a `DOCTYPE` with no entity reference
- **WHEN** the exchange enters the route pipeline
- **THEN** the unmarshal succeeds and the JSON projection contains no key
  derived from the DTD

#### Scenario: empty request body fails closed

- **GIVEN** a lowered REST POST operation in binding `xml` and a request whose
  body is `Body::Empty`
- **WHEN** the exchange enters the route pipeline
- **THEN** the binding-injected unmarshal step fails closed at the seam with
  `CamelError::ValidationError` and the first user step never runs

#### Scenario: malformed response JSON fails closed

- **GIVEN** a lowered REST operation in binding `xml` whose user steps leave a
  top-level JSON object with two element keys
- **WHEN** the exchange reaches the response-marshal step
- **THEN** the step returns `CamelError::ProcessorErrorWithSource` whose source
  is the original `CamelError::TypeConversionFailed`

#### Scenario: leaf text stays a JSON string

- **GIVEN** a lowered REST POST operation in binding `xml` and a request whose
  body contains an element with numeric text
- **WHEN** the exchange enters the route pipeline
- **THEN** the corresponding JSON leaf is a string, not a number

#### Scenario: empty response body stays empty

- **GIVEN** a lowered REST operation in binding `xml` whose user steps leave
  `Body::Empty`
- **WHEN** the exchange reaches the response-marshal step
- **THEN** the step succeeds and the body remains `Body::Empty`

#### Scenario: OpenAPI parity with json

- **GIVEN** a REST POST operation in binding `xml` with
  `consumes: application/xml`, `produces: application/xml`, and a
  `request_schema`
- **WHEN** OpenAPI is generated
- **THEN** the request body content under `application/xml` carries the
  authored schema and the response content under `application/xml` carries
  the authored or weak-stub schema with the same warnings as binding `json`

### Requirement: v1 behavioral compatibility for omitted binding

A REST operation that omits `binding` SHALL lower to the same consumer
URI (`http://{host}:{port}{full_path}?httpMethod={VERB}`), the same
route id derivation, and the same defaults as before this change; the
step sequence gains exactly one leading media negotiation step. For
successful requests, and for failures not touched by ADR-0096 fault
blame, all REST behaviors observable before this change SHALL remain
unchanged — same statuses and same response bytes. ADR-0096 authorizes
exactly two observable changes for pre-existing failure modes:
non-request-borne `TypeConversionFailed` moves from 400/`bad_request` to
500/`conversion_failed`, and binding-seam conversion failures retain
status 400 while their error identity changes to
`ValidationError`/`validation_error`. Requests that mismatch the declared
media, which previously failed late at unmarshal or succeeded with an
un-negotiated representation, SHALL fail fast per the L2 negotiation
requirements (415 / 406). The response-side marshal failure change
(400 to 500) authorized by its own requirement remains in force. The
documented error-handler kind consequences and metric-label move at the
binding seam (ADR-0096 Consequences) are part of the intended change
surface.

#### Scenario: pre-change route lowers with the negotiation prefix

- **GIVEN** a REST operation authored exactly as in v1 (no `binding`,
  `consumes` and `produces` `application/json`)
- **WHEN** the block lowers
- **THEN** the produced route has the identical `from` URI and route
  id as v1, and the step sequence is `[ContentNegotiation(consumes,
  produces), Unmarshal(json)?, To/Steps, ResponseMarshal(json),
  SetHeader(Content-Type: application/json), SetHeaderIfAbsent(<default
  status>)]`

#### Scenario: well-formed request behavior is unchanged

- **GIVEN** a v1-authored REST operation serving JSON whose response-marshal
  step succeeds
- **WHEN** a request arrives with a satisfying `Content-Type` and an
  `Accept` that admits the declared `produces`
- **THEN** the response status and bytes are identical to the
  pre-change behavior

#### Scenario: pin suites stay green as the internal regression net

- **GIVEN** the REST-related test suites present before this change,
  with their lowered-sequence assertions updated for the internal
  response-marshal step
- **WHEN** they run after this change
- **THEN** they pass, and no behavior assertion other than the
  leading-step prefix, the response-step identity, the response-marshal
  status change (400 to 500), the ADR-0096 fault-blame mapping changes
  (non-request-borne conversion failures 400/`bad_request` to
  500/`conversion_failed`; binding-seam conversion failures reclassified
  to `ValidationError`/`validation_error` at status 400), and the
  documented handler-kind and metric-label consequences required
  modification

### Requirement: Response-side marshal failures are server errors

In every non-`raw` binding, the response-encode step injected by lowering
SHALL be an internal step not authorable in YAML. A failure raised by that
step SHALL be `CamelError::ProcessorErrorWithSource` carrying the original
data-format error as its source, so the HTTP finalizer reports status 500
with the generic `Internal Server Error` body and no error detail leaks to
the client. The binding-injected request unmarshal SHALL raise
`CamelError::ValidationError` at the seam (see `rest-dsl`, REST binding seam
carries request-borne blame) and keeps the 400 mapping through the
`validation_error` reply. A user-authored `marshal:` or mid-route `unmarshal`
step SHALL keep its existing `TypeConversionFailed` error identity, which the
HTTP finalizer reports as 500 (ADR-0096 fault blame). Because the
response-side failure is raised as `ProcessorErrorWithSource`, an
`on_exceptions` clause written with `kind: TypeConversionFailed` SHALL NOT
match a REST response-marshal failure; `kind: ProcessorErrorWithSource` SHALL
match it. The same identity change applies to `do_try` catch-by-variant
(`variant_name()` aliases the variant to `ProcessorError`, so a
`TypeConversionFailed` catch no longer matches while a `ProcessorError` catch
does) and to error trace and metric labels, which now report the processor
family. An authored `marshal:` step placed inside a REST operation's `steps`
is not the injected response step: it SHALL keep `TypeConversionFailed` and
is reported as 500. Raw binding injects no response-encode step, so its
behavior is unchanged.

#### Scenario: json response marshal failure is a server error

- **GIVEN** a lowered REST operation in binding `json` whose user steps leave
  `Body::Stream` (the JSON data format cannot marshal a stream)
- **WHEN** the exchange reaches the response-marshal step and fails
- **THEN** the step returns `CamelError::ProcessorErrorWithSource` and the HTTP
  finalizer replies with status 500 and body `Internal Server Error`

#### Scenario: form response marshal failure is a server error

- **GIVEN** a lowered REST operation in binding `form` whose user steps leave a
  top-level JSON array or scalar (the form data format only marshals a JSON
  object)
- **WHEN** the exchange reaches the response-marshal step and fails
- **THEN** the step returns `CamelError::ProcessorErrorWithSource` and the HTTP
  finalizer replies with status 500

#### Scenario: xml response marshal failure is a server error

- **GIVEN** a lowered REST operation in binding `xml` whose user steps leave
  non-UTF-8 `Body::Bytes`
- **WHEN** the exchange reaches the response-marshal step and fails
- **THEN** the step returns `CamelError::ProcessorErrorWithSource` and the HTTP
  finalizer replies with status 500

#### Scenario: request-side conversion stays a bad request

- **GIVEN** a lowered REST POST operation in binding `xml` and a malformed XML
  request body
- **WHEN** the binding-injected unmarshal step fails
- **THEN** the step raises `CamelError::ValidationError` at the seam and the
  HTTP finalizer replies with status 400 and JSON field
  `error: "validation_error"`

#### Scenario: authored marshal step keeps its conversion error

- **GIVEN** a route with an authored `marshal: xml` step and a
  `Body::Json({"a": 1, "b": 2})` body (a top-level object with two root keys)
- **WHEN** the step fails
- **THEN** it returns `CamelError::TypeConversionFailed`, not
  `ProcessorErrorWithSource`; the same variant is kept and the accepted inputs
  follow `data-formats`, XML marshal body acceptance

#### Scenario: authored marshal failure is a server error

- **GIVEN** a REST operation with an authored `marshal: xml` step that fails
  on a two-root-key JSON object
- **WHEN** the HTTP finalizer maps the failure
- **THEN** the reply has status 500 and JSON field
  `error: "conversion_failed"`

#### Scenario: XML user step body is returned unchanged

- **GIVEN** a lowered REST operation in binding `xml` whose user steps leave
  `Body::Xml("<user><name>kenny</name></user>")`
- **WHEN** the exchange reaches the response-marshal step
- **THEN** the step succeeds and the reply body is exactly those XML bytes, and
  the explicit `Content-Type` set by lowering to the trimmed declared
  `produces` overrides the finalizer's inferred `application/xml` (including
  for `produces: application/vnd.x+xml`)
