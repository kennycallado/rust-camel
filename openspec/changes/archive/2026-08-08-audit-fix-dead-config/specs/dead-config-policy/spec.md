## ADDED Requirements

### Requirement: No silently ignored config fields

The system SHALL NOT parse a config field from URI parameters or TOML configuration unless the field is consumed by runtime logic. When a removed parameter is present in a URI, the system SHALL reject it with an error indicating the parameter is not supported.

**Exception:** A field may be retained on a config struct for serde deserialization compatibility if `validate()` rejects any non-default value with an explicit error (fail-closed). Such a field is not considered silently ignored because the operator receives an error at validation time.

#### Scenario: Removed xj transformDirection rejected

- **GIVEN** a `camel-xj` endpoint URI with `transformDirection=XML2JSON`
- **WHEN** the URI is parsed
- **THEN** parsing fails with an error indicating `transformDirection` is not supported (use `direction` instead)

#### Scenario: Removed xj resourceUri rejected

- **GIVEN** a `camel-xj` endpoint URI with `resourceUri=classpath:extra.xslt`
- **WHEN** the URI is parsed
- **THEN** parsing fails with an error indicating `resourceUri` is not supported

#### Scenario: Removed http cookieHandling rejected

- **GIVEN** a `camel-http` endpoint URI with `cookieHandling=InMemory`
- **WHEN** the URI is parsed
- **THEN** parsing fails with an error indicating `cookieHandling` is not supported

#### Scenario: Removed direct block rejected

- **GIVEN** a `camel-direct` endpoint URI with `block=true`
- **WHEN** the URI is parsed
- **THEN** parsing fails with an error indicating `block` is not supported

#### Scenario: Removed direct exchange_pattern rejected

- **GIVEN** a `camel-direct` endpoint URI with `exchange_pattern=InOnly`
- **WHEN** the URI is parsed
- **THEN** parsing fails with an error indicating `exchange_pattern` is not supported

#### Scenario: Removed direct exchangePattern (camelCase) rejected

- **GIVEN** a `camel-direct` endpoint URI with `exchangePattern=InOnly`
- **WHEN** the URI is parsed
- **THEN** parsing fails with an error indicating `exchangePattern` is not supported

### Requirement: proxy_url validation rejection

The system SHALL retain the `proxy_url` field on `HttpConfig` for serde deserialization compatibility but SHALL reject any non-None value at validation time with an SSRF-specific error.

#### Scenario: proxy_url set to valid URL rejected at validation

- **GIVEN** an `HttpConfig` with `proxy_url` set to `Some("http://proxy:8080")`
- **WHEN** `validate()` is called
- **THEN** validation fails with an error stating proxy_url is incompatible with SSRF DNS pinning

#### Scenario: proxy_url None passes validation

- **GIVEN** an `HttpConfig` with `proxy_url` set to `None`
- **WHEN** `validate()` is called
- **THEN** validation succeeds

#### Scenario: proxy_url from TOML rejected at validation

- **GIVEN** TOML config with `proxy_url = "http://proxy:8080"` (deserializes successfully because the field is retained)
- **WHEN** `validate()` is called
- **THEN** validation fails with the SSRF incompatibility error

### Requirement: WebSocket send timeout enforcement (client mode)

The system SHALL enforce `send_timeout` on the client-mode WebSocket send path (`ws_stream.send`). Server-send mode (internal mpsc `try_send_with_backpressure`) SHALL NOT be affected. When the timeout elapses before the client-mode send completes, the system SHALL return an error.

#### Scenario: Client send completes within timeout

- **GIVEN** a `camel-ws` client endpoint with `sendTimeoutMs=5000`
- **WHEN** a message is sent via `ws_stream.send` and the sink accepts it within 5 seconds
- **THEN** the send succeeds with no error

#### Scenario: Client send exceeds timeout

- **GIVEN** a `camel-ws` client endpoint with `sendTimeoutMs=100`
- **WHEN** a message is sent via `ws_stream.send` and the sink does not accept it within 100 milliseconds
- **THEN** the send returns a timeout error

#### Scenario: Default send timeout

- **GIVEN** a `camel-ws` endpoint with no `sendTimeoutMs` specified
- **WHEN** the endpoint configuration is resolved
- **THEN** the default send timeout is 30 seconds
