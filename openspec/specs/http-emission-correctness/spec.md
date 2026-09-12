# http-emission-correctness Specification

## Purpose
TBD - created by archiving change http-bridge-emission-correctness. Update Purpose after archive.
## Requirements
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

When `bridgeEndpoint` is `true`, the HTTP producer `resolve_url` SHALL return the endpoint base URL plus configured query params and SHALL ignore exchange `CamelHttpPath` and `CamelHttpQuery`. The bridged emission SHALL assemble the URL from the base URL's authored bytes verbatim — no dot-segment collapse, no default-port stripping, no scheme or host lowercasing (an intentional alignment with the authored-byte canon; it is a visible change for downstreams that relied on WHATWG normalization from the bridge arm). When `bridgeEndpoint` is `false` (the default), the existing path/query merge behaviour is unchanged. For the same declared base URL and resolved query, emission SHALL be byte-identical whether bridged or not — no arm normalizes.

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

#### Scenario: Bridge arm preserves dot-segment paths

- **GIVEN** a bridged producer endpoint whose base URL path contains dot segments, `http://h/a/../b`
- **WHEN** the producer resolves the outbound URL
- **THEN** the outbound path is `/a/../b` byte-for-byte — dot segments are not collapsed

#### Scenario: Bridge arm preserves explicit default port

- **GIVEN** a bridged producer endpoint `http://h:80/p`
- **WHEN** the producer resolves the outbound URL
- **THEN** the outbound URL keeps `:80` — the explicit default port is not stripped

#### Scenario: Bridge arm preserves scheme and host case

- **GIVEN** a bridged producer endpoint `HTTP://ExAMPLE.COM/p`
- **WHEN** the producer resolves the outbound URL
- **THEN** the outbound URL keeps `HTTP://ExAMPLE.COM/p` as authored — no lowercasing of scheme or host

#### Scenario: Bridge arm with no query emits base verbatim

- **GIVEN** a bridged producer endpoint `http://h/p` with no resolved query (no raw query, no query_params)
- **WHEN** the producer resolves the outbound URL
- **THEN** the outbound URL is `http://h/p` exactly as authored, with no dangling `?`

#### Scenario: Bridge and non-bridge arms emit byte-identical URLs

- **GIVEN** the same declared base URL and the same resolved query composition
- **WHEN** one exchange resolves through the bridged arm and another through the non-bridge path (for example the `CamelHttpQuery` composition arm)
- **THEN** both emissions are byte-identical — normalization divergence between arms cannot occur

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

### Requirement: Pinned DNS client reuse

The `camel-http` producer SHALL reuse a cached, DNS-pinned `reqwest::Client`
for repeated requests whose SSRF-validated resolution `(host, addrs)` is
identical while the cached entry remains logically retrievable, instead of
constructing a new client per request. Per-request
SSRF resolution and validation (`resolve_initial_url_for_ssrf`) SHALL remain
in the request path unchanged. The cache SHALL be bounded by both a
time-to-live and a maximum entry count.

#### Scenario: Repeated identical resolution reuses the pinned client

- **GIVEN** a producer whose requests resolve to the same `(host, addrs)` pair
- **WHEN** two requests are sent while the cached entry remains logically
  retrievable
