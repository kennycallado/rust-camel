# Authentication and authorization

The `camel-auth` crate validates bearer tokens, maps claims into a
`Principal`, and evaluates authorization decisions for route-level
`security_policy`. It is provider-neutral. OIDC presets for specific
providers live in component crates such as `camel-component-keycloak`.

## Architecture

The auth pipeline has three layers:

1. **TokenAuthenticator** validates a bearer or API token and returns a `Principal`. Implementations include `IntrospectionAuthenticator` (RFC 7662), `ApiKeyAuthenticator`, `StaticTokenAuthenticator`, and `LocalJwtValidator`.
2. **ClaimsMapper** maps token or introspection claims into `Principal` fields: subject, roles, scopes, issuer, audience. `JsonPointerClaimsMapper` resolves JSON Pointer paths, so any OIDC provider works without code.
3. **PermissionEvaluator** evaluates resource, action, and scope requests and returns a `PermissionDecision`. Route-level `security_policy.permission` calls it.

The enforcement boundary is `SecurityPolicyLayer` in camel-core. It evaluates BEFORE route steps run. A granted decision stores `Principal` properties on the Exchange. A denied decision returns `Unauthorized` into route error handling.

See [ADR-0010](../adr/0010-security-policy-pre-pipeline-authorization.md) for the pre-pipeline authorization decision.

## Keycloak-style auth

The native auth pipeline reproduces a Keycloak flow without external dependencies. It issues JWTs, validates them, and applies role-based policies. The same pipeline works with a real Keycloak through `camel-component-keycloak`.

Issue tokens with `NativeTokenIssuer`:

```rust,ignore
{{#include ../../../examples/security-keycloak/src/main.rs:keycloak-token-issuance}}
```

Validate tokens with `LocalJwtValidator` and `NativeJwksProvider`:

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
`permission`. Routes with `security_policy` do not support the canonical
hot-reload path.

</details>

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
