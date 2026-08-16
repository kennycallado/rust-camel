# Auth Service

Provider-neutral authentication and authorization for rust-camel. Validates bearer/API tokens,
maps claims into a `Principal`, and evaluates authorization decisions for route-level
`security_policy`. OIDC-compliant by configuration; provider-specific presets live in component
crates (e.g. `camel-component-keycloak`).

> **Scope boundary.** The route-level **contract types** — `SecurityPolicy`, `Principal`,
> `AuthorizationDecision`, `SecurityPolicyConfig` — are defined in
> [`crates/camel-api/CONTEXT.md`](../../camel-api/CONTEXT.md), so camel-core and camel-dsl can use
> them without depending on this crate. The enforcement **boundary** (`SecurityPolicyLayer`,
> pre-pipeline, ADR-0010) lives in camel-core. This crate owns the **decision sources**: token
> validation, claim mapping, and permission evaluation. See also the parent
> [`crates/services/CONTEXT.md`](../CONTEXT.md), which glosses the cross-cutting auth terms.

## Language

**TokenAuthenticator**:
Provider-neutral contract (`token_authenticator.rs`) that validates a bearer/API token and returns a
`Principal`. Implementations: `IntrospectionAuthenticator` (RFC 7662), `ApiKeyAuthenticator`,
`StaticTokenAuthenticator`.
_Avoid_: Keycloak client, JWT parser (those are specific mechanisms, not the contract)

**ClaimsMapper**:
Provider-neutral mapping (`claims.rs`) from token/introspection claims into `Principal` fields
(subject, roles, scopes, issuer, audience). `JsonPointerClaimsMapper` resolves via JSON Pointer
paths so any OIDC provider can be configured without code.
_Avoid_: Keycloak mapping, claim parser

**JwtValidator / JwksProvider**:
`JwtValidator` (`jwt.rs`) verifies JWT signature/claims; `JwksProvider` (`jwks.rs`) supplies signing
keys (`RemoteJwksProvider` fetches a remote JWKS; `NativeJwksProvider` serves locally-issued keys).
_Avoid_: token verifier (use the specific trait), key store

**PermissionEvaluator**:
Authorization engine (`permission.rs`) backing `security_policy.permission`. Evaluates a
`PermissionRequest` (resource/action/scope) and returns a `PermissionDecision`. One decision *source*
behind the SecurityPolicy boundary — not the boundary itself. `CachingPermissionEvaluator` wraps it.
_Avoid_: SecurityPolicy (SecurityPolicy is the route boundary type in camel-api; this is one source)

**SecurityPolicyRegistry / PermissionEvaluatorRegistry**:
Name→implementation lookups (`registry.rs`) so a `security_policy.ref` / `permission` can resolve a
registered evaluator by name at route-compile time.
_Avoid_: policy map, evaluator factory

**NativeTokenIssuer**:
Self-contained token issuer (`native_issuer.rs`) for the built-in (non-OIDC) auth path:
issues/signs tokens via `NativeSigningKey` against an `M2mClientStore` of machine-to-machine clients.
_Avoid_: OAuth server, identity provider (it is a minimal native issuer, not a full IdP)

## Credential sources

A route declares where its credential comes from via `credential_sources`.
The list is part of the `security_policy` block. It maps to the
`CredentialSource` enum (`camel_api::security_policy::CredentialSource`,
re-exported here). The current variants:

- `AuthorizationHeader` — the `Authorization` header, Bearer scheme.
- `QueryParam { param }` — a named query parameter.
- `Cookie { name }` — a named cookie.
- `Header { name }` — an API key in a named custom header (name validated as an RFC 9110 token at load time).

When the key is absent, the effective list is `[AuthorizationHeader]` only
(fail-closed, ADR-0033). Extraction tries the declared sources in order; first
match wins. A miss on every source maps to `Unauthenticated`, never a panic or
a 500. Malformed input (a cookie without `name=`, a query value that fails to
parse, a header value rejected by `http::HeaderValue::from_str`) is treated as
an absent source (ADR-0032).

Every extracted value flows through the same constant-time store lookup
(`NativeCredentialStore::lookup`, `native_auth.rs`). Extraction differs per
source; comparison does not. No source introduces an early-exit comparison path.

**`trust_upstream_principal` semantics.** On a component-owned path (WS), the
flag means "accept the principal the component authenticated". The component
extracts the credential, validates it, stores the principal via
`store_principal_properties`, then calls `policy.evaluate`. The spoof caveat
(the property could be stamped by an upstream filter) applies only to the layer
path, where a policy reads a principal that an arbitrary upstream step may have
written. On the component path the component gates evaluation on successful
authentication, so the caveat does not apply. The mechanism never sets the flag
implicitly — only the route YAML does.

The store/read pair for the preloaded principal is one format: a JSON string
under `camel.auth.principal` (`store_principal_properties` writes it;
`principal_from_exchange` reads it). The trust branch delegates to the canonical
reader, so a component-stored principal round-trips (ADR-0059).

The Bearer scheme is parsed per RFC 9110 (RFC 7235): case-insensitive and
whitespace-tolerant. An empty token after the scheme is an absent source.

## Batch 6 — Security hardening

### JWKS body cap (`fn fetch_and_store`, jwks.rs:40)

`MAX_JWKS_BODY_BYTES = 1024 * 1024` (1 MiB). Applied in `fetch_and_store()`:
- **Content-Length pre-check** (`fn fetch_and_store`, jwks.rs:103-108): rejects before buffering if declared size exceeds cap.
- **Streaming abort** (`fn fetch_and_store`, jwks.rs:116-128): cumulative chunk bytes checked per-iteration; aborts with error if cap exceeded mid-stream.

