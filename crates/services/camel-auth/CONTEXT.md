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

## Batch 6 — Security hardening

### JWKS body cap (jwks.rs:40)

`MAX_JWKS_BODY_BYTES = 1024 * 1024` (1 MiB). Applied in `fetch_and_store()`:
- **Content-Length pre-check** (jwks.rs:103-108): rejects before buffering if declared size exceeds cap.
- **Streaming abort** (jwks.rs:116-128): cumulative chunk bytes checked per-iteration; aborts with error if cap exceeded mid-stream.

### JWKS max-age clamp (jwks.rs:130-143)

- `MIN_JWKS_TTL_SECS = 60`, `MAX_JWKS_TTL_SECS = 3600` (jwks.rs:41-42).
- Only applied to the *parsed* `max-age` value from `Cache-Control` header.
- When `Cache-Control` is absent or lacks `max-age`, the `default_ttl` (300s) is used unclamped.

### JWKS DNS pinning (jwks.rs:52-58, http_client.rs:16-58)

`RemoteJwksProvider::new()` calls `build_ssrf_pinned_client()` which:
- Validates the URI is public HTTPS (`validate_uri()` with `SsrfPolicy`).
- Resolves DNS with 5s timeout, filters through `camel_api::is_ssrf_blocked_ip`.
- Pins validated IPs via `reqwest::Client::resolve_to_addrs()` — eliminates TOCTOU window.
- Sets `redirect::Policy::none()` and 5s connect / 10s request timeout.

### OAuth2 SSRF pinning (oauth2.rs:93-99)

`ClientCredentialsProvider::new()` calls the same `build_ssrf_pinned_client()` on the token endpoint,
with 10s connect timeout and 30s request timeout.

### Introspection SSRF pinning (introspection.rs:86-92)

`CachingTokenIntrospector::new()` calls `build_ssrf_pinned_client()` on the introspection endpoint,
with 5s connect timeout and 10s request timeout.

### Introspection exp/nbf enforcement (introspection_auth.rs:42-52)

`IntrospectionAuthenticator::authenticate()` enforces:
- `exp < now` → `AuthError::TokenExpired` (rejects expired tokens).
- `nbf > now` → `AuthError::TokenInvalid` (rejects not-yet-valid tokens).

### JWT alg/use matching (jwt.rs:60-70)

`key_matches()` requires:
- `kid` matches token header.
- `alg` is absent (spec default) OR equals `EXPECTED_ALG` (`"RS256"`).
- `use` is absent (spec default) OR equals `"sig"`.

### Constant-time comparison (native_client_store.rs:14-18, 142-146)

`constant_time_eq(a, b)` compares byte slices in constant time (no early-exit on mismatch).
Applied to:
- `client_id` lookup in `M2mClientStore::authenticate()`.
- Secret hash comparison via the same helper.

### Zeroize (multiple files)

`Zeroizing<String>` applied to all secret-bearing fields:
- `ClientCredentialsProvider.client_secret` (oauth2.rs:60).
- `CachingTokenIntrospector.client_secret` (introspection.rs:70).
- `CachedToken.access_token` (oauth2.rs:29), `TokenResponse.access_token` (oauth2.rs:36).
- `M2mClient.secret_value` (native_client_store.rs:40), `M2mClientSecret` enum value (native_client_store.rs:35).
- `NativeClient.secret_value` (native_auth.rs:21), `NativeClientSecret` enum value (native_auth.rs:16).

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
