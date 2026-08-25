# Tasks: auth-provider-unavailable

## camel-api (kernel contract)

### Task 1: Add typed `CamelError::AuthProviderUnavailable` variant

**Files:**
- `crates/camel-api/src/error.rs` (modified)

**Steps:**
1. Add variant to `CamelError` (after `Unauthorized(String)`, near the auth variants) with
   doc comment: "Auth provider (JWKS/introspection/token endpoint) is unreachable or
   failing. Promotes the auth-provider-down signal from a stringly-typed ProcessorError to
   a matchable variant; WebSocket and gRPC transports map it to 503 / UNAVAILABLE." and
   display attribute `#[error("Auth provider unavailable: {0}")]`, payload `String`.
2. In `CamelError::classify()`: extend the processor arm to
   `Self::ProcessorError(_) | Self::ProcessorErrorWithSource(_, _) | Self::AuthProviderUnavailable(_) => "processor"`.
3. In `CamelError::variant_name()`: add arm
   `Self::AuthProviderUnavailable(_) => "ProcessorError"` — the aliasing pattern already
   used by `ProcessorErrorWithSource` (doc comment on `variant_name` cites spec §5.4).
   Update that doc comment to mention `AuthProviderUnavailable` also aliases to
   `"ProcessorError"` so existing `doTry` catch handlers keep matching.
4. Compiler-safety note (exact): `variant_name()` is exhaustive within camel-api, so the
   compiler forces its new arm. `classify()` has an `_ => "unknown"` catch-all — its new
   arm is NOT compiler-forced and is pinned only by the
   `auth_provider_unavailable_classifies_as_processor` test below. Do not skip step 2.

**Tests:** (executable spec — name, arrange, act, assert)
- `auth_provider_unavailable_classifies_as_processor`: arrange `CamelError::AuthProviderUnavailable("jwks down".into())` → act `err.classify()` → assert returns `"processor"`.
- `auth_provider_unavailable_variant_name_aliases_processor_error`: arrange `CamelError::AuthProviderUnavailable("jwks down".into())` → act `err.variant_name()` → assert returns `"ProcessorError"`.
- `auth_provider_unavailable_display_carries_detail`: arrange `CamelError::AuthProviderUnavailable("conn refused".into())` → act `err.to_string()` → assert contains `"conn refused"` and starts with `"Auth provider unavailable"`.
- Command: `cargo test -p camel-api --lib`. Place the first two tests inside the existing
  `mod variant_name_tests` (error.rs:435) and ALSO extend the
  `variant_name_covers_all_variants` fixture (error.rs:444) with the case
  `(CamelError::AuthProviderUnavailable("x".into()), "ProcessorError")` so the coverage
  table stays exhaustive; the display test goes in the module matching the file's existing
  layout for error-display tests.
- expected: fails before the variant exists (compile error), passes after.

**Acceptance:**
- `cargo test -p camel-api --lib` exits 0.
- `cargo clippy -p camel-api -- -D warnings` exits 0.

- [x] 1

## camel-auth (service)

### Task 2: Map `AuthError::ProviderUnavailable` to the typed variant; update unit tests

**Files:**
- `crates/services/camel-auth/src/types.rs` (modified)
- `crates/services/camel-auth/src/token_authenticator.rs` (modified)
- `crates/services/camel-auth/src/introspection_auth.rs` (modified)

**Steps:**
1. types.rs `From<AuthError> for CamelError`: replace the
   `AuthError::ProviderUnavailable(s) => CamelError::ProcessorError(format!("auth provider unavailable: {s}"))`
   arm (lines 42-44) with `AuthError::ProviderUnavailable(s) => CamelError::AuthProviderUnavailable(s)`.
2. types.rs test `auth_error_maps_provider_unavailable` (line 84-88): assert
   `matches!(camel_err, CamelError::AuthProviderUnavailable(s) if s.contains("jwks down"))`.
3. token_authenticator.rs test `test_authenticate_bearer_provider_unavailable`
   (lines 141-158): change match arm to
   `CamelError::AuthProviderUnavailable(msg) => assert!(msg.contains("connection refused"))`
   and update the panic message to "expected AuthProviderUnavailable".
4. introspection_auth.rs test `introspection_provider_error_propagates`
   (lines 209-231): change match arm to
   `CamelError::AuthProviderUnavailable(msg) => assert!(msg.contains("connection refused"))`
   and the panic arm to "expected AuthProviderUnavailable".

