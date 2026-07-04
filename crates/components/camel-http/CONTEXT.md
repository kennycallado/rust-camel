## Log-level policy

Per ADR-0012, this component's `error!` sites are categorized as:

- **(e) outside-contract** (lib.rs L748, L774):
  - L748 = accept-loop error in `run_axum_server`. Calls `runtime.metrics().increment_errors(route_id, "e:http:accept")` BEFORE the `error!`. The metric is the operator signal; `error!` provides loud log visibility.
  - L774 = server task exited unexpectedly in `monitor_axum_task`. Calls `runtime.metrics().increment_errors(route_id, "e:http:server-task-exited")` BEFORE the `error!`. Same pattern.
  Both sites keep `error!` with `// log-policy: outside-contract`.

- **(c) system-broken** (lib.rs L1108): `Body::Stream` already consumed before HTTP reply — programming-contract violation in `dispatch_handler`. Keeps `error!` with `// log-policy: system-broken`. No metric call (operator alert via error! is the signal).

- **(a) handler-owned** (lib.rs L1210): pipeline error processing HTTP request → 500 response in `dispatch_handler`. Route ErrorHandler owns the ERROR. Downgraded to `warn!` with `// log-policy: handler-owned`. No metric call.

### warn! sites (ADR-0012 advisory)

- **(a) handler-owned** (lib.rs L1380): TLS verification disabled via `insecure=true` or `verify_peer=false` in `TlsConfig`. Emitted during `build_client()` when the caller opts out of certificate validation. `warn!` with `// log-policy: handler-owned`. No metric call — the operator is responsible for this config.

Reviewer: r_glm5.1 verifies these classifications against source at Phase C review time.

## Contract Surface

Per ADR-0024 and spec §3.4. Documents the accepted and rejected names/values for the HTTP consumer reply finaliser. Future bug reports check here first: if behaviour is in this surface, it's a feature request; if not, it's a bug.

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

- All Exchange headers EXCEPT:
  - Headers starting with `Camel` (Camel-internal namespace).
  - Hop-by-hop / request-side headers: `content-length`, `content-type` (use the explicit Content-Type derivation above), `transfer-encoding`, `connection`, `cache-control`, `date`, `pragma`, `trailer`, `upgrade`, `via`, `warning`, `host`, `user-agent`, `accept`, `accept-encoding`, `accept-language`, `accept-charset`, `authorization`, `proxy-authorization`, `cookie`, `expect`, `from`, `if-match`, `if-modified-since`, `if-none-match`, `if-range`, `if-unmodified-since`, `max-forwards`, `proxy-connection`, `range`, `referer`, `te`.
- User-supplied `Content-Type` header on the Exchange overrides the inferred content type.

### Rejected

- `Body::Stream` already consumed before reply → `500 Internal Server Error` + empty body (system-broken `error!` at lib.rs:1109).
- Pipeline returns `Err(CamelError::Unauthenticated(msg))` → `401 Unauthorized` + `WWW-Authenticate: Bearer` + body "Unauthorized".
- Pipeline returns `Err(CamelError::Unauthorized(msg))` → `403 Forbidden` + body "Forbidden".
- Pipeline returns `Err(_)` (any other error) → `500 Internal Server Error` + body "Internal Server Error".

### Silent behaviour forbidden

- There is NO `Err(CamelError::Stopped)` special-case arm. Stop arrives as `Ok(ex)` (ADR-0024) and is handled by the same reply-finaliser path as normal completion. A future regression that re-introduces a Stop special-case is a bug.

### Stop-specific contract

- `stop: true` after `set_body` + `set_header("CamelHttpResponseCode", "409")` produces HTTP `409` + the body — identical to a route that reaches the end of the pipeline with the same Exchange state.
- `stop: true` with no body and no status header produces HTTP `200` + empty body (NOT `204` — the legacy `204` default was Bug B adjacent behaviour, removed in Phase 3).
