# camel-http Specification

## Purpose
TBD - created by archiving change jsonreply. Update Purpose after archive.
## Requirements
### Requirement: JSON error replies preserve existing behavior

The HTTP error finalizer SHALL construct JSON replies for
`TypeConversionFailed`, `ValidationError`, `UnsupportedMediaType`, and
`NotAcceptable` through one private helper without changing observable reply
behavior.

#### Scenario: Type conversion failure remains a bad request

- **GIVEN** `pipeline_error_to_reply` receives `TypeConversionFailed("bad")`
- **WHEN** it maps the error to an HTTP reply
- **THEN** the reply has status 400, `Content-Type: application/json`, and
  JSON fields `error: "bad_request"` and `message: "bad"`

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

### Requirement: HTTP maxInflightRequests upper bound

The system SHALL reject `maxInflightRequests` values above
`tokio::sync::Semaphore::MAX_PERMITS` with a typed configuration
error (`CamelError::Config`) that names the parameter, the configured
value, and the limit. The rejection SHALL occur at URI parse, at
`create_consumer`, and at consumer start before shared-server
registry interaction, listener binding, consumer envelope-channel
construction, or inflight-semaphore construction; `spawn_entry` SHALL
additionally validate before any listener side effect and before the
semaphore primitive as defense-in-depth. The system SHALL NOT panic
during route startup for any representable `maxInflightRequests`
value. The value `0` SHALL remain accepted with its existing
reject-everything (503) semantics; no zero normalization SHALL be
introduced.

#### Scenario: oversized value rejected at URI parse

- **GIVEN** an http consumer URI with `maxInflightRequests` greater than `tokio::sync::Semaphore::MAX_PERMITS`
- **WHEN** the URI is parsed into `HttpServerConfig` (including via `from_uri_with_defaults`)
- **THEN** parsing fails with a typed configuration error naming `maxInflightRequests`, the configured value, and the limit

#### Scenario: oversized value rejected at consumer creation even when constructed directly

- **GIVEN** an `HttpComponent` whose `HttpServerConfig` was constructed directly (not via URI parse) with `maxInflightRequests` greater than the limit
- **WHEN** `create_consumer` is invoked
- **THEN** it returns a typed configuration error before any `HttpConsumer` is constructed

#### Scenario: oversized value rejected at consumer start before side effects

- **GIVEN** an `HttpConsumer` constructed directly with `maxInflightRequests` greater than the limit
- **WHEN** `start` is invoked
- **THEN** it returns a typed configuration error before shared-server registry interaction, listener binding, envelope-channel construction, or semaphore construction, and no panic occurs

#### Scenario: defense-in-depth before the semaphore primitive

- **GIVEN** `spawn_entry` invoked with `max_inflight_requests` greater than the limit
- **WHEN** it runs
- **THEN** it returns a typed configuration error before any listener side effect and before `tokio::sync::Semaphore::new` is called, and no panic occurs

#### Scenario: boundary values

- **GIVEN** the limit `L = tokio::sync::Semaphore::MAX_PERMITS`
- **WHEN** bound validation runs with `L-1`, `L`, and `L+1`
- **THEN** `L-1` and `L` are accepted unchanged and `L+1` is rejected with a typed configuration error

#### Scenario: zero-value semantics retained

- **GIVEN** a consumer configured with `maxInflightRequests=0`
- **WHEN** it starts and receives a request
- **THEN** the request is rejected with HTTP 503 as before (rc-3y6j reject-everything semantics); validation does not normalize or reject the zero value

#### Scenario: accepted boundary is constructible in Tokio primitives

- **GIVEN** the limit `L = tokio::sync::Semaphore::MAX_PERMITS`
- **WHEN** a semaphore of `L` permits is constructed
- **THEN** construction succeeds without panicking (the accepted upper bound is exactly the primitive's bound)

