## Trust boundary and credential redaction

### Exchange data

ADR-0032 defines request headers, body, query values, path values, and path
parameters as untrusted exchange data. The HTTP Consumer copies this data into
`exchange.input` without validation or redaction. Each route must validate the
data where it crosses into a control action, resource decision, or
executable/interpretable sink.

The Consumer bounds resource use with a 2 MiB default request-body limit, a
read timeout, and an in-flight request semaphore. The Producer has a 10 MiB
default response-body limit.

The in-flight semaphore (`maxInflightRequests`) is the single intake
backpressure point. The per-route `RequestEnvelope` channel capacity derives
from the same limit (`envelope_channel_capacity`, minimum 1), so the channel
can never become a second, hidden inflight cap; `0` rejects every request with
503 instead of panicking at consumer start.

### Credential redaction

`HttpAuth` implements `Debug` manually and redacts passwords and bearer tokens.
`ServerTlsConfig` and `TlsConfig` also implement `Debug` manually and redact
certificate and key paths (`ca_cert_path`, `client_cert_path`, `client_key_path`).
Follow these patterns for types that contain credentials or sensitive paths.
`HttpConfig` derives `Debug` but delegates to `TlsConfig`'s redacting impl for
its `tls` field, so its `Debug` output is safe.

### Outbound SSRF and TLS defaults

The Producer validates each outbound URL and redirect hop. By default,
`allow_internal=false` rejects internal addresses. DNS resolution pins validated
addresses with `resolve_to_addrs` to prevent DNS rebinding. Cross-origin
redirects remove `Authorization` and `Cookie` headers. When
`allow_internal=true`, cleartext HTTP to public addresses remains forbidden.

`TlsConfig` verifies peer certificates by default. The Producer disables
verification only when an operator sets `tls.insecure=true` or
`tls.verify_peer=false`, and it emits a warning. The Consumer rejects a partial
server TLS configuration that supplies only a certificate or only a key.
Producer TLS material (CA bundle, client identity) is read from disk when a
client is built; pinned clients are cached for `PINNED_CLIENT_TTL` (60 s), so
edited PEM files take effect at most once per TTL window per client. Producer
certificate rotation is not a supported feature (consumer-side TLS has a
hot-reload path via `TlsReloadRegistry` in `src/tls_reload.rs`; the producer
has no equivalent).

The Producer attaches the exchange body only for entity-enclosing methods
(POST, PUT, PATCH). GET, HEAD, DELETE, OPTIONS, and TRACE send no body and log
one `warn!` when a non-empty body (or any stream body) is dropped. The body
stays consumed. No configuration override exists (Apache Camel
`HttpMethods.isEntityEnclosing` parity).

The pinned-client cache is shared across all endpoints of one component
instance. It retains at most `PINNED_CLIENT_MAX_ENTRIES` (64) clients, each
live for a `PINNED_CLIENT_TTL` (60 s) window. Each cached client serves one
`(host, address set)` pair. Every `get_or_build` lookup emits
`camel_pinned_client_cache_hits_total`/`_misses_total` (one miss per client
construction; single-flight waiters count as hits) and
`camel_pinned_client_cache_size` through the late-bound handle wired once at
endpoint creation (ADR-0066) — the leak-regression signal rc-u4qz. It holds up
to `pool_max_idle_per_host`
(default 100) idle connections for that host until `pool_idle_timeout_ms`
(default 90 s) closes them. Worst case is 64 clients times 100 idle
connections per component instance. The shared unpinned client also holds up
to `pool_max_idle_per_host` idle connections for each of its hosts.

## Credential sources

A `security_policy` block on a `from: http://` route may declare a
`credential_sources` list. It names where the credential comes from:
`authorization_header`, a query parameter, a cookie, or a named custom header
(`header: {name: X-API-Key}`). The list is validated at plan compilation
(unsupported sources for the transport fail at load) and is consumed at the
request boundary: `HttpKernelAuth` extracts via `extract_token_multi` and
authenticates through `camel_auth::kernel_authenticate`, installing the
typed principal carrier on the Exchange before the pipeline (ADR-0061
Rule 1; extraction shapes per ADR-0059). When the key is absent, the
default is `[authorization_header]` only (ADR-0033).