**Tests:** (executable spec)
- `auth_error_maps_provider_unavailable` (updated, types.rs): `AuthError::ProviderUnavailable("jwks down")` → `.into::<CamelError>()` → assert variant `AuthProviderUnavailable` with payload containing "jwks down".
- `test_authenticate_bearer_provider_unavailable` (updated, token_authenticator.rs): UnavailableValidator returning `AuthError::ProviderUnavailable("connection refused")` → `authenticate_bearer("token")` → assert `CamelError::AuthProviderUnavailable` containing "connection refused".
- `introspection_provider_error_propagates` (updated, introspection_auth.rs): FailingIntrospector returning `AuthError::ProviderUnavailable("connection refused")` → `authenticate_bearer("tok")` → assert `CamelError::AuthProviderUnavailable` containing "connection refused".
- Command: `cargo test -p camel-auth --lib`
- expected: compile-fails before Task 1 (variant absent), passes after both tasks.

**Acceptance:**
- `rg -n 'ProcessorError.*auth provider unavailable' crates/services/camel-auth/src/` returns no hits.
- `cargo test -p camel-auth --lib` exits 0.
- `cargo clippy -p camel-auth -- -D warnings` exits 0.

- [x] 2

## camel-ws (transport)

### Task 3: Match the variant in `ws_upgrade_auth_error`; add regression tests

**Files:**
- `crates/components/camel-ws/src/lib.rs` (modified)

**Steps:**
1. In `ws_upgrade_auth_error` (line 415-424): replace the guard arm
   `CamelError::ProcessorError(msg) if msg.contains("auth provider unavailable") => (StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable")`
   with `CamelError::AuthProviderUnavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable")`.
   The `Unauthenticated` arm (401) and `_` fallback (500) stay unchanged.
2. In the `mod tests` (line 1704+), add unit tests calling `ws_upgrade_auth_error` directly
   (it is crate-private; same-module tests can call it) — see Tests.

**Tests:** (executable spec)
- `ws_upgrade_error_provider_unavailable_is_503`: arrange `CamelError::AuthProviderUnavailable("totally arbitrary detail with no marker".into())` → act `ws_upgrade_auth_error(&err).into_response()` → assert `resp.status() == StatusCode::SERVICE_UNAVAILABLE`. The detail deliberately lacks any fixed marker substring (wording-independence).
- `ws_upgrade_error_generic_processor_error_is_500`: arrange `CamelError::ProcessorError("auth provider unavailable".into())` (a ProcessorError whose message HAPPENS to contain the old marker) → act `ws_upgrade_auth_error(&err)` → assert status `INTERNAL_SERVER_ERROR` — proves selection is variant-based, not text-based.
- `ws_upgrade_error_unauthenticated_is_401`: arrange `CamelError::Unauthenticated("bad".into())` → assert status `UNAUTHORIZED` (behavior parity guard).
- Command: `cargo test -p camel-ws --lib ws_upgrade_error`
- expected: the 503 test fails (or does not compile) before the swap; after the swap the
  500-with-old-marker test fails if anyone reintroduces string matching.

**Acceptance:**
- `rg -n 'contains\("auth provider unavailable"\)' crates/components/camel-ws/` returns no hits.
- `cargo test -p camel-ws --lib` exits 0.
- `cargo clippy -p camel-ws -- -D warnings` exits 0.

- [x] 3

## camel-component-grpc (transport)

### Task 4: Match the variant in `auth_error_to_status`; update mock; add regression tests

**Files:**
- `crates/components/camel-component-grpc/src/server.rs` (modified)

**Steps:**
1. In `auth_error_to_status` (line 464-480): replace the guard arm
   `camel_api::CamelError::ProcessorError(ref msg) if msg.contains("auth provider unavailable") => tonic::Status::unavailable(msg.clone())`
   with `camel_api::CamelError::AuthProviderUnavailable(msg) => tonic::Status::unavailable(msg)`.
   The `Unauthenticated` arm and the `other` fallback (tracing::error + internal) stay
   unchanged.
2. MockAuthenticator (test module, line ~1601): the `should_fail_unavailable` branch now
   returns `Err(camel_api::CamelError::AuthProviderUnavailable("auth provider down".into()))`
   instead of constructing the magic-string ProcessorError.
3. In the test module (line 811+), add unit tests for `auth_error_to_status` — see Tests.

