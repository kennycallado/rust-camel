## MODIFIED Requirements

### Requirement: REST operation binding mode declaration

The REST DSL SHALL support an optional `binding` field on each REST operation
with the closed value set `json`, `raw`, `form`, and `xml`. A missing `binding`
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

#### Scenario: xml binding declared explicitly

- **GIVEN** a REST operation with `binding: xml`,
  `consumes: application/xml`, and `produces: application/xml`
- **WHEN** the document deserializes and the block lowers
- **THEN** lowering succeeds and the operation uses the XML binding
  pipeline

## ADDED Requirements

### Requirement: XML binding mode media acceptance

In binding `xml`, lowering SHALL accept a `consumes` or `produces` value whose
media base (after outer trimming and removal of `;`-separated parameters) is a
valid `type/subtype` whose subtype, compared ASCII case-insensitively, is
either `xml` or ends with `+xml` and has a non-empty prefix. The type token
SHALL NOT be restricted, so structured-suffix types of any type token are
accepted. At most one `;`-separated parameter SHALL be present, and it SHALL be
`charset=utf-8` (case-insensitive name and value, surrounding whitespace
trimmed). A value with another base, an empty parameter, more than one
parameter, a parameter other than `charset=utf-8`, or a quoted parameter value
SHALL fail route load with a `CamelError::RouteError` that names the operation,
the offending field, the offending value, and the accepted XML media family.

#### Scenario: application/xml accepted

- **GIVEN** a REST operation in binding `xml` with
  `consumes: application/xml` and `produces: application/xml`
- **WHEN** the block lowers
- **THEN** lowering succeeds

#### Scenario: text/xml accepted

- **GIVEN** a REST operation in binding `xml` with
  `consumes: text/xml` and `produces: text/xml`
- **WHEN** the block lowers
- **THEN** lowering succeeds

#### Scenario: structured XML suffix accepted

- **GIVEN** a REST operation in binding `xml` with
  `consumes: application/atom+xml` and `produces: application/atom+xml`
- **WHEN** the block lowers
- **THEN** lowering succeeds

#### Scenario: UTF-8 charset parameter accepted

- **GIVEN** a REST operation in binding `xml` with
  `consumes: application/xml; charset=utf-8`
- **WHEN** the block lowers
- **THEN** lowering succeeds

#### Scenario: JSON media fails with a precise error

- **GIVEN** a REST operation in binding `xml` with `consumes: application/json`
- **WHEN** the block lowers
- **THEN** lowering fails with a `RouteError` whose message contains the
  operation label, the string `consumes`, the offending value, and the
  accepted XML media family

#### Scenario: plain text media fails

- **GIVEN** a REST operation in binding `xml` with `produces: text/plain`
- **WHEN** the block lowers
- **THEN** lowering fails with a `RouteError` naming the operation and the
  offending value

#### Scenario: non-UTF-8 charset fails

- **GIVEN** a REST operation in binding `xml` with
  `produces: application/xml; charset=iso-8859-1`
- **WHEN** the block lowers
- **THEN** lowering fails with a `RouteError` naming the operation and the
  offending value

#### Scenario: empty prefix suffix fails

- **GIVEN** a REST operation in binding `xml` with
  `consumes: application/+xml`
- **WHEN** the block lowers
- **THEN** lowering fails with a `RouteError` naming the operation and the
  offending value

### Requirement: XML binding mode pipeline

In binding `xml`, lowering SHALL inject the same step sequence as binding
`json` except that the data format is `xml`: a leading media negotiation step
carrying the declared `consumes` and `produces`; for body-bearing verbs (POST,
PUT, PATCH) an `unmarshal: xml` step carrying `request_schema` when present;
user steps (`to` or `steps`); a `marshal: xml` step; a `SetHeader` step setting
`Content-Type` to the trimmed declared `produces`; and a final
`SetHeaderIfAbsent` step setting the default success status for the verb unless
`success_status` is declared. `request_schema` and `response.schema` SHALL be
accepted in binding `xml`. The XML body SHALL become `Body::Json` before schema
validation and before the first user step, so request validation is ordered
XML-unmarshal, then JSON, then schema-validate. The JSON projection SHALL follow
`xml_convert`: the root element becomes a single-key top-level object, attributes
become `@name` string keys, mixed text becomes `#text`, repeated child elements
become arrays, empty elements become `null`, `xmlns` declarations are dropped, and
leaf text is always a JSON string — authored schemas SHALL type leaves as strings.
A malformed XML body SHALL fail closed with a typed conversion error, which the
HTTP layer reports as 400. A `DOCTYPE` SHALL NOT be a parse error by itself, but
an entity reference outside the predefined set SHALL fail closed instead of being
expanded. An empty request body SHALL fail closed with a typed conversion error
before the first user step. The declared `response.schema` SHALL
remain declared metadata for OpenAPI generation; no runtime response validation
SHALL be added. The `marshal: xml` step SHALL require a top-level JSON object with
exactly one root element key; zero or multiple element keys, a top-level array, or
a top-level scalar SHALL fail closed with a typed conversion error. An empty
response body SHALL pass through the `marshal: xml` step unchanged, matching
binding `json`. OpenAPI generation SHALL treat binding `xml` exactly as binding
`json` for schema selection: request and response content keyed by the declared
XML media strings, authored schemas when present, and the same weak-stub `type:
object` fallback with warnings when absent.

#### Scenario: POST step sequence

- **GIVEN** a REST POST operation in binding `xml` with a `to` target
- **WHEN** the block lowers
- **THEN** the step sequence is exactly
  `[ContentNegotiation(consumes, produces), Unmarshal(xml),
  To(target), Marshal(xml), SetHeader(Content-Type: <produces>),
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
- **THEN** the unmarshal step returns `CamelError::TypeConversionFailed`
  without reaching the first user step

#### Scenario: entity reference fails closed

- **GIVEN** a lowered REST POST operation in binding `xml` and a request body
  carrying a `DOCTYPE` with an internal entity declaration and a reference to
  that entity
- **WHEN** the exchange enters the route pipeline
- **THEN** the unmarshal fails closed with a typed conversion error because the
  entity is not in the predefined set, and the declared replacement text never
  appears in any JSON projection

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
- **THEN** the unmarshal step returns `CamelError::TypeConversionFailed` and the
  first user step never runs

#### Scenario: malformed response JSON fails closed

- **GIVEN** a lowered REST operation in binding `xml` whose user steps leave a
  top-level JSON object with two element keys
- **WHEN** the exchange reaches the `marshal: xml` step
- **THEN** the step returns `CamelError::TypeConversionFailed`

#### Scenario: leaf text stays a JSON string

- **GIVEN** a lowered REST POST operation in binding `xml` and a request whose
  body contains an element with numeric text
- **WHEN** the exchange enters the route pipeline
- **THEN** the corresponding JSON leaf is a string, not a number

#### Scenario: empty response body stays empty

- **GIVEN** a lowered REST operation in binding `xml` whose user steps leave
  `Body::Empty`
- **WHEN** the exchange reaches the `marshal: xml` step
- **THEN** the step succeeds and the body remains `Body::Empty`

#### Scenario: OpenAPI parity with json

- **GIVEN** a REST POST operation in binding `xml` with
  `consumes: application/xml`, `produces: application/xml`, and a
  `request_schema`
- **WHEN** OpenAPI is generated
- **THEN** the request body content under `application/xml` carries the
  authored schema and the response content under `application/xml` carries
  the authored or weak-stub schema with the same warnings as binding `json`
