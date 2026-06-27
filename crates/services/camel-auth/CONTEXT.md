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
