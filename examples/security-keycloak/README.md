# security-keycloak

Keycloak-style local simulation using the native auth pipeline. It does not contact Keycloak, and no Docker or external Keycloak is required.

## Running

```bash
cargo run -p security-keycloak
```

No extra setup required. The RSA signing key is in `fixtures/test_key.pem`.

## What it shows

- **JWT issuance** — `NativeTokenIssuer` creates signed JWTs with embedded roles (simulates Keycloak token endpoint)
- **JWKS validation** — `NativeJwksProvider` + `LocalJwtValidator` verify tokens against the signing key
- **Role-based SecurityPolicy** — `RolePolicy` evaluates the `Authorization: Bearer <token>` header and checks for required roles
- **Granted / Denied decisions** — Alice (admin role) is granted access; Bob (viewer role) is denied

## How it works

1. `NativeSigningKey::from_pem()` loads the RSA private key
2. `M2mClientStore` registers two machine-to-machine clients (alice with admin+user roles, bob with viewer role)
3. `NativeTokenIssuer::try_new()` creates a token issuer bound to the key and client store
4. `issue_token()` is called for each client, returning a signed JWT
5. `LocalJwtValidator` validates tokens using `NativeJwksProvider` and `JsonPointerClaimsMapper`
6. `RolePolicy::evaluate()` reads the `Authorization` header, validates the bearer token, and checks roles
7. The route attaches the policy via `RouteBuilder::security_policy()` so every exchange is authorized before processing

## Route

```
admin-only-route:
  timer:tick?period=2000&repeatCount=2
    -> set_header("authorization", Bearer <alice_token>)
    -> RolePolicy[admin]
    -> log:info?showHeaders=true
```

The timer produces 2 exchanges. Each gets Alice's bearer token injected as a header. The security policy validates the token and checks for the "admin" role before the log step.

## Production usage

In production, replace `NativeTokenIssuer` with a real Keycloak OIDC endpoint. Real Keycloak JWKS validation coverage lives in `crates/camel-test/tests/keycloak_jwks_test.rs` and runs with:

```bash
cargo test -p camel-test --features integration-tests --test keycloak_jwks_test -- --nocapture
```

## Expected output

```
=== Keycloak Security Example ===
Issuer:  https://keycloak.example.com/realms/test
Alice:   admin,user roles -> token issued
Bob:     viewer role      -> token issued

--- Token Validation ---
Alice token: VALID  (subject=alice, roles=["admin", "user"])
Bob token:   VALID  (subject=bob, roles=["viewer"])

--- Role-Based Security Policy ---
Alice vs RolePolicy[admin]: GRANTED (subject=alice)
Bob vs RolePolicy[admin]:   DENIED (...)

--- Route with Security Policy ---
Route: timer -> set_header(Bearer alice token) -> RolePolicy[admin] -> log
Running for ~5s...

[info] Exchange[... headers={authorization: Bearer eyJ...} ...]
[info] Exchange[... headers={authorization: Bearer eyJ...} ...]

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
  fixtures/
    test_key.pem      2048-bit RSA private key (test only)
  src/
    main.rs           Host example: token issuance, validation, role policy, secured route
```
