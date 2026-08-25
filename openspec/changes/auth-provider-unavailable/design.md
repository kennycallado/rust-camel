# Design: auth-provider-unavailable

## Approach

Promote the flattened auth-provider-down signal to a typed kernel error variant, exactly
as ADR-0033 did for config validation and as `EndpointUri(EndpointUriError)` did for
endpoint-URI merge failures: contract types live in camel-api so transports can match
without depending on camel-auth (see crates/services/camel-auth/CONTEXT.md scope boundary).

1. **camel-api** (`src/error.rs`): new variant on the `#[non_exhaustive]` `CamelError`:

   ```rust
   /// Auth provider (JWKS/introspection/token endpoint) is unreachable or failing.
   /// Promotes the auth-provider-down signal from a stringly-typed ProcessorError
   /// to a matchable variant; transports map it to 503 / UNAVAILABLE.
   #[error("Auth provider unavailable: {0}")]
   AuthProviderUnavailable(String),
   ```

   **Error-handler compatibility (required):** `variant_name()` returns `"ProcessorError"`
   for the new variant — same aliasing pattern as `ProcessorErrorWithSource` (error.rs
   doc, spec §5.4) — so existing `doTry` catch-by-variant handlers keep matching exactly
   as today. `classify()` joins the `"processor"` arm. Both asserted by unit tests
   (`variant_name_tests` extension + classify test).

2. **camel-auth** (`src/types.rs`): `From<AuthError> for CamelError` maps
   `ProviderUnavailable(s)` → `CamelError::AuthProviderUnavailable(s)`. Update the unit
   tests that assert the old `ProcessorError` + magic-string shape (types.rs:84-88,
   token_authenticator.rs:154, introspection_auth.rs:226).

3. **camel-ws** (`ws_upgrade_auth_error`): replace the
   `ProcessorError(msg) if msg.contains("auth provider unavailable")` guard with
   `CamelError::AuthProviderUnavailable(_) => (SERVICE_UNAVAILABLE, "Service Unavailable")`.
   Unauthenticated → 401 and the 500 fallback stay as-is.

4. **camel-component-grpc** (`auth_error_to_status`): replace the string guard with
   `CamelError::AuthProviderUnavailable(msg) => tonic::Status::unavailable(msg)`.
   The error-log arm for the internal fallback stays as-is. MockAuthenticator
   (server.rs:1601) returns the typed variant.

5. **Tests asserting the magic string**: camel-component-keycloak
   tests/introspection.rs:85,104 switch from substring asserts to
   `matches!(err, CamelError::AuthProviderUnavailable(_))` (keeping
   detail-preservation asserts where they exist). camel-test ws_security_test.rs: its
   `MockOutcome::Error` (lines 23-47) constructs `ProcessorError(msg)` whose message drove
   the 503-vs-500 mapping — add a distinct `MockOutcome::ProviderUnavailable` arm
   producing `CamelError::AuthProviderUnavailable`, retain the generic `Error` arm for the
   500 regression. New regression tests in camel-ws and camel-component-grpc assert
   503/UNAVAILABLE for the variant and 500/INTERNAL for a generic ProcessorError — proving
   status selection is variant-based, wording-independent.

## Affected crates

- camel-api: +1 enum variant, classify arm
- camel-auth: From-impl arm, 3 unit-test updates
- camel-ws: matcher swap + regression test
- camel-component-grpc: matcher swap, mock update, regression test
- camel-component-keycloak: 2 test asserts
- camel-test: 1 fixture/test assert

## Architecture boundaries

Kernel contract (camel-api) grows one variant — no dependency edges change; transports
keep depending only on camel-api (never camel-auth). Data/control plane untouched: this is
error plumbing at the transport denial boundary (ADR-0010 boundary language). No DSL, no
config surface, no behavior change beyond display text of one error.

## Alternatives considered

- **Map at transports only (keep ProcessorError)**: transports cannot distinguish auth-down
  from any other processor error without the string — that IS the bug.
- **Generic `ServiceUnavailable` variant**: no second consumer today; the auth case carries
  specific transport semantics (503/UNAVAILABLE at the authn boundary). YAGNI; a later
  generalization can rename/migrate mechanically.
- **`ProcessorErrorWithSource` chaining**: preserves a source chain but still requires
  downcasting at the transports — heavier than a direct variant for a single signal.

Single-phase change (one coherent slice, no milestone grouping needed).
