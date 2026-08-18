# security-keycloak

Keycloak-style local simulation using the native auth pipeline. It does not contact Keycloak, and no Docker or external Keycloak is required.

## Running

```bash
cargo run -p security-keycloak
```

No extra setup required.

## What it shows

- **Static credential store** — `NativeCredentialStore` registers two principals (alice with admin+user roles, bob with viewer role) with pre-shared plaintext tokens
- **Token validation** — `StaticTokenAuthenticator` validates bearer tokens against the store
- **Role-based SecurityPolicy** — `RolePolicy` evaluates the `Authorization: Bearer <token>` header and checks for required roles
- **Granted / Denied decisions** — Alice (admin role) is granted access; Bob (viewer role) is denied

## How it works

1. `NativeCredentialStore::try_new()` registers alice (admin,user) and bob (viewer) with plaintext tokens
2. `StaticTokenAuthenticator` wraps the store and validates bearer tokens against it
3. `RolePolicy::evaluate()` reads the `Authorization` header, validates the bearer token, and checks for the "admin" role
4. `BearerInjectingPolicy` wraps the `RolePolicy` to inject the `Authorization` header — the timer consumer produces no HTTP headers, so the wrapper simulates the transport
5. The route attaches the policy via `RouteBuilder::security_policy()` so every exchange is authorized before processing

## Route

```
admin-only-route:
  timer:tick?period=2000&repeatCount=2
    -> [BearerInjectingPolicy -> RolePolicy[admin]]
    -> log:info?showHeaders=true
```

The timer produces 2 exchanges. Each gets Alice's bearer token injected as a header by the wrapper policy. The security policy validates the token and checks for the "admin" role before the log step.

## Production usage

In production, the bearer token arrives from HTTP consumers and the `Authorization` header is already present — no wrapper policy needed. Replace the static token store with a real Keycloak OIDC endpoint. Real Keycloak JWKS validation coverage lives in `crates/camel-test/tests/keycloak_jwks_test.rs` and runs with:

```bash
cargo test -p camel-test --features integration-tests --test keycloak_jwks_test -- --nocapture
```

## Expected output

```
=== Keycloak Security Example ===
Issuer:  https://keycloak.example.com/realms/test
Alice:   admin,user roles -> static auth
Bob:     viewer role      -> static auth

--- Validation ---
Alice OK subject=alice roles=["admin", "user"]
Bob: VALID  (subject=bob, roles=["viewer"])

--- Role-Based Security Policy ---
Alice vs RolePolicy[admin]: GRANTED (subject=alice)
Bob vs RolePolicy[admin]:   DENIED (missing required role(s): admin)

--- Route with Security Policy ---
Route: timer -> [BearerInjectingPolicy -> RolePolicy[admin]] -> log
Wrapper injects Authorization header before RolePolicy evaluates.
Running for ~5s...

[info] Exchange[... headers={authorization: Bearer alice-token ...} ...]
[info] Exchange[... headers={authorization: Bearer alice-token ...} ...]

--- Summary ---
Alice (admin,user): GRANTED - has admin role
Bob (viewer):       DENIED  - missing admin role

Done.
```

## Files

```
examples/security-keycloak/
  Cargo.toml          Example crate (workspace dependencies)
  README.md           This file
  src/
    main.rs           Host example: static credential store, token validation, role policy, secured route
```