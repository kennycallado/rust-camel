# Proposal: auth-provider-unavailable

## Why

`AuthError::ProviderUnavailable` (typed, in camel-auth) is destroyed when it crosses the
kernel boundary: `From<AuthError> for CamelError` (crates/services/camel-auth/src/types.rs:42-44)
flattens it into `CamelError::ProcessorError("auth provider unavailable: {detail}")`. Both
transports then string-match that magic substring to pick the right denial status:

- camel-ws `ws_upgrade_auth_error` (crates/components/camel-ws/src/lib.rs:418) — 503 vs 500
- camel-component-grpc `auth_error_to_status` (crates/components/camel-component-grpc/src/server.rs:468) — UNAVAILABLE vs INTERNAL

Any wording change silently degrades 503 → 500 and UNAVAILABLE → INTERNAL. Clients and
load balancers lose the "retry later, provider down" signal. Found during finish-auth-flip
review (bd: rc-bfus, discovered from rc-oj1j).

## What Changes

- Add typed `CamelError::AuthProviderUnavailable(String)` in camel-api, following the
  established promotion precedent (`ConfigValidation`, `EndpointUri`): stringly-typed →
  matchable variant so operators discriminate programmatically.
- `From<AuthError> for CamelError` maps `ProviderUnavailable` to the new variant.
- camel-ws and camel-component-grpc match the variant; delete both magic-string matches.
- Update the sites that construct or assert the magic string (grpc MockAuthenticator
  server.rs:1601; tests in camel-auth, camel-component-keycloak, camel-test).
- Update `CamelError::classify()` and any same-crate exhaustive matches (compiler-guided).

**Excluded:** `LlmError::ProviderUnavailable` mapping (different enum, different semantics);
camel-http transport (has no such string match); any general `ServiceUnavailable` variant
for non-auth dependencies (no second consumer today — YAGNI).

## Acceptance criteria

- `rg "auth provider unavailable" crates/` returns no production-code string matchers
  (transport status selection is variant-based).
- A wording change in the error detail cannot change the HTTP status (covered by test).
- ws maps `AuthProviderUnavailable` → 503; grpc maps it → `tonic::Status::unavailable`,
  each asserted by a test that constructs the variant directly.
- Error-handler compatibility preserved: `variant_name()` reports `"ProcessorError"` and
  `classify()` reports `"processor"` for the new variant (doTry catch handlers unchanged).
- camel-api, camel-auth, camel-ws, camel-component-grpc, camel-component-keycloak,
  camel-test all build and their touched test suites pass.

## Risk budget

- Low risk: additive `#[non_exhaustive]` variant; external matches unaffected.
- Acceptable: display-text change on the wire detail (was "Processor error: auth provider
  unavailable: X" → now "Auth provider unavailable: X"). Status codes and semantics unchanged.
- Out of bounds: any behavior change in error handlers' treatment of the error beyond
  parity with today's `ProcessorError` fallback handling; new public API beyond the variant.
- Known residual (accepted, release-note line): userland `on_exception` closures that
  match `CamelError::ProcessorError(_)` directly (not via `variant_name()`) stop catching
  auth-down errors — inherent to variant promotion; `variant_name()`-based doTry catches
  are unaffected.

Affected crates: camel-api, camel-auth, camel-ws (components), camel-component-grpc,
camel-component-keycloak (tests), camel-test (tests). Bd: rc-bfus.
