## MODIFIED Requirements

### Requirement: Consumer response header emission

The HTTP consumer reply-finalizer SHALL NOT strip the valid response headers `Cache-Control`, `Pragma`, `Warning`, or `Via`. A route that sets one of these headers SHALL see it emitted on the wire. The finalizer SHALL still exclude hop-by-hop/framing, request-only, and server-owned (`Date`) headers from responses.

The finalizer SHALL resolve the route's response content type with ASCII case-insensitive header lookup, so a route that sets `content-type`, `Content-Type`, or any mixed-case spelling controls the reply. Inheritance SHALL be tracked by provenance, not by value: at ingress the consumer records the request value in the `CamelHttpRequestContentType` exchange property and sets the `CamelHttpContentTypeInherited` marker property when the request carried a `Content-Type` header. Every route path that writes, removes, replaces, or restores the `Content-Type` header SHALL clear or restore the marker when a key matches ASCII-case-insensitively: `SetHeader`, `SetHeaderIfAbsent`, `DynamicSetHeader`, `DynamicSetHeaderIfAbsent`, `RemoveHeader`, the script mutator (for any mutation a script lands on the header entry set), the HTTP and CXF producer response-header mappings, WASM host input replacement (entry-list diff, so guest removals clear too), function patch application, claim-check header restore, archive/stream fragment stripping, and original-message restoration (the marker presence snapshotted with the original message at route entry is restored with it; capture always replaces the snapshot, so a stale presence cannot survive into a nested route — never inferred from header presence).

While the marker is set the finalizer SHALL NOT emit a `Content-Type` from the message header or from body-kind inference. Once a route write has cleared the marker — or when the request carried no `Content-Type`, so no marker exists — the finalizer SHALL emit the message-header value verbatim without comparing it to the recorded request value, and SHALL apply body-kind inference unchanged. A route-set value SHALL be emitted even when it equals the request value, and the message header, when resolved, is the emitted value. The `Body::Stream` metadata rule is unchanged and independent of the marker: the metadata content type is the fallback, resolved only when no route-set message header resolves, so the reply never carries two `Content-Type` values; a metadata value equal to the recorded request value SHALL stay suppressed in both marker states (so an ingress-copied metadata value cannot reintroduce the request value, and removing the header cannot unmask it), while a metadata value that differs from the recorded request value SHALL be emitted as that fallback. Writes through the documented raw escape hatch (direct header-map mutation inside custom processors) bypass provenance tracking, as they bypass header normalization today. `select_response_headers` SHALL drop every content-type key case-insensitively and SHALL append exactly one `Content-Type` when a content type is resolved (route value, or body inference as the fallback), and none otherwise.

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

- **GIVEN** a request with `Content-Type: application/xml` whose marker property
  `CamelHttpContentTypeInherited` is set, and a route that does not write
  `Content-Type`
- **WHEN** the consumer builds the response
- **THEN** no route-echoing `Content-Type` is emitted: the untouched message
  header and body-kind inference are suppressed, and an equal-value stream
  metadata content type is filtered, so the reply never echoes the request
  value

#### Scenario: inherited stream metadata is not echoed

- **GIVEN** a request with `Content-Type: application/xml` whose body is a
  `Body::Stream` with metadata content type `application/xml`, and the route
  does not change it
- **WHEN** the consumer builds the response
- **THEN** the stream metadata content type is suppressed and no
  `Content-Type: application/xml` is emitted; the suppression compares the
  metadata value with the recorded request value, applies in both marker
  states, and the metadata channel is only the fallback when no route-set
  header resolved, so removing the header does not unmask the ingress copy

#### Scenario: route value identical to the request is treated as inherited

- **GIVEN** a request with `Content-Type: application/xml` and a route that
  writes `content-type: application/xml` through `SetHeader`
- **WHEN** the consumer builds the response
- **THEN** the write has cleared the marker, so the route value is emitted
  verbatim as `Content-Type: application/xml`; provenance, not value
  equality, decides

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
