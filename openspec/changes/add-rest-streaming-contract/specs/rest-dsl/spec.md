## ADDED Requirements

### Requirement: Raw binding stream ownership and metadata

In binding `raw`, the request `Body::Stream` SHALL reach the first user step
and every injected binding-independent step unconsumed: the lowered pipeline
SHALL NOT poll, materialize, re-wrap, or cache the request stream, and SHALL
NOT alter its `StreamMetadata`. The HTTP consumer SHALL record the request
`Content-Type` and `Content-Length` in the `StreamMetadata` of the stream it
installs on the exchange. A raw operation SHALL be free to reply with the
original request stream or with a new `Body::Stream`, and both forms SHALL
reach the wire intact under the response `Content-Type` the route supplies.

#### Scenario: pipeline never polls the request stream

- **GIVEN** a lowered binding `raw` route and an exchange whose body is a
  stream that records every poll
- **WHEN** the compiled step sequence runs to completion
- **THEN** the poll recorder shows zero polls and the exchange body variant
  is still `Body::Stream`

#### Scenario: pipeline preserves stream identity

- **GIVEN** a lowered binding `raw` route, an exchange whose body is a
  stream, and a kept identity handle to that stream's mutex
- **WHEN** the compiled step sequence runs to completion
- **THEN** the stream mutex reachable from the exchange body is the same
  allocation as the identity handle, proving no replacement `StreamBody`
  was installed

#### Scenario: request stream stays single-consumption after the pipeline

- **GIVEN** a binding `raw` route whose compiled steps have run to
  completion over a one-chunk stream body
- **WHEN** the body is consumed once and then consumed again
- **THEN** the first consumption yields exactly the stream's bytes and the
  second fails with `CamelError::AlreadyConsumed`

#### Scenario: request stream metadata is preserved end-to-end

- **GIVEN** a binding `raw` route and an exchange whose stream metadata
  declares a content type and size hint
- **WHEN** the compiled step sequence runs to completion
- **THEN** the metadata is byte-for-byte unchanged, and independently, a
  real HTTP request carrying `Content-Type` and `Content-Length` arrives at
  the route boundary as `Body::Stream` whose metadata carries both values

#### Scenario: reply body may be the original or a new stream

- **GIVEN** a binding `raw` operation whose processing leaves the original
  request stream as the reply body, and another that replaces the reply
  body with a newly generated `Body::Stream`
- **WHEN** the HTTP reply is finalized for both
- **THEN** both responses deliver their full payload on the wire under the
  route-supplied response `Content-Type`

### Requirement: Raw binding stream error and limit contract

In binding `raw`, a second consumption attempt on a stream already consumed
during route processing SHALL fail with `CamelError::AlreadyConsumed` and
SHALL propagate as a pipeline error, never as a panic. A reply whose
`Body::Stream` is already consumed SHALL produce an HTTP 500 response with
an empty body. Request size limits SHALL fail closed: a request with
`Content-Length` over `max_request_body` SHALL be rejected 413 before the
stream opens, and a chunked request whose accumulated bytes exceed
`max_request_body` SHALL yield the cap error when the stream is consumed.
Response limits SHALL apply to materialized reply bytes only: an over-cap
materialized reply SHALL be replaced with an HTTP 500 carrying the message
`Response body exceeds configured limit`, while a streamed reply SHALL NOT
be byte-capped by `max_response_body`. A client disconnect during a streamed
reply SHALL NOT fail the consumer: subsequent requests on the same server
SHALL be served.

#### Scenario: in-route double consumption fails with AlreadyConsumed

- **GIVEN** a binding `raw` route processing an exchange whose request
  stream has already been fully consumed once during the route
- **WHEN** route processing consumes the stream a second time
- **THEN** the consumption fails with `CamelError::AlreadyConsumed` and the
  failure propagates as an error, not a panic

#### Scenario: consumed reply stream returns 500

- **GIVEN** a binding `raw` route whose reply body is a `Body::Stream`
  whose underlying stream was already consumed
- **WHEN** the HTTP reply is finalized
- **THEN** the response status is 500 and the response body is empty

#### Scenario: chunked request over the cap fails closed on consumption

- **GIVEN** a raw HTTP endpoint with `max_request_body` set to N and a
  chunked request (no `Content-Length`) whose total bytes exceed N
- **WHEN** the route consumes the request stream
- **THEN** consumption fails with the cap error naming the configured limit

#### Scenario: materialized reply bytes over the cap are replaced with 500

- **GIVEN** a raw HTTP endpoint with `max_response_body` set to N and a
  route that replies with materialized bytes longer than N
- **WHEN** the HTTP reply is finalized
- **THEN** the response status is 500 and the body is the message
  `Response body exceeds configured limit`

#### Scenario: streamed replies are not byte-capped

- **GIVEN** a raw HTTP endpoint with `max_response_body` set to N and a
  route that replies with a `Body::Stream` longer than N
- **WHEN** the HTTP reply is finalized
- **THEN** the response succeeds and delivers every stream byte

#### Scenario: client disconnect during a streamed reply does not fail the consumer

- **GIVEN** a raw HTTP endpoint streaming a reply that has delivered at
  least one chunk
- **WHEN** the client drops the connection mid-reply and a new request is
  sent to the same server
- **THEN** the new request is served successfully
