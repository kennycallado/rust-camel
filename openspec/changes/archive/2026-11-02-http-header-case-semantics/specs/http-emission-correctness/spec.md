## MODIFIED Requirements

### Requirement: Consumer response header emission

The HTTP consumer reply-finalizer SHALL NOT strip the valid response headers `Cache-Control`, `Pragma`, `Warning`, or `Via`. A route that sets one of these headers SHALL see it emitted on the wire. The finalizer SHALL still exclude hop-by-hop/framing, request-only, and server-owned (`Date`) headers from responses.

The finalizer SHALL resolve the route's response content type with ASCII
case-insensitive header lookup, so a route that sets `content-type`,
`Content-Type`, or any mixed-case spelling controls the reply. The finalizer
SHALL NOT echo the inbound request `Content-Type` when the route did not
change it: at ingress the consumer records the request value in the
Camel-namespaced exchange property `CamelHttpRequestContentType`, and a
content-type whose current value equals that recorded value is treated as
inherited and omitted, leaving body inference in charge. The same suppression
SHALL apply to a `Body::Stream` whose metadata content type equals the
recorded request value, so a stream inferred from the request cannot
reintroduce it. A route content type that differs from the recorded request
value SHALL be emitted. Equality is the normative boundary between inherited
and route-set: a route that sets exactly the request value is treated as
inherited. `select_response_headers` SHALL drop every content-type key
case-insensitively and SHALL append exactly one `Content-Type` when a
content type is resolved (route value or body inference), and none otherwise.

#### Scenario: route-set Cache-Control emitted

- **GIVEN** a route that runs `set_header("Cache-Control", "public, max-age=3600")` after all pipeline steps
- **WHEN** the consumer builds the response
- **THEN** the response carries `Cache-Control: public, max-age=3600`

#### Scenario: Via emitted as the fourth header

- **GIVEN** a route that runs `set_header("Via", "1.1 myproxy")`
- **WHEN** the consumer builds the response
- **THEN** the response carries `Via: 1.1 myproxy`

#### Scenario: Pragma and Warning also survive

- **GIVEN** a route that runs `set_header("Pragma", "no-cache")` and `set_header("Warning", "199 misc")`
- **WHEN** the consumer builds the response
- **THEN** the response carries both `Pragma` and `Warning`

#### Scenario: request-only headers not emitted on a response

- **GIVEN** a response whose exchange headers include `User-Agent` and `Accept`
- **WHEN** the consumer builds the response
- **THEN** neither `User-Agent` nor `Accept` is emitted on the response

#### Scenario: server-owned Date still excluded

- **GIVEN** a response whose exchange headers include a route-set `Date`
- **WHEN** the consumer builds the response
- **THEN** the response `Date` is owned by the server and the route-set value is not emitted as the authoritative `Date`

#### Scenario: lowercase route content type controls the reply

- **GIVEN** a request without a `Content-Type` and a route that sets
  `content-type: application/vnd.acme+json`
- **WHEN** the consumer builds the response
- **THEN** the response carries `Content-Type: application/vnd.acme+json`

#### Scenario: mixed-case route content type controls the reply

- **GIVEN** a request without a `Content-Type` and a route that sets
  `CoNtEnT-TyPe: text/csv`
- **WHEN** the consumer builds the response
- **THEN** the response carries `Content-Type: text/csv`

#### Scenario: inherited request content type is not echoed

- **GIVEN** a request with `Content-Type: application/xml`, an inherited
  exchange `Content-Type: application/xml`, and `CamelHttpRequestContentType`
  set to `application/xml`
- **WHEN** the consumer builds the response
- **THEN** `select_response_headers` emits no `Content-Type` unless body
  inference supplies one, and never emits the request value as a route value

#### Scenario: inherited stream metadata is not echoed

- **GIVEN** a request with `Content-Type: application/xml` whose body is a
  `Body::Stream` with metadata content type `application/xml`, and the route
  does not change it
- **WHEN** the consumer builds the response
- **THEN** the stream metadata content type is suppressed and no
  `Content-Type: application/xml` is emitted

#### Scenario: route value identical to the request is treated as inherited

- **GIVEN** a request with `Content-Type: application/xml` and a route that
  sets `content-type: application/xml`
- **WHEN** the consumer builds the response
- **THEN** the value is treated as inherited and omitted (no provenance
  channel exists for identical values)

#### Scenario: empty body without a resolved content type emits none

- **GIVEN** an exchange whose body is `Body::Empty`, no route content type,
  and no inferred content type
- **WHEN** the consumer builds the response
- **THEN** the response carries no `Content-Type` header

#### Scenario: route override replaces the inherited value

- **GIVEN** a request with `Content-Type: application/xml` and a route that
  sets `content-type: application/json`
- **WHEN** the consumer builds the response
- **THEN** the response carries exactly one `Content-Type: application/json`

#### Scenario: duplicate content-type keys are dropped before emission

- **GIVEN** exchange headers containing `Content-Type` and `content-type`
- **WHEN** `select_response_headers` selects response headers
- **THEN** neither raw key is forwarded and at most the single re-derived
  `Content-Type` is appended