- **THEN** the pinned-client builder runs at most once (observed through the
  cache's build counter), and both requests use a client whose
  `resolve_to_addrs` pin equals the validated address set

#### Scenario: TTL expiry causes a rebuild

- **GIVEN** a cached pinned client for `(host, A)` whose TTL has elapsed
- **WHEN** a new request validates the same `(host, A)`
- **THEN** a fresh client is built and cached — the expired entry is
  logically dead (non-retrievable) even if its physical reclamation is
  deferred by cache maintenance

#### Scenario: Concurrent misses on the same key build once

- **GIVEN** no cached client exists for a validated `(host, addrs)` key
- **WHEN** multiple concurrent requests need that same key
- **THEN** the pinned-client builder runs once and every request uses the
  resulting client

#### Scenario: Producers of one endpoint share the cache

- **GIVEN** producers created from one endpoint share its pinned-client cache
- **WHEN** two such producers send requests with the same validated
  `(host, addrs)` while the entry remains logically retrievable
- **THEN** both hit the same cache entry and at most one client is built

#### Scenario: Changed resolution builds a fresh pinned client

- **GIVEN** a cached pinned client for `(host, A)` where `A` is one addr set
- **WHEN** a request resolves the same host to a different addr set `B`
- **THEN** a new client pinned to `(host, B)` is built and cached, and the
  request connects only to addresses in `B`

#### Scenario: Addr-set order does not split cache entries

- **GIVEN** DNS returns the same address set in different order across requests
- **WHEN** the cache key is constructed
- **THEN** the addr vector is sorted and deduplicated so both requests hit the
  same cache entry while it remains logically retrievable

#### Scenario: Cache retention is bounded

- **GIVEN** a cache entry older than the TTL, or a cache at capacity
- **WHEN** a lookup or insertion occurs after expiry or beyond capacity
- **THEN** the expired entry is no longer retrievable and the retained entry
  count remains bounded; physical reclamation of the evicted client occurs
  after deferred cache maintenance removes the cache-owned handle and the
  last external clone held by an in-flight request drops

#### Scenario: IP-literal URLs bypass the pinned cache

- **GIVEN** a request URL whose host is an IP literal
- **WHEN** `resolve_initial_url_for_ssrf` returns `None`
- **THEN** the request uses the endpoint's shared client and the pinned cache
  records no entry; the same bypass applies to redirect hops whose target
  host is an IP literal — such hops use an unpinned client and never enter
  the cache

#### Scenario: Redirect hops use the same cache

- **GIVEN** a producer request that follows a manual redirect to a new host
  that is a hostname (not an IP literal)
- **WHEN** the redirect hop resolves and validates `(redirect_host, addrs)`
- **THEN** the hop's client is obtained from the same pinned-client cache, so
  repeated redirects to the same validated target reuse one client while the
  entry remains logically retrievable

### Requirement: Component-scoped pinned client cache

The `camel-http` HTTP and HTTPS components SHALL construct one
pinned-client cache per component instance and SHALL share it across every
endpoint the component creates, including endpoints created for dynamic EIP
URI resolution (`recipientList`, `routingSlip`, `dynamicRouter`). Per-request
SSRF resolution and validation SHALL remain unchanged.

#### Scenario: Endpoints of one component share one cache

- **GIVEN** one `HttpComponent` and two endpoints created from it through
  distinct URIs
- **WHEN** producers of both endpoints send requests that validate to the
  same `(host, addrs)` while the entry remains logically retrievable
- **THEN** the pinned-client builder runs at most once and both producers
  use the same cached client

#### Scenario: Dynamic EIP resolutions hit the shared cache

- **GIVEN** a component whose `create_endpoint` is called repeatedly for
  distinct runtime URIs that share one host, the resolution shape of
  `recipientList`/`routingSlip`/`dynamicRouter`
- **WHEN** each resolution creates a producer and sends a request whose
  validated `(host, addrs)` is identical
- **THEN** the pinned-client builder runs at most once across the
  resolutions while the entry remains logically retrievable

#### Scenario: HTTPS component owns its own cache

- **GIVEN** separate `HttpComponent` and `HttpsComponent` instances
- **WHEN** each creates an endpoint
- **THEN** each endpoint's pinned cache is the one owned by its component,
  and the two caches are distinct

#### Scenario: Reuse across endpoints preserves config identity

- **GIVEN** endpoints of one component whose `http_config` is a clone of
  the component's `HttpConfig`
- **WHEN** a cache hit serves a request from a different endpoint of the
  same component
- **THEN** the reused client is built from an identical `HttpConfig`, so
  reuse is semantically equal to a rebuild

### Requirement: Component-scoped shared unpinned client

The `camel-http` HTTP and HTTPS components SHALL construct one shared
unpinned `reqwest::Client` per component instance and SHALL pass clones of
it to every endpoint the component creates, including endpoints created
for dynamic EIP URI resolution. Endpoint creation SHALL NOT invoke the
client builder. SSRF validation and DNS pinning semantics SHALL remain
unchanged.

#### Scenario: One component constructor builds one client

- **GIVEN** the client-builder invocation counter at a baseline
- **WHEN** one `HttpComponent` or `HttpsComponent` is constructed
- **THEN** the counter increases by exactly one

#### Scenario: Endpoint creation adds no client builds

- **GIVEN** a constructed component
- **WHEN** `create_endpoint` runs for two distinct URIs
- **THEN** the counter does not increase, and both endpoints hold clones
  of the component's single client (one shared connection pool)

#### Scenario: Dynamic EIP resolutions add no client builds

- **GIVEN** a constructed component and a counter baseline
- **WHEN** a resolution loop calls `create_endpoint` plus
  `create_producer` three times over distinct URIs, the resolution shape
  of `recipientList`/`routingSlip`/`dynamicRouter`, without sending
  requests
- **THEN** the counter does not increase

#### Scenario: Reuse preserves config identity

- **GIVEN** endpoints of one component whose unpinned client derives
  solely from the component's `HttpConfig`
- **WHEN** a request from any endpoint of that component uses the shared
  unpinned client (IP-literal URL or IP-literal redirect hop)
- **THEN** the client is built from an identical `HttpConfig`, so reuse is
  semantically equal to a rebuild

### Requirement: Raw-preserving outbound query serialization

When the producer endpoint URI carries an authored raw query, `resolve_url` MUST emit those bytes byte-for-byte minus explicitly consumed option keys. Option-key consumption MUST have a single metadata-driven owner (ADR-0041 `uri_options()`): it is the sole authority for OUTBOUND option filtering — the raw filter consults it at runtime — while `from_uri`'s manual typed parsing (`skip_impl`) stays direct and unchanged; no duplicated handwritten key lists. Authored non-option pairs are carried ONLY by `raw_query` (no structured re-collection). "Byte-for-byte" is bounded to wire-legal authored bytes: a raw byte forbidden in a query component MUST produce a resolve error, never silent re-encoding. Query-source precedence MUST hold: `bridgeEndpoint` bridging > `CamelHttpUri` override > `CamelHttpQuery` header (composed per ADR-0071: pairs ride verbatim, append for keys absent from the higher-precedence source, and lose key collisions) > endpoint `raw_query` base > programmatic `query_params` (the only structured path — minimal RFC-3986 encoding, declaration order, `%20` never `+` for spaces). `RAW(...)` wrappers MUST NOT be unwrapped or re-encoded by the serializer. URL resolution failures MUST propagate as errors — malformed operator input never panics the producer task.

#### Scenario: Authored order and encoding survive to the wire

- **Given** a producer endpoint URI `http://h/p?a=1&b=x,y&c=t:1&connectTimeout=5s` where `connectTimeout` is a consumed camel-http option
- **When** the producer resolves the outbound URL
- **Then** the outbound query is `a=1&b=x,y&c=t:1` — authored order, authored separators, no form-encoding of `,` `:` or space, consumed option removed

#### Scenario: Query that is all consumed options disappears

- **Given** a producer endpoint URI `http://h/p?connectTimeout=5s` whose only query pair is a consumed option
- **When** the producer resolves the outbound URL
- **Then** the outbound URL is `http://h/p` with no query component — no dangling `?`

#### Scenario: Authored empty-query marker is preserved

- **Given** a producer endpoint URI `http://h/p?` with a bare empty-query marker
- **When** the producer resolves the outbound URL
- **Then** the outbound URL preserves the empty query distinctly — an authored `?` is not conflated with an all-consumed query

#### Scenario: Percent-encoded option keys are consumed by decoded match

- **Given** a producer endpoint URI whose raw query authors the option as `connect%54imeout=5s`
- **When** the producer resolves the outbound URL
- **Then** the pair is consumed — the raw filter matches the decoded key, not the encoded bytes

#### Scenario: CamelHttpQuery stays verbatim

- **Given** a producer exchange carrying a `CamelHttpQuery` header and an endpoint with raw query `x=1`
- **When** the producer resolves the outbound URL
- **Then** the header pairs ride verbatim (not structured-serialized), append after the endpoint pairs for absent keys, and lose key collisions to the higher-precedence query source (ADR-0071)

#### Scenario: Programmatic fallback stays deterministic

- **Given** query parameters attached programmatically via endpoint `query_params` config with no raw source, declared as `[("z", "1"), ("a", "2")]` in that order
- **When** the producer resolves the outbound URL
- **Then** the query is `z=1&a=2` — declaration order (not lexical), minimal RFC-3986 percent-encoding, `%20` — never `+` — for spaces

#### Scenario: Wire-forbidden raw byte errors instead of re-encoding

- **Given** an authored raw query containing a byte forbidden in a query component (e.g. a literal space or `#`)
- **When** the producer resolves the outbound URL
- **Then** resolution fails with an error naming the offending byte — the serializer never silently re-encodes authored bytes

#### Scenario: RAW wrapper neither unwrapped nor re-encoded

- **Given** a producer endpoint URI whose raw query contains a `RAW(...)`-wrapped value
- **When** the producer resolves the outbound URL
- **Then** the outbound query carries the wrapper bytes exactly as authored — the serializer neither strips the wrapper nor double-encodes it (the rc-g4isv regression)

#### Scenario: Malformed base URL errors instead of panicking

- **Given** an operator-supplied base URL that `url::Url::parse` rejects
- **When** the producer resolves the outbound URL
- **Then** the failure propagates as a producer error result and the producer task keeps running — no `.expect`/panic path (the rc-ph7z2 regression)

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

