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

The Producer attaches the exchange body only for entity-enclosing methods
(POST, PUT, PATCH). GET, HEAD, DELETE, OPTIONS, and TRACE send no body and log
one `warn!` when a non-empty body (or any stream body) is dropped. The body
stays consumed. No configuration override exists (Apache Camel
`HttpMethods.isEntityEnclosing` parity).

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