### JWKS max-age clamp (`fn fetch_and_store`, jwks.rs:130-143)

- `MIN_JWKS_TTL_SECS = 60`, `MAX_JWKS_TTL_SECS = 3600` (`fn fetch_and_store`, jwks.rs:41-42).
- Only applied to the *parsed* `max-age` value from `Cache-Control` header.
- When `Cache-Control` is absent or lacks `max-age`, the `default_ttl` (300s) is used unclamped.

### JWKS DNS pinning (`fn build_ssrf_pinned_client`, jwks.rs:52-58, http_client.rs:16-58)

`RemoteJwksProvider::new()` calls `build_ssrf_pinned_client()` which:
- Validates the URI is public HTTPS (`validate_uri()` with `SsrfPolicy`).
- Resolves DNS with 5s timeout, filters through `camel_api::is_ssrf_blocked_ip`.
- Pins validated IPs via `reqwest::Client::resolve_to_addrs()` — eliminates TOCTOU window.
- Sets `redirect::Policy::none()` and 5s connect / 10s request timeout.

### OAuth2 SSRF pinning (`ClientCredentialsProvider::new`, oauth2.rs:93-99)

`ClientCredentialsProvider::new()` calls the same `build_ssrf_pinned_client()` on the token endpoint,
with 10s connect timeout and 30s request timeout.

### Introspection SSRF pinning (`CachingTokenIntrospector::new`, introspection.rs:86-92)

`CachingTokenIntrospector::new()` calls `build_ssrf_pinned_client()` on the introspection endpoint,
with 5s connect timeout and 10s request timeout.

### Introspection exp/nbf enforcement (`fn authenticate_bearer`, introspection_auth.rs:42-52)

`IntrospectionAuthenticator::authenticate()` enforces:
- `exp < now` → `AuthError::TokenExpired` (rejects expired tokens).
- `nbf > now` → `AuthError::TokenInvalid` (rejects not-yet-valid tokens).

### JWT alg/use matching (`fn key_matches`, jwt.rs:60-70)

`key_matches()` requires:
- `kid` matches token header.
- `alg` is absent (spec default) OR equals `EXPECTED_ALG` (`"RS256"`).
- `use` is absent (spec default) OR equals `"sig"`.

### Constant-time comparison (`fn constant_time_eq`, native_client_store.rs:14-18, 142-146)

`constant_time_eq(a, b)` compares byte slices in constant time (no early-exit on mismatch).
Applied to:
- `client_id` lookup in `M2mClientStore::authenticate()`.
- Secret hash comparison via the same helper.

### Zeroize (multiple files)

`Zeroizing<String>` applied to all secret-bearing fields:
- `ClientCredentialsProvider.client_secret` (`struct ClientCredentialsProvider`, oauth2.rs:60).
- `CachingTokenIntrospector.client_secret` (`struct CachingTokenIntrospector`, introspection.rs:70).
- `CachedToken.access_token` (`struct TokenResponse`, oauth2.rs:29), `TokenResponse.access_token` (oauth2.rs:36).
- `M2mClient.secret_value` (`enum M2mClientSecret`, native_client_store.rs:40), `M2mClientSecret` enum value (native_client_store.rs:35).
- `NativeClient.secret_value` (`enum NativeCredentialSecret`, native_auth.rs:21), `NativeClientSecret` enum value (native_auth.rs:16).
- `native_issuer::TokenResponse.access_token` (`struct TokenResponse`, native_issuer.rs:124).

> **Debug redaction is separate.** In zeroize 1.9.0, `Zeroizing<T>` derives `Debug`. Its
> implementation prints the inner value. `Zeroizing<String>` clears memory on drop, but it does not
> redact formatted output. Each type that contains a secret must also implement `Debug` manually.
>
> Follow the redacting implementations for `NativeCredentialSecret`, `M2mClientSecret`,
> `ExtractedToken`, `CachingTokenIntrospector`, `ClientCredentialsProvider`,
> `IntrospectionAuthenticator`, `NativeSigningKey`, and `NativeTokenIssuer`.
>
> Both known gaps are resolved. `native_issuer::TokenResponse` (rc-c9xo) and
> `oauth2::TokenResponse` (rc-fvl5) now use manual `Debug` implementations that
> redact `access_token` as `[REDACTED]`. Regression tests verify sentinel exclusion.
> This supports the ADR-0032 trust boundary by keeping token data out of
> diagnostic sinks.

## Example dialogue

> "I declared `security_policy.permission` on a route. Which crate decides allow/deny?"
> "The boundary is `SecurityPolicyLayer` in camel-core (pre-pipeline, ADR-0010). It calls a
> `PermissionEvaluator` from this crate, resolved by name via `PermissionEvaluatorRegistry`. The
> evaluator returns a `PermissionDecision`; a grant stores `Principal` properties on the Exchange,
> a denial returns `Unauthorized` into route error handling."
>
> "Where is the `Principal` type, since both camel-core and this crate use it?"
> "In camel-api (`security_policy.rs`), re-exported here for convenience. Contract types stay in
> camel-api so core/dsl avoid depending on the auth service."
>
> "How do I support a non-Keycloak OIDC provider?"
> "Configure a `JsonPointerClaimsMapper` with that provider's claim paths and point a
> `JwtValidator`/`JwksProvider` at its JWKS. No code change — provider-specific presets are only a
> convenience layer in component crates."
