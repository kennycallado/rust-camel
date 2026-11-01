## ADDED Requirements

### Requirement: Response-side marshal failures are server errors

The HTTP error finalizer SHALL map `CamelError::ProcessorErrorWithSource` to
status 500 with the fixed body `Internal Server Error` and no error detail in
the reply. The REST lowering raises that variant for a failed response-encode
step (see `rest-dsl`, response-side marshal failures), so a server-side
marshalling defect is never reported as a client error. The finalizer SHALL
keep mapping `CamelError::TypeConversionFailed` to status 400 with the
`bad_request` JSON body for request-side conversion failures; that mapping is
not widened to response-side failures because the response step no longer
raises `TypeConversionFailed`.

#### Scenario: processor error with source is a server error

- **GIVEN** `pipeline_error_to_reply` receives
  `CamelError::ProcessorErrorWithSource("response marshal failed (xml)", source)`
- **WHEN** it maps the error to an HTTP reply
- **THEN** the reply has status 500 and body `Internal Server Error`, and the
  reply body contains none of the message or source text