### Redact-by-construction

camel-http has no request access log. The diagnostic sinks that exist on the
request path are the error-context logs around `pipeline_error_to_reply` and the
error reply body. The contract is redact-by-construction: no diagnostic record
emitted while handling a request renders a declared credential value (query
parameter, cookie, or custom header). The 401/403 reply body carries a generic
reason only. Any access log added later inherits the same obligation (ADR-0051).

### Browser cookie guidance

Cookies are the only transport for browser-facing tile services (`<img src>`
from Leaflet, MapLibre, OpenLayers), which cannot attach custom headers.

- `SameSite=Lax` (or stricter) and `HttpOnly` cookies are the operator's
  responsibility. rust-camel does not set these attributes; the operator sets
  them where the cookie is issued.
- Cookie auth on state-changing verbs (POST, PUT, DELETE) requires CSRF
  defense. SameSite does not remove the need for it.
- GET-only tile services are the primary target. A read-only route serving
  images carries no state-changing side effect, so the CSRF surface is minimal.

## Log-level policy

Per ADR-0012, this component's `error!` sites are categorized as:

- **(e) outside-contract** (lib.rs L748, L774):
  - L748 = accept-loop error in `run_axum_server`. Calls `runtime.metrics().increment_errors(route_id, "e:http:accept")` BEFORE the `error!`. The metric is the operator signal; `error!` provides loud log visibility.
  - L774 = server task exited unexpectedly in `monitor_axum_task`. Calls `runtime.metrics().increment_errors(route_id, "e:http:server-task-exited")` BEFORE the `error!`. Same pattern.
  Both sites keep `error!` with `// log-policy: outside-contract`.

- **(c) system-broken** (lib.rs L1108): `Body::Stream` already consumed before HTTP reply — programming-contract violation in `dispatch_handler`. Keeps `error!` with `// log-policy: system-broken`. No metric call (operator alert via error! is the signal).

- **(a) handler-owned** (lib.rs L1210): pipeline error processing HTTP request → 500 response in `dispatch_handler`. Route ErrorHandler owns the ERROR. Downgraded to `warn!` with `// log-policy: handler-owned`. No metric call.

### warn! sites (ADR-0012 advisory)

- **(a) handler-owned** (lib.rs, `build_client()`, warn "HTTP TLS verification disabled"): TLS verification disabled via `insecure=true` or `verify_peer=false` in `TlsConfig`. `warn!` with `// log-policy: handler-owned`. No metric call — the operator is responsible for this config.
- **(a) handler-owned** (lib.rs, `HttpProducer::call`, warn "dropping request body" x2, stream arm and non-empty-bytes arm): request body dropped for a non-entity-enclosing HTTP method (GET, HEAD, DELETE, OPTIONS, TRACE) in the Producer send path. Emitted when the exchange body is non-empty (or any stream body) for such a method. `warn!` with `// log-policy: handler-owned`. No metric call — the route author controls the method and body.

Reviewer: r_glm5.1 verifies these classifications against source at Phase C review time.

## Outbound query fidelity

The Producer resolves the outbound URL through `resolve_url`, which returns
`Result<String, CamelError>` — malformed operator input never panics the
producer task. The outbound URL policy — query composition and the
`allowedUriHosts` fence — is recorded in ADR-0071. Query source precedence:

1. `bridgeEndpoint=true` — the endpoint base URL carries the endpoint's own
   `raw_query` (consumed option keys filtered; `bridgeEndpoint` itself is one)
   with programmatic `query_params` appending absent keys after it; all exchange
   URL headers (`CamelHttpUri`, `CamelHttpPath`, `CamelHttpQuery`) are ignored.
   This check precedes the `CamelHttpUri` override so bridging wins.
2. `CamelHttpUri` override — replaces the base URL; `CamelHttpPath` and
   `CamelHttpQuery` compose with it (ADR-0071). When the override URI carries
   its own query and the exchange also carries `CamelHttpQuery`, the two merge
   at pair level — override-URI pairs first (winning collisions), header pairs
   appending for absent keys — instead of concatenating a second `?` marker
   (the double-`?` merge fix). `CamelHttpPath` applies to the path component
   before query composition.
