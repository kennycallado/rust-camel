# Spec Delta: http-emission-correctness

## ADDED Requirements

### Requirement: RFC-categorised header classification

The HTTP component SHALL classify policy-relevant HTTP headers through one shared module that sorts each such header into one of three RFC-derived buckets: hop-by-hop/framing, request-only, or server-owned. Both the outbound-request producer and the response-emission consumer SHALL consult this one module, so the two classification lists cannot drift. Headers outside these buckets remain unclassified and pass unless a direction-specific rule excludes them.

The hop-by-hop/framing bucket SHALL include the fixed RFC 7230 section 6.1 set (`connection`, `keep-alive`, `proxy-authenticate`, `proxy-authorization`, `te`, `trailer`, `transfer-encoding`, `upgrade`) plus `proxy-connection`. The module SHALL also exclude, in both directions, any header whose name appears as a token in a `Connection` field value on the current message (dynamic Connection-named stripping).

#### Scenario: fixed hop-by-hop set excluded in both directions

- **GIVEN** an exchange whose headers include `Connection`, `Transfer-Encoding`, and `Upgrade`
- **WHEN** the producer builds the outbound request AND the consumer builds the response
- **THEN** none of those headers pass the shared classification module in either direction

#### Scenario: dynamic Connection-named stripping

- **GIVEN** an exchange whose headers include `Connection: X-Custom, Keep-Alive` and a header named `X-Custom`
- **WHEN** the headers are classified
- **THEN** `X-Custom` is excluded as hop-by-hop in both directions, while headers NOT named by `Connection` survive

#### Scenario: malformed Connection value never panics

- **GIVEN** an exchange carrying `Connection: X-Custom, bad token, ,` plus `X-Custom` and `X-Unrelated` headers
- **WHEN** the headers are classified
- **THEN** `X-Custom` is stripped, the invalid and empty tokens are ignored, and `X-Unrelated` is preserved without panic

#### Scenario: case-insensitive and de-duplicated tokens

- **GIVEN** a `Connection` value with mixed-case and repeated tokens (`X-Custom, x-custom,  X-Custom `)
- **WHEN** the headers are classified
- **THEN** the named header is excluded exactly once, compared case-insensitively after trimming whitespace

### Requirement: Producer outbound header forwarding

The HTTP producer SHALL NOT forward hop-by-hop/framing headers (including dynamically Connection-named ones) or `content-length` onto the outbound request. The outbound `Host` SHALL be derived from the destination URL, not copied from the incoming request. Request-only headers, including `User-Agent` and `Accept`, SHALL be forwarded unless `skipRequestHeaders` explicitly excludes them. `Host` remains destination-derived.

#### Scenario: Host and framing headers not forwarded

- **GIVEN** an exchange whose headers include `Host: localhost`, `Content-Length: 42`, and `Connection: keep-alive`
- **WHEN** the producer builds the outbound request to destination `http://httpbin.org`
- **THEN** the outbound request carries no `Host: localhost`, no `Content-Length` copied from the exchange, and no `Connection`; the outbound `Host` equals `httpbin.org`

#### Scenario: request-only headers forwarded

- **GIVEN** an exchange whose headers include `Accept: application/json` and `User-Agent: myclient/1.0`
- **WHEN** the producer builds the outbound request
- **THEN** both `Accept` and `User-Agent` are forwarded to the destination

#### Scenario: user skipRequestHeaders still honoured

- **GIVEN** a producer configured with `skipRequestHeaders=Authorization` and an exchange carrying `Authorization`
- **WHEN** the producer builds the outbound request
- **THEN** `Authorization` is not forwarded, in addition to the RFC-excluded set

### Requirement: Consumer response header emission

The HTTP consumer reply-finalizer SHALL NOT strip the valid response headers `Cache-Control`, `Pragma`, `Warning`, or `Via`. A route that sets one of these headers SHALL see it emitted on the wire. The finalizer SHALL still exclude hop-by-hop/framing, request-only, and server-owned (`Date`) headers from responses.

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

### Requirement: Policy documentation

The change SHALL author ADR-0057 ("HTTP header emission policy") under `docs/adr/` and SHALL correct `crates/components/camel-http/CONTEXT.md` to cite ADR-0057 for header policy. ADR-0057 SHALL define the three RFC-derived buckets and the direction rules (producer outbound exclusion; consumer response emission). `CONTEXT.md` SHALL no longer cite ADR-0024 for header policy.

#### Scenario: ADR-0057 defines both direction rules

- **GIVEN** the change is complete
- **WHEN** `docs/adr/0057-*.md` is inspected
- **THEN** it defines the hop-by-hop/framing, request-only, and server-owned buckets, the dynamic Connection-named stripping, the producer outbound exclusion rule, and the consumer response emission rule

#### Scenario: CONTEXT.md re-cited to ADR-0057

- **GIVEN** the change is complete
- **WHEN** `crates/components/camel-http/CONTEXT.md` is inspected at the reply-header contract section
- **THEN** the header policy cites ADR-0057 (headers) and ADR-0024 (status/body/Stop), not ADR-0024 alone for headers

### Requirement: bridgeEndpoint URL bridging

When `bridgeEndpoint` is `true`, the HTTP producer `resolve_url` SHALL return the endpoint base URL plus configured query params and SHALL ignore exchange `CamelHttpPath` and `CamelHttpQuery`. When `bridgeEndpoint` is `false` (the default), the existing path/query merge behaviour is unchanged.

#### Scenario: bridgeEndpoint true ignores exchange path

- **GIVEN** an exchange carrying `CamelHttpPath=/foo` and a producer endpoint `http://x?bridgeEndpoint=true`
- **WHEN** the producer resolves the outbound URL
- **THEN** the outbound URL is `http://x` with no `/foo` appended

#### Scenario: bridgeEndpoint false keeps merge behaviour

- **GIVEN** an exchange carrying `CamelHttpPath=/foo` and a producer endpoint `http://x` (bridgeEndpoint defaults to false)
- **WHEN** the producer resolves the outbound URL
- **THEN** the outbound URL appends `/foo`, preserving the existing behaviour

#### Scenario: configured query params still applied under bridging

- **GIVEN** a producer endpoint `http://x?bridgeEndpoint=true&token=secret`
- **WHEN** the producer resolves the outbound URL with an exchange carrying `CamelHttpQuery=dropme=1`
- **THEN** the outbound URL carries `token=secret` and does NOT carry `dropme=1`

### Requirement: Composed bridge/proxy acceptance

The HTTP component SHALL pass a composed `http:` -> `http:` bridge/proxy integration test that exercises the full path: a Host-routed destination receives an outbound request whose `Host` matches the destination (not the consumer host), and route-set response headers survive emission. The test SHALL be local and deterministic and SHALL NOT depend on a public CDN or external host.

#### Scenario: outbound Host matches destination through a bridge

- **GIVEN** a local destination server that rejects any request whose `Host` does not match its own address
- **WHEN** a request is proxied through an `http:` consumer into an `http:` producer with `bridgeEndpoint=true`
- **THEN** the destination accepts the request (the outbound `Host` matches the destination)

#### Scenario: route-set response header survives the bridge

- **GIVEN** a bridge route that sets `Cache-Control` on the response
- **WHEN** the consumer emits the response
- **THEN** the client receives `Cache-Control` on the wire
