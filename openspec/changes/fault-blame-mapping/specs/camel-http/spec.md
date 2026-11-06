# camel-http delta — fault-blame-mapping

Implements ADR-0096 fault blame for the HTTP consumer mapping.

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
`ConsumerStopping` keeps its `503 Service Unavailable` mapping; the
remaining fallback arm (every other error) maps to `500 Internal Server
Error`.

#### Scenario: Type conversion failure remains a bad request

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

### Requirement: Response-side marshal failures are server errors

The HTTP error finalizer SHALL map `CamelError::ProcessorErrorWithSource` to
status 500 with the fixed body `Internal Server Error` and no error detail in
the reply. The REST lowering raises that variant for a failed response-encode
step (see `rest-dsl`, response-side marshal failures), so a server-side
marshalling defect is never reported as a client error. The finalizer SHALL
map `CamelError::TypeConversionFailed` to status 500 with the
`conversion_failed` JSON body (ADR-0096 fault blame): a conversion failure
that the REST binding seam has not reclassified request-borne is a server or
upstream fault, and the injected response step no longer raises
`TypeConversionFailed`.

#### Scenario: processor error with source is a server error

- **GIVEN** `pipeline_error_to_reply` receives
  `CamelError::ProcessorErrorWithSource("response marshal failed (xml)", source)`
- **WHEN** it maps the error to an HTTP reply
- **THEN** the reply has status 500 and body `Internal Server Error`, and the
  reply body contains none of the message or source text