3. `CamelHttpQuery` header — COMPOSES with the arm-specific higher-precedence
   source (ADR-0071), it does not replace it:
   - Base arm (no `CamelHttpUri`): the higher-precedence source is the
     endpoint query (`raw_query` + programmatic `query_params`,
     consumed-option-filtered). Header pairs append verbatim for absent keys
     only; the endpoint wins any collision.
   - Override arm (`CamelHttpUri` present): the higher-precedence source is
     the override URI's own query — the endpoint base query does NOT ride an
     override (the override remains untrusted exchange data under ADR-0032).
   - A present-but-empty `CamelHttpQuery` is a no-op: the higher-precedence
     source is emitted unchanged with no additional `?` marker.
   - Header pair bytes ride verbatim; a raw byte forbidden in a query
      component inside a header value is a resolve error naming the byte
      (Wave-A law). This deliberately diverges from Apache Camel
      header-wins-verbatim semantics: collisions resolve to the higher-
      precedence source — the endpoint config in the base arm, the override
      URI's own pairs in the override arm.
4. Endpoint `raw_query` base — authored bytes byte-for-byte minus consumed
   option keys, then programmatic `query_params`.

Default inbound reflection (rc-k3pir, ADR-0071): the consumer installs
`CamelHttpPath`/`CamelHttpQuery` from the inbound wire request, and a
non-bridged producer consumes them by default, composing per rule 3. The
plain-proxy shape keeps working; the operator query pair is not replaced by
reflected inbound data.

`query_params` is programmatic-only — never auto-populated from the URI. It is a
`Vec<(String, String)>` emitted in declaration order with minimal RFC-3986
encoding; `%20`, never `+`, for spaces. Authored keys always win: a
`query_params` entry whose key is already present in the authored pairs is
skipped (no duplication, no override). Authored non-option pairs are carried
ONLY by `raw_query`; there is no structured re-collection. `RAW(...)` wrappers
are neither unwrapped nor re-encoded.

A raw byte forbidden in a query component produces a resolve error naming the
offending byte — the serializer never silently re-encodes authored bytes.

Apostrophe (0x27) is rejected even though RFC 3986 `pchar` admits it: reqwest's
WHATWG URL parser re-encodes 0x27 as `%27` in every http/https query, so the
raw byte can never ride the wire verbatim. Admitting it would silently
normalize authored bytes; the wire-faithful authored form is the explicit
`%27` escape (rc-nmupb).

### CamelHttpUri host fence (`allowedUriHosts`)

Next to `bridgeEndpoint`, the outbound URL policy includes an opt-in
`allowedUriHosts` fence for the `CamelHttpUri` override (rc-rbfxq, ADR-0071).
Comma-separated exact host entries, each optionally `host:port`; bracketed
IPv6 literals compare in canonical form, DNS names case-insensitively, a
host-only entry permits any port, and a `host:port` entry matches only the
override's effective port. A malformed entry (path or userinfo, or anything
the `url` crate rejects) or a declared option yielding zero valid entries
fails endpoint creation. An armed fence fails closed: an override resolving
to an unlisted host, or yielding no host, is a resolve error and the rejected
URL is rendered only through the diagnostics redaction path (ADR-0051). An
unarmed endpoint (option absent) keeps the pre-fence override behavior. The
option is consumed and never appears in the outbound query. Unlike ADR-0034's
mandatory `authorizedRoutes`, this fence is opt-in — a deliberate
compatibility trade-off (defense-in-depth hardening, not incident response).

Option-key consumption has a single metadata-driven owner: `uri_options()`
(ADR-0041), consulted at runtime by the raw filter (`is_consumed_option`) — no
duplicated handwritten key lists. `connectTimeout` is promoted to metadata-only:
it is a consumed option whose effective value comes from the global http config.
The legacy `HTTP_CAMEL_OPTIONS` list is deleted.

## Contract Surface

Per ADR-0057 (headers) and ADR-0024 (status/body/Stop). Documents the accepted and rejected names/values for the HTTP consumer reply finaliser. Future bug reports check here first: if behaviour is in this surface, it is a feature request. If not, it is a bug.

### Accepted — reply status code

