# camel-auth

Provider-neutral OIDC authentication and authorization service for rust-camel.

Supports local JWT validation via JWKS, remote token introspection (RFC 7662),
built-in role/scope policies, permission evaluation with caching, and a native
auth mode that works without an external identity provider. Configurable claim
mapping via `ClaimsMapper` enables any OIDC-compliant provider. Provider-specific
presets live in their respective component crates (e.g. `camel-component-keycloak`).

## Architecture

`camel-auth` sits between `camel-api` (which defines core types like `SecurityPolicy`,
`AuthorizationDecision`, and `Principal`) and provider-specific component crates:

```
┌──────────────────────────────┐
│  camel-api                   │   SecurityPolicy, Principal, AuthorizationDecision
└──────────────┬───────────────┘
               │ re-exports
┌──────────────▼───────────────┐
│  camel-auth                  │   JWT validation, introspection, policies, registry
└──────────────┬───────────────┘
               │ used by
┌──────────────▼───────────────┐
│  camel-component-keycloak    │   Provider-specific presets
│  camel-component-http        │   Bearer token extraction
└──────────────────────────────┘
```

## Installation

Add to `Cargo.toml` (workspace dependency):

```toml
[dependencies]
camel-auth.workspace = true
```

## Modules

| Module | Description |
|--------|-------------|
| `bearer` / `bearer_token_layer` | Token extraction from requests and Tower layers for injecting/validating bearer tokens |
| `built_in` | Built-in policies: `RolePolicy` and `ScopePolicy` |
| `claims` | Claim mapping via `ClaimsMapper`, `JsonPointerClaimsMapper`, and `ClaimPaths` |
| `credential_source` | Multi-source credential extraction (header, query parameter, cookie) with `CredentialSource` |
| `introspection` | Remote token introspection (RFC 7662) with `TokenIntrospector` and `CachingTokenIntrospector` |
| `introspection_auth` | `IntrospectionAuthenticator` bridging introspection into the authenticator trait |
| `jwks` | JWKS key management: `Jwk`, `JwksProvider`, `RemoteJwksProvider`, HTTPS URI validation |
| `jwt` | Local JWT validation: `JwtValidator` and `LocalJwtValidator` |
| `oauth2` | OAuth2 client credentials flow: `ClientCredentialsProvider` and `TokenProvider` |
| `permission` | Permission evaluation trait `PermissionEvaluator`, `PermissionRequest`, `PermissionDecision` |
| `permission_policy` | `PermissionPolicy` bridge connecting permission evaluation to the security policy system |
| `permission_cache` | `CachingPermissionEvaluator` with `PermissionCacheOptions` |
| `registry` | `NamedRegistry<T>` implementations: `SecurityPolicyRegistry`, `PermissionEvaluatorRegistry` |
| `token_authenticator` | `TokenAuthenticator` trait with blanket impl for `JwtValidator` |
| `native_auth` | Built-in auth without external IdP: `ApiKeyAuthenticator`, `StaticTokenAuthenticator` |
| `native_issuer` | `NativeTokenIssuer` for issuing tokens in native mode, `NativeSigningKey` |
| `native_jwks` | `NativeJwksProvider` serving JWKS from native signing keys |
| `native_client_store` | `M2mClientStore` and `M2mClient` for machine-to-machine native auth |
| `types` | `AuthError` enum for all authentication/authorization error cases |

## Quick Example

JWT validation with role-based access control:

```rust
use camel_auth::{
    LocalJwtValidator, JwtValidator, RolePolicy,
    ClaimsMapper, JsonPointerClaimsMapper, ClaimPaths,
    SecurityPolicyRegistry, BearerTokenLayer,
};
use camel_api::security_policy::SecurityPolicy;

// Create a JWT validator backed by a JWKS endpoint
let validator = LocalJwtValidator::from_jwks_url("https://auth.example.com/.well-known/jwks.json").await?;

// Map claims from the token into roles used by policies
let mapper = JsonPointerClaimsMapper::new(ClaimPaths::roles_from("/realm_access/roles"));

// Register a role policy
let role_policy = RolePolicy::new(vec!["admin".into()], mapper);
let mut registry = SecurityPolicyRegistry::new();
registry.register("admin-only", role_policy);

// Use as a Tower layer to extract and validate bearer tokens
let layer = BearerTokenLayer::new(validator);
```

## Error Handling

All errors are captured in `AuthError`:

```rust
pub enum AuthError {
    InvalidToken(String),
    ExpiredToken,
    IntrospectionFailed(String),
    JwksFetchFailed(String),
    Unauthorized(String),
    PermissionDenied(String),
    // ...
}
```

## License

Apache-2.0
