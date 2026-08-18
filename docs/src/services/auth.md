# Authentication and authorization

The `camel-auth` crate validates bearer tokens, maps claims into a
`Principal`, and evaluates authorization decisions for route-level
`security_policy`. It is provider-neutral. OIDC presets for specific
providers live in component crates such as `camel-component-keycloak`.

## Architecture

The auth pipeline has three layers:

1. **TokenAuthenticator** validates a bearer or API token and returns a `Principal`. Implementations include `IntrospectionAuthenticator` (RFC 7662), `StaticTokenAuthenticator`, and `LocalJwtValidator`.
2. **ClaimsMapper** maps token or introspection claims into `Principal` fields: subject, roles, scopes, issuer, audience. `JsonPointerClaimsMapper` resolves JSON Pointer paths, so any OIDC provider works without code.
3. **PermissionEvaluator** evaluates resource, action, and scope requests and returns a `PermissionDecision`. Route-level `security_policy.permission` calls it.

The enforcement boundary is `SecurityPolicyLayer` in camel-core. It evaluates BEFORE route steps run. A granted decision stores `Principal` properties on the Exchange. A denied decision returns `Unauthorized` into route error handling.

See [ADR-0010](../adr/0010-security-policy-pre-pipeline-authorization.md) for the pre-pipeline authorization decision.

## Native auth

The native auth pipeline reproduces a Keycloak-style flow without external dependencies. It validates static credentials against a local store and applies role-based policies. The same pipeline works with a real Keycloak through `camel-component-keycloak`.

Register static credentials in `Camel.toml`. Each `[[security.native.credentials]]` entry binds a `subject` to a credential, supplied inline (`secret`) or by environment-variable reference (`secret_env`). Roles and scopes are optional:

```toml
[security.native]
subject = "native-user"
issuer = "native"

[[security.native.credentials]]
subject = "svc-orders"
secret_env = "ORDERS_SECRET"
roles = ["service"]
scopes = ["read:orders", "write:orders"]
```

The CLI builds a `NativeCredentialStore` from these entries and wraps it in a `StaticTokenAuthenticator`. Each entry maps its credential to a `Principal` with the entry's roles and scopes.

The example defines the bearer values it presents to the authenticator:

```rust,ignore
{{#include ../../../examples/security-keycloak/src/main.rs:keycloak-token-issuance}}
```

Validate a bearer value with `StaticTokenAuthenticator`:

```rust,ignore
{{#include ../../../examples/security-keycloak/src/main.rs:keycloak-validation}}
```

Apply a `RolePolicy` that checks for required roles:

```rust,ignore
{{#include ../../../examples/security-keycloak/src/main.rs:keycloak-role-policy}}
```

<details>
<summary>YAML equivalent for the secured route</summary>

The Rust example wraps `RolePolicy` in `BearerInjectingPolicy` to inject a
static demo token. In a YAML route, the bearer token arrives in the transport
`Authorization` header and the policy is declarative.

```yaml
routes:
  - id: admin-only-route
    from: timer:tick?period=2000&repeatCount=2
    security_policy:
      roles: [admin]
      all_required: true
    steps:
      - to: log:info?showHeaders=true
```

YAML `security_policy` accepts one of `roles`, `scopes`, `ref`, `wasm`, or
`permission` as its policy form. An optional `credential_sources` list
declares where the credential comes from (see below). Routes with
`security_policy` do not support the canonical hot-reload path.

</details>

## Credential sources

By default, a route reads its credential from the `Authorization` header
(ADR-0033). A browser cannot set that header on an `<img src>` request. Map
tiles served to Leaflet, MapLibre, or OpenLayers need another transport. The
`credential_sources` key names the extraction sources:

```yaml
routes:
  - id: tile-route
    from: "http://0.0.0.0:8080/tiles"
    security_policy:
      roles: [tile-user]
      credential_sources:
        - cookie: { name: session }
        - authorization_header
    steps:
      - to: "log:info"
```

Each entry names one source:

| Form | Meaning |
|---|---|
| `authorization_header` | Bearer token in the `Authorization` header |
| `query_param: { param: <name> }` | Token in a query parameter |
| `cookie: { name: <name> }` | Token in a cookie |
| `header: { name: <name> }` | API key in a named custom header |

Extraction runs in the declared order. The first source that supplies a
credential wins; later sources are fallbacks. When no source supplies a
credential, the request fails with `401` before policy evaluation. Store
lookups run in constant time.

`http://` and `ws://` consumers support the key. On a `ws://` route, a
`roles` or `scopes` policy that uses a non-header source requires
`trust_upstream_principal: true`; without the flag the consumer treats the
token as unauthenticated.

Load-time validation rejects malformed declarations: an unknown source form,
an empty cookie name, or a header name that is not a valid RFC 9110 token.

Diagnostic records never render a declared credential value. The 401 reply
body carries a generic reason only (ADR-0051). The operator sets
`SameSite=Lax` (or stricter) and `HttpOnly` on session cookies where the
cookie is issued. Cookie auth on state-changing verbs still requires CSRF
defense.

See [ADR-0059](../adr/0059-auth-extraction-path-divergence.md) for the
extraction architecture.

## WASM authorization policy

A WASM plugin can serve as an authorization policy. The plugin reads `camel.auth.*` properties from the Exchange and returns a grant or denial.

```rust,ignore
{{#include ../../../examples/security-wasm-policy/src/main.rs:wasm-policy-setup}}
```

> **Note:** Service registration is Rust API only. YAML routes compile to
> the same `RouteDefinition`. The service wiring stays in application
> code.

<details>
<summary>YAML equivalent for a production route</summary>

Register the WASM policy in `Camel.toml` under
`[security.policies.wasm.<name>]`, then reference it by name in the route.

```yaml
routes:
  - id: wasm-secured-route
    from: timer:tick?period=1000&repeatCount=5
    security_policy:
      wasm: role-check
    steps:
      - to: log:info?showHeaders=true
```

</details>

For production routes, prefer `Camel.toml` registration through `[security.policies.wasm.<name>]` with YAML `security_policy: wasm: <name>`.

See [ADR-0050](../adr/0050-wasm-sandbox-capability-posture.md) for the WASM sandbox capability posture.

## Security defaults

The startup-validation phase enforces fail-closed security defaults. Routes refuse to start when required configuration is missing. See [ADR-0033](../adr/0033-security-defaults-fail-closed-startup-validation.md) for the full policy.

**Reference**: [camel-auth crate](https://github.com/kennycallado/rust-camel/blob/main/crates/services/camel-auth/CONTEXT.md)