- `CamelHttpResponseCode` header on the Exchange (type `u16` as JSON number, or string-parseable-as-u16 in range `100..1000`). Drives the HTTP response status.
- If header absent: `200 OK` (normal completion) or `200 OK` (Stop — post-ADR-0024, same code path).
- The `200`-on-empty-Stop replaces the legacy `204` default. Users wanting `204` set `CamelHttpResponseCode=204` explicitly.

### Accepted — reply body

- `Body::Empty` → empty body.
- `Body::Bytes(b)` → raw bytes.
- `Body::Text(s)` → UTF-8 bytes, `Content-Type: text/plain; charset=utf-8` unless overridden.
- `Body::Xml(s)` → UTF-8 bytes, `Content-Type: application/xml`.
- `Body::Json(v)` → JSON-serialised bytes, `Content-Type: application/json`.
- `Body::Stream(s)` → streamed; `Content-Type` from `s.metadata.content_type`. Body MUST NOT be already consumed (system-broken `error!` if it is).

### Accepted — reply headers

The reply finaliser copies an Exchange header to the HTTP response unless a
rule below excludes it. ADR-0057 defines the rules. It sorts header names
into three buckets and treats two fields as re-derived. See
`docs/adr/0057-http-header-emission-policy.md`.

Excluded from the response:

- Headers starting with `Camel` (Camel-internal namespace).
- Hop-by-hop / framing (compatibility set: RFC 2616 section 13.5.1
  conventions + RFC 7230 per-section definitions; RFC 7230 section 6.1
  mandates removing `Connection` and connection-option-named headers):
  `connection`, `keep-alive`, `proxy-authenticate`, `proxy-authorization`,
  `te`, `trailer`, `transfer-encoding`, `upgrade`, `proxy-connection`.
- Request-only (client-side, not valid in a response): `host`, `user-agent`,
  `accept`, `accept-encoding`, `accept-language`, `accept-charset`,
  `accept-datetime`, `authorization`, `cookie`, `expect`, `from`, `if-match`,
  `if-modified-since`, `if-none-match`, `if-range`, `if-unmodified-since`,
  `max-forwards`, `range`, `referer`.
- Server-owned (RFC 7231 section 7.1.1.2): `date`. Only the origin server
  sets this field.
- Re-derived by the HTTP server, not copied from the Exchange:
  `content-length` and `content-type`. Use the explicit Content-Type
  derivation above.
- Dynamic Connection-named headers: any header that a `Connection` field
  value names is hop-by-hop for that connection (RFC 7230 section 6.1).

Emitted (valid response headers, NOT excluded):

- `cache-control`, `pragma`, `warning`, `via`. RFC 7231 and RFC 7234 define
  these for server-to-client communication. A bridging proxy passes them
  through.

- User-supplied `Content-Type` header on the Exchange overrides the inferred content type.

### Rejected

- `Body::Stream` already consumed before reply → `500 Internal Server Error` + empty body (system-broken `error!` at lib.rs:1109).
- Pipeline returns `Err(CamelError::Unauthenticated(msg))` → `401 Unauthorized` + `WWW-Authenticate: Bearer` + body "Unauthorized".
- Pipeline returns `Err(CamelError::Unauthorized(msg))` → `403 Forbidden` + body "Forbidden".
- Pipeline returns `Err(CamelError::ConsumerStopping)` → `503 Service Unavailable` + body "Service Unavailable". Fires only when an exchange is aborted past the drain grace window (ADR-0043 amend).
- Pipeline returns `Err(_)` (any other error) → `500 Internal Server Error` + body "Internal Server Error".

### Silent behaviour forbidden

- There is NO `Err(CamelError::Stopped)` special-case arm. Stop arrives as `Ok(ex)` (ADR-0024) and is handled by the same reply-finaliser path as normal completion. A future regression that re-introduces a Stop special-case is a bug.

### Stop-specific contract

- `stop: true` after `set_body` + `set_header("CamelHttpResponseCode", "409")` produces HTTP `409` + the body — identical to a route that reaches the end of the pipeline with the same Exchange state.
- `stop: true` with no body and no status header produces HTTP `200` + empty body (NOT `204` — the legacy `204` default was Bug B adjacent behaviour, removed in Phase 3).
