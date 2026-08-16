# Proposal: credential-sources

## Why

External report from a production proxy-cache deployment (v0.28.0, verified on
main). Browser-facing tile services (`<img src>` from Leaflet, MapLibre, OpenLayers)
cannot attach custom headers. `Authorization: Bearer` is structurally impossible
for them. A cookie is the only viable transport.

Today `security_policy` on `from: http://` routes accepts Bearer tokens from the
`Authorization` header only. Users work around this with hand-rolled Rhai cookie
gates. Those gates have real costs: non-constant-time comparison, script errors
map to 500 instead of 401, no shared rate-limit or expiry logic, duplication per
route.

The abstraction already exists and is complete for the existing Bearer,
query, and cookie sources: `CredentialSource` plus `extract_token_multi()` in
`camel-auth`. But HTTP never uses it. The WS plumbing exists yet stays inert
(`SecurityContext::from_arc` hardcodes the default source list).
`ApiKeyAuthenticator` is complete but dead: no DSL surface reaches it, and
its only workspace reference is a re-export. Phase 4 extends the abstraction
with a custom-header variant for API-key style auth.

This is a component-contract inconsistency, not a missing feature. The fix is
wiring, not new abstraction.

## What Changes

- Add `credential_sources` to the route `security_policy` DSL block. Forms:
  `authorization_header`, query parameter, cookie, custom header.
- Thread the list through `SecurityPolicyConfig` into `RolePolicy` /
  `ScopePolicy` evaluation, so `authenticate()` extracts via
  `extract_token_multi` instead of a hardcoded `strip_prefix("Bearer ")`.
- Add a `Header{name}` variant to `CredentialSource` so API-key style custom
  headers work with the existing `StaticTokenAuthenticator` (constant-time
  store lookup) — this supersedes exposing `ApiKeyAuthenticator` itself.
- Activate the inert WS wiring (`with_credential_sources`) so both consumers
  resolve the same declared sources.
- Extend ADR-0051 redaction to the HTTP request-logging path (query, cookie,
  custom header values).
- Short ADR documenting the two auth paths (policy-owned vs component-owned
  extraction).
- Spec delta in `openspec/specs/security/`; CSRF/SameSite/HttpOnly guidance
  for cookie transport on browser-facing routes.

Excluded: gRPC consumer (same structural gap, `server.rs:434`; cheap fast-follow
after the `authenticate()` refactor — separate bd issue), Keycloak/WASM policy
providers, `ApiKeyAuthenticator` Rust API redesign.

## Acceptance criteria

- A `from: http://` route with `security_policy` (roles or scopes) and a cookie
  source authenticates a cookie-transported token; `<img>` requests succeed
  without an `Authorization` header.
- Default behavior when `credential_sources` is absent: Bearer from the
  `Authorization` header, byte-identical for well-formed requests
  (ADR-0033 fail-closed). One disclosed exception: RFC 9110 scheme parsing
  unification (see risk budget and ADR-0059) — case-insensitive scheme and
  whitespace tolerance widen acceptance only; an empty token after the
  scheme is treated as an absent source and falls to the trust branch.
- Cookie, query, and custom-header values stay absent from diagnostic
  tracing records and HTTP error replies (ADR-0051); `lint-secrets` clean.
- WS routes resolve declared sources; default WS behavior unchanged.
- Constant-time comparison on the native store path for all sources.
- Unauthenticated requests map to 401 (not 500) on the native path.
- Spec delta merged; `CONTEXT.md` of camel-auth and camel-http updated.

## Risk budget

Acceptable: additive DSL field (`deny_unknown_fields` keeps old files valid),
schema regeneration, test churn in camel-auth/camel-dsl.
Out of bounds: any behavior change for routes without `credential_sources`
(one disclosed exception: the RFC 9110 Bearer-scheme parsing unification
with WS extraction — case-insensitive scheme, whitespace tolerance; widens
acceptance only, no previously-granted route changes outcome; ADR-0059),
weakening of redaction, timing regressions on token comparison, new
authentication abstractions.

Bd: rc-7x1z. Affected crates: camel-dsl, camel-api, camel-auth, camel-core,
camel-http (components/camel-ws), camel-cli (schema regen), docs.
