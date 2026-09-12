## ADDED Requirements

### Requirement: Producer injected header construction surfacing

When construction of a producer-injected header (`userAgent` configuration,
configured authentication credentials, or injected trace-context headers)
as a `HeaderName`/`HeaderValue` pair fails because the name or value is
invalid, the HTTP producer SHALL omit that header from the outbound request,
SHALL emit a DEBUG drop record identifying the header name and the failure
reason, and SHALL proceed with the request. The drop record SHALL NOT
contain the header value bytes. Valid configured headers SHALL be injected
with no drop records and no behavioural change. The connection-close
injection uses a statically valid literal and cannot fail at runtime.

#### Scenario: invalid configured user-agent surfaced

- **GIVEN** a producer whose configured `userAgent` value fails `HeaderValue`
  construction (contains an invalid byte)
- **WHEN** the producer builds and sends the outbound request
- **THEN** the request is sent without a `User-Agent` derived from the
  configuration, a DEBUG drop record naming the user-agent header and the
  invalid-value reason is emitted, and the request completes

#### Scenario: invalid configured bearer token surfaced

- **GIVEN** a producer configured with Bearer authentication whose token
  fails `HeaderValue` construction (contains an invalid byte)
- **WHEN** the producer builds and sends the outbound request
- **THEN** the request is sent without an `Authorization` header derived from
  the token, a DEBUG drop record naming the authorization header and the
  invalid-value reason is emitted, and the request completes

#### Scenario: invalid injected header pair surfaced

- **GIVEN** an injected header pair (from configuration or trace-context)
  whose name or value fails construction as a `HeaderName`/`HeaderValue`
- **WHEN** the producer routes the pair through its shared
  header-construction path
- **THEN** the pair is omitted, a DEBUG drop record naming the header and
  the invalid-name or invalid-value reason is emitted, and the request
  proceeds

#### Scenario: drop records never carry values

- **GIVEN** any injected-header construction failure
- **WHEN** the DEBUG drop record is emitted
- **THEN** the record contains the header name and reason only, with no
  header value bytes

#### Scenario: valid configuration unchanged

- **GIVEN** a producer with a valid `userAgent` and valid credentials
- **WHEN** the producer builds and sends the outbound request
- **THEN** both headers are present on the captured request and no drop
  records are emitted for them
