## ADDED Requirements

### Requirement: Typed auth-provider unavailability propagation

The system SHALL propagate auth-provider unavailability (JWKS, introspection, or token
endpoint unreachable/failing) as a typed `CamelError::AuthProviderUnavailable` variant
across the kernel boundary, and the WebSocket and gRPC transports SHALL select the denial
status by matching that variant — never by inspecting the error message text.

#### Scenario: Typed mapping at the kernel boundary

- **GIVEN** an `AuthError::ProviderUnavailable(detail)` raised inside camel-auth
- **WHEN** it is converted into `CamelError` via the `From` impl
- **THEN** the result is `CamelError::AuthProviderUnavailable` carrying the detail, not a
  `ProcessorError` with an embedded magic string

#### Scenario: WebSocket denial maps to 503 by variant

- **GIVEN** WebSocket handshake authentication fails with `CamelError::AuthProviderUnavailable`
- **WHEN** the upgrade is rejected by `ws_upgrade_auth_error`
- **THEN** the HTTP response status is 503 Service Unavailable, selected by variant match
  with no string inspection of the error message

#### Scenario: gRPC denial maps to UNAVAILABLE by variant

- **GIVEN** gRPC per-request authentication fails with `CamelError::AuthProviderUnavailable`
- **WHEN** the error is mapped by `auth_error_to_status`
- **THEN** the `tonic::Status` code is `unavailable`, selected by variant match with no
  string inspection of the error message

#### Scenario: Wording independence

- **GIVEN** an `AuthProviderUnavailable` whose detail text is arbitrary (including text
  that does not contain any fixed marker substring)
- **WHEN** the error reaches the WebSocket or gRPC transport's denial mapping
- **THEN** the status is still 503 / UNAVAILABLE — a wording change alone can never
  degrade the denial to 500 / INTERNAL

#### Scenario: Error-handler catch compatibility

- **GIVEN** a `CamelError::AuthProviderUnavailable` flowing through route error handling
- **WHEN** `doTry` catch-by-variant matching consults `CamelError::variant_name()` and
  the error is classified via `CamelError::classify()`
- **THEN** `variant_name()` reports `"ProcessorError"` (same aliasing as
  `ProcessorErrorWithSource`) and `classify()` reports `"processor"`, so existing
  ProcessorError catch handlers keep matching exactly as before the change

#### Scenario: Other processor errors keep the internal status

- **GIVEN** authentication fails with a generic `CamelError::ProcessorError` unrelated to
  provider availability
- **WHEN** the error reaches the WebSocket or gRPC transport's denial mapping
- **THEN** the status is 500 Internal Server Error / INTERNAL as before