**Tests:** (executable spec)
- `auth_error_to_status_provider_unavailable_is_unavailable`: arrange `CamelError::AuthProviderUnavailable("arbitrary wording no marker".into())` → act `auth_error_to_status(err)` → assert `status.code() == tonic::Code::Unavailable` and message contains "arbitrary wording no marker".
- `auth_error_to_status_generic_processor_error_is_internal`: arrange `CamelError::ProcessorError("auth provider unavailable".into())` → act → assert `status.code() == tonic::Code::Internal` — proves variant-based selection even when the message carries the old marker.
- `auth_error_to_status_unauthenticated_is_unauthenticated`: arrange `CamelError::Unauthenticated("bad".into())` → assert code `Unauthenticated` (parity guard).
- Command: `cargo test -p camel-component-grpc --lib auth_error_to_status`
- expected: unavailable test fails before the swap; the internal-with-old-marker test
  fails if string matching is reintroduced.

**Acceptance:**
- `rg -n 'contains\("auth provider unavailable"\)' crates/components/camel-component-grpc/src/` returns no hits.
- `cargo test -p camel-component-grpc --lib` exits 0.
- `cargo clippy -p camel-component-grpc -- -D warnings` exits 0.

- [x] 4

## Downstream test fixtures (camel-test, camel-component-keycloak)

### Task 5: Update downstream assertions and ws mock fixture; keep 500 regression coverage

**Files:**
- `crates/camel-test/tests/ws_security_test.rs` (modified)
- `crates/components/camel-component-keycloak/tests/introspection.rs` (modified)

**Steps:**
1. ws_security_test.rs `MockOutcome` (lines 26-33): add a new arm
   `ProviderUnavailable` documented as "Fails with
   `CamelError::AuthProviderUnavailable`; the variant drives the 503 mapping in the
   transport's upgrade-error conversion." Extend the `authenticate_bearer` match with
   `MockOutcome::ProviderUnavailable => Err(CamelError::AuthProviderUnavailable("fixture: idp unreachable".into()))`.
   Keep the existing `Error(&'static str)` arm (generic ProcessorError) but rewrite its
   doc comment to "generic `ProcessorError` — always the 500 path; status selection is
   variant-based" (the current doc's "message drives the 500 vs 503 mapping" becomes
   false after this change).
2. ws_security_test.rs `test_ws_503_provider_unavailable` (lines 198-213): switch the
   fixture to `MockOutcome::ProviderUnavailable`. Assert stays
   `StatusCode::SERVICE_UNAVAILABLE`.
3. ws_security_test.rs: add `test_ws_500_generic_processor_error_with_marker` mirroring
   the 503 test but using `MockOutcome::Error("auth provider unavailable: fixture")`
   (message carries the old marker on purpose) → assert `StatusCode::INTERNAL_SERVER_ERROR`.
   The existing `test_ws_500_provider_error` (line ~258, generic message "policy error",
   no marker) stays unchanged — the new test's distinguishing purpose is that the marker
   substring ALONE does not produce 503. Reuses the existing helpers (`make_app_state`,
   `kernel_security_context`, `ws_upgrade_request`, `dispatch_handler`) exactly as the
   503 test does.
4. keycloak tests/introspection.rs: in
   `introspection_provider_401_maps_to_provider_unavailable` (lines 71-86) and
   `introspection_provider_500_maps_to_provider_unavailable` (lines 88-104), change the
   match arm from `CamelError::ProcessorError(msg) => assert!(msg.contains("auth provider unavailable"))`
   to `CamelError::AuthProviderUnavailable(msg) => assert!(!msg.is_empty())` and the panic
   arm text to "expected AuthProviderUnavailable".

**Tests:** (executable spec)
- `test_ws_503_provider_unavailable` (updated): MockOutcome::ProviderUnavailable → upgrade request → 503.
- `test_ws_500_generic_processor_error_with_marker` (new): MockOutcome::Error("auth provider unavailable: fixture") → upgrade request → 500 despite marker substring.
- `introspection_provider_401_maps_to_provider_unavailable` / `introspection_provider_500_maps_to_provider_unavailable` (updated): wiremock 401/500 → `authenticate_bearer` → variant is `AuthProviderUnavailable` with non-empty payload.
- Command: `cargo test -p camel-test --test ws_security_test` and
  `cargo test -p camel-component-keycloak --test introspection`
- expected: compile-fails before Tasks 1-4 (variant/mocking mismatch), passes after.

**Acceptance:**
- `cargo test -p camel-test --test ws_security_test` exits 0.
- `cargo test -p camel-component-keycloak --test introspection` exits 0.
- `cargo clippy -p camel-test -p camel-component-keycloak -- -D warnings` exits 0.
- Repo-wide sweep: `rg -n 'contains\("auth provider unavailable"\)' crates/` returns no hits (no string matcher survives anywhere).

- [x] 5
