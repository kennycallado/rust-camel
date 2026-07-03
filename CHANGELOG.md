# Changelog

## Security Hardening — Breaking (v1.0.0)

### ControlBus: authorizedRoutes required (R4-H1)
- **Breaking:** `CamelRouteId` header override removed. Target `routeId`
  must be declared statically in the URI.
- **New required param:** `authorizedRoutes` (comma-separated allowlist).
  Without it, all controlbus commands are rejected (fail-closed).
- **Self-restart denied:** `routeId` cannot equal the calling route's ID.
- **Migration:** Add `&authorizedRoutes=<route-id>` to every
  `controlbus:route` URI.

### SQL: allow_dynamic_query default false (H7)
- **Breaking:** `CamelSql.Query` header and body-sourced queries are
  denied unless `allow_dynamic_query=true`.
- **Migration:** Add `allow_dynamic_query=true` to endpoint config for
  routes that use dynamic queries.

### Auth: trust_upstream_principal default false (H1)
- **Breaking:** `camel.auth.principal` exchange property fallback is
  denied unless `trust_upstream_principal=true` in the security policy.
- **Migration:** Add `trust_upstream_principal: true` to routes that
  rely on upstream-set principal properties.

### gRPC: default deadline 30s + connect_timeout 10s (H14)
- **Breaking:** gRPC RPCs now have a default 30s deadline.
- **Opt-out:** Set `default_deadline_ms=0` to restore unlimited.
- **New param:** `connect_timeout_ms` (default 10s).

### CSV: formula-injection neutralization (R3-H1)
- **Non-breaking:** Cells with leading `= + - @ \t \r` are auto-prefixed
  with `'`. No migration needed.

### Keycloak/LLM: hardened HTTP clients (H15)
- **Breaking:** Redirect following disabled (`Policy::none()`).
  Connect timeout 10s, request timeout 30s enforced.
- **SSRF validation:** Internal/private/loopback IPs rejected by default.
  Shared `is_ssrf_blocked_ip` helper in `camel-api`.
- **New param:** `allow_internal_urls` / `allowInternalUrls` (default false)
  for local dev against Keycloak instances on loopback.
