## ADDED Requirements

### Requirement: Producer method/body coupling

The HTTP producer SHALL attach the exchange body to the outbound request
only when the resolved method is entity-enclosing. The entity-enclosing set
is POST, PUT, and PATCH. GET, HEAD, DELETE, OPTIONS, and TRACE SHALL NOT
carry a request body. When a body is suppressed for a non-entity-enclosing method, the producer
SHALL emit exactly one `warn!` log entry that names the resolved method and
the correlation id. A stream body always warns, because its emptiness
cannot be known without consuming it. The exchange body stays consumed; the
producer does not restore it. No configuration option overrides this
behaviour. This matches Apache Camel `HttpMethods.isEntityEnclosing`
semantics and RFC 9110 section 9.3.1 guidance.

#### Scenario: GET with non-empty body sends no body

- **GIVEN** an exchange with a non-empty body and `httpMethod=GET` on the
  producer
- **WHEN** the producer sends the request
- **THEN** the outbound request has no body and carries no Content-Length
  or Transfer-Encoding header
- **AND** the exchange body is consumed

#### Scenario: HEAD with body suppressed

- **GIVEN** an exchange with a non-empty body and header
  `CamelHttpMethod: HEAD`
- **WHEN** the producer sends the request
- **THEN** the outbound request has no body

#### Scenario: DELETE, OPTIONS, TRACE bodies suppressed

- **GIVEN** an exchange with a non-empty body and the producer method set
  to DELETE, OPTIONS, or TRACE
- **WHEN** the producer sends the request
- **THEN** the outbound request has no body for each of the three methods

#### Scenario: entity-enclosing methods keep the body

- **GIVEN** an exchange with a non-empty body and the producer method set
  to POST, PUT, or PATCH
- **WHEN** the producer sends the request
- **THEN** the outbound request carries the exchange body

#### Scenario: suppressed body logs one warning

- **GIVEN** an exchange with a non-empty body and `httpMethod=GET`
- **WHEN** the producer suppresses the body
- **THEN** exactly one `warn!` entry is emitted and names GET
- **AND** an empty-body GET emits no warning

#### Scenario: suppressed body stays consumed

- **GIVEN** an exchange with a body (bytes or `Body::Stream`) and
  `httpMethod=GET`
- **WHEN** the producer sends the request
- **THEN** the exchange body is consumed after the send, for both the bytes
  and the stream form

#### Scenario: redirect hops never replay a suppressed body

- **GIVEN** an exchange with a non-empty body, `httpMethod=GET`, and a
  producer that follows redirects through a 307 and a 308 hop
- **WHEN** the producer completes the redirect chain
- **THEN** no hop carries a request body

#### Scenario: stream body under GET not attached

- **GIVEN** an exchange whose body is a `Body::Stream` and the producer
  method resolves to GET
- **WHEN** the producer sends the request
- **THEN** the outbound request has no body and no `AlreadyConsumed` error
  surfaces
