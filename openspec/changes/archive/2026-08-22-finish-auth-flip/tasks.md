# Tasks: finish-auth-flip

## Phase 1: Delete legacy arms + authenticator hand-off

### Task 1.1: Delete the ws legacy authn arm

**Files:**
- `crates/components/camel-ws/src/lib.rs` (modified — delete `LegacyPrincipal` at ~:391, the `dispatch_handler` else-branch at ~:567-598, and the legacy `principal` plumbing into `ws_handler`)
- `crates/components/camel-ws/tests/credential_sources_test.rs` (modified — port the 5 legacy tests)
- `crates/components/camel-ws/tests/kernel_auth_test.rs` (modified — fixture at ~:109-111 keeps only kernel fields)

**Steps:**
1. Delete `struct LegacyPrincipal(Principal)` and its `impl AuthPrincipal` (lib.rs ~:391).
2. Delete the `dispatch_handler` else-branch that reads `sec_ctx.authenticator` (~:567-598): the `extract_token_multi` + `authenticate_bearer` + local `policy.evaluate` path and its "until the Task 2.9 migration deletes it" comment. The kernel branch remains the only authentication path; a context without a plan is Public pass-through (no extraction).
3. Remove the legacy `principal` parameter from `ws_handler` and every caller; the connection carries only the kernel carrier (`carrier: Option<AuthenticatedPrincipal>`).
4. Port the 5 legacy tests in `tests/credential_sources_test.rs` to the kernel path (fixture builds a plan with the same `credential_sources` over a single-provider `ProviderRegistry`; mirror `ws_kernel_handshake_grants` in `tests/kernel_auth_test.rs:167` — a DIFFERENT file from the ported ones): `ws_cookie_source_authenticates` (:146), `ws_token_outside_declared_sources_rejected` (:164 — the declared-sources negative: a token in an UNdeclared source is ignored), `ws_default_header_only_unchanged` (:182), `ws_no_credential_rejected_before_eval` (:213), `ws_query_param_sentinel_redacted_in_upgrade_logs` (:243). Assertions keep grant/deny outcomes; grant cases add `read_carrier` + `provider_id` assertions where missing.
5. The redaction test's positive control CANNOT survive on its plan-less fixture (post-change plan-less = Public pass-through, never extracts, `debug_log_query_credential` never fires). Since Task 1.8 also rejects `QueryParam` in ws plans at compile time, the port needs an explicit test-only construction: build the plan struct directly in the test (bypassing plan compilation) with a QueryParam source, drive the kernel branch (it calls the same redaction helper, lib.rs ~:517), keep the sentinel assertions identical.

**Tests:**
- `ws_legacy_arm_deleted_source_scan`: setup the crate source; action `grep` for `LegacyPrincipal` and `sec_ctx.authenticator` in `crates/components/camel-ws/src/`; assert zero production hits (test-file comment scans allowed); command `cargo test -p camel-component-ws`; expected: pass.
- `ws_kernel_bearer_header_authenticates` (ported from `ws_default_header_only_unchanged`): Authenticated-plan ws route + single StubAuth provider registry; connect with `Authorization: Bearer <token>`, send one message; assert 101 + body ran + `read_carrier` provider id; command `cargo test -p camel-component-ws kernel_bearer`; expected: pass.
- `ws_kernel_cookie_authenticates` (ported from `ws_cookie_source_authenticates`): `Cookie { name: "session" }` source + matching cookie; assert grant + carrier; command `cargo test -p camel-component-ws kernel_cookie`; expected: pass.
- `ws_kernel_token_outside_declared_sources_rejected` (ported from `ws_token_outside_declared_sources_rejected`): token present ONLY in an undeclared source (e.g. cookie while the plan declares AuthorizationHeader); assert handshake refused 401 — declared-sources negative preserved; command `cargo test -p camel-component-ws kernel_token_outside`; expected: pass.
- `ws_kernel_missing_credential_denies` (ported from `ws_no_credential_rejected_before_eval`): no credential at upgrade; assert handshake refused 401; command `cargo test -p camel-component-ws kernel_missing`; expected: pass.
- `ws_query_param_sentinel_redacted_in_upgrade_logs` (ported, SAME name): test-only QueryParam plan (direct struct construction per step 5); assert the "WS upgrade with query token (redacted)" debug line is captured with the sentinel redacted; command `cargo test -p camel-component-ws redact`; expected: pass.
- `ws_public_route_no_extraction_counted_provider`: ws route without a plan + provider whose `authenticate` increments a counter; connect with a VALID credential and send a message; assert 101 + body ran + counter == 0; command `cargo test -p camel-component-ws public_route_no_extraction`; expected: pass.

**Acceptance:**
- `cargo test -p camel-component-ws` exits 0 (all suites).
- `! grep -n 'LegacyPrincipal\|sec_ctx.authenticator' crates/components/camel-ws/src/lib.rs` exits 0 (negated grep — zero matches is the PASS condition).
- `cargo clippy -p camel-component-ws --all-targets -- -D warnings` exits 0.

- [x] 1.1

### Task 1.2: Delete the grpc legacy authn arm

**Files:**
- `crates/components/camel-component-grpc/src/server.rs` (modified — delete the legacy branch of `authenticate_request` at ~:519-548 and `extract_principal`; shrink the return tuple — the kernel branch currently returns `Ok((Some(view), Some(principal)))` at ~:540-547; update the four destructure sites ~:693/:746/:794/:875)
- `crates/components/camel-component-grpc/src/consumer.rs` (modified — delete `GrpcPrincipal` at ~:37, the four policy-evaluation scratch blocks at ~:563-591, 612-640, 655-683, 698-719 — these run on KERNEL-authenticated traffic today, so their deletion moves per-request policy enforcement wholly to the pipeline layer — and the `ctx.authenticator` read at ~:483-497; remove the `authenticator` slot from the `GrpcDispatchEntry` construction ~:490-497; the alias lives in server.rs:34)
- `crates/components/camel-component-grpc/tests/server_auth_test.rs` (modified — update legacy-path tests)

**Steps:**
1. In `server.rs`: reduce `authenticate_request` to the kernel path — a `KernelAuthn` plan + registry authenticates; no plan → Public pass-through returning no principal and no error. Delete `GrpcPrincipal`, `extract_principal`, and the `legacy_authenticator`/`legacy_sources` parameters from the signature and callers.
2. In `server.rs` + `consumer.rs`: the four per-arm policy-evaluation scratch blocks in `consumer.rs` currently evaluate policy on KERNEL-authenticated traffic as well (the kernel branch returns a principal view). Deleting them removes per-request policy evaluation at the transport — enforcement semantics move wholly to the pipeline layer + the strict dispatch check. Shrink `authenticate_request`'s return to the kernel outcome (no principal view), update the four destructure sites, delete the `ctx.authenticator` read in `consumer.rs` context wiring (~:483-497), and remove the `authenticator` slot from `GrpcDispatchEntry` (alias server.rs:34, construction consumer.rs ~:490-497).
3. Update `server_auth_test.rs`: tests that exercised the legacy path now assert kernel-or-Public semantics — a contextless consumer is Public pass-through (`grpc_public_no_extraction_counted_provider`: request with a valid credential, provider call counter == 0); a plan+registry consumer authenticates via the kernel (existing `grpc_denies_without_credentials_under_kernel`, `grpc_named_header_credential_authenticates`, `grpc_carrier_present_on_second_request_fresh_exchange` stay green unchanged).

**Tests:**
- `grpc_legacy_arm_deleted_source_scan`: `grep` for `GrpcPrincipal\|extract_principal\|legacy_authenticator` in `crates/components/camel-component-grpc/src/`; assert zero hits; command `cargo test -p camel-component-grpc`; expected: pass.
- `grpc_public_no_extraction_counted_provider`: gRPC route without a plan + counting provider; call with a VALID credential; assert success + counter == 0; command `cargo test -p camel-component-grpc public_no_extraction`; expected: pass.
- `grpc_kernel_auth_policy_denied_regression`: kernel-authenticated request (valid token) on a route whose PIPELINE policy denies; assert `Status::permission_denied` — proves denial semantics survive the scratch-eval deletion (enforcement now pipeline-only); command `cargo test -p camel-component-grpc --test server_auth_test`; expected: pass.
- Existing kernel tests unchanged and green: `grpc_denies_without_credentials_under_kernel`, `grpc_named_header_credential_authenticates`, `grpc_carrier_present_on_second_request_fresh_exchange` in `--test server_auth_test`, plus `grpc_plan_present_at_interceptor_construction` (unit test in src/server.rs — run via plain `cargo test -p camel-component-grpc`); expected: pass.

**Acceptance:**
- `cargo test -p camel-component-grpc` exits 0.
- `! grep -rn 'GrpcPrincipal\|extract_principal\|legacy_authenticator' crates/components/camel-component-grpc/src/` exits 0 (negated grep — zero matches is the PASS condition).
- `cargo clippy -p camel-component-grpc --all-targets -- -D warnings` exits 0.

- [x] 1.2

### Task 1.3: Delete `SecurityContext.authenticator`

**Files:**
- `crates/components/camel-component-api/src/consumer.rs` (modified — delete the `authenticator` field at ~:320 from `SecurityContext`; fix the unit tests at ~:799-827 that construct it)
- `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs` (modified — at the two `set_security_context` sites (~:296-315 start, ~:703-720 resume) drop the authenticator argument from `SecurityContext::from_arc(..)`; the struct's `policy` and `credential_sources` fields REMAIN)
- `crates/camel-component-ws/tests/credential_sources_test.rs` + `tests/kernel_auth_test.rs` (modified in 1.1; 1.3 verifies no residual field use — the kernel_auth_test.rs:109-111 fixture keeps kernel fields only)
- `crates/camel-test/tests/ws_security_test.rs` (modified — the ~:95 fixture exercises the legacy ws arm semantically; port to the kernel path the same way as 1.1's ports)
- `crates/camel-component-grpc/src/consumer.rs` (modified — remove the `ctx.authenticator` read if 1.2 left it)

**Steps:**
1. Delete the `pub authenticator: Arc<dyn TokenAuthenticator>` field from `SecurityContext` (camel-component-api consumer.rs ~:320) and fix every constructor/struct-literal site the compiler flags.
2. At camel-core's two `set_security_context` sites (route_controller_trait.rs ~:296-315 start, ~:703-720 resume) drop the authenticator argument from the `SecurityContext::from_arc(..)` call. `RouteDefinition::with_security_authenticator` (the declaration marker) STAYS untouched.
3. Confirm `RouteDefinition.security_authenticator` and its classification role (`route_compiler_ext.rs:285`) compile unchanged — do NOT touch the declaration marker or `with_security_authenticator` on RouteDefinition/RouteBuilder.
4. Grep the workspace for remaining `SecurityContext {` literal constructions and `ctx.authenticator`/`sec_ctx.authenticator` reads; fix or delete each.

**Tests:**
- `security_context_authenticator_deleted_source_scan`: `grep -rn 'authenticator' crates/components/camel-component-api/src/consumer.rs`; assert the field is gone (registry/doc mentions allowed); command `cargo test -p camel-component-api`; expected: pass.
- Existing suites prove no behavior change: `cargo test -p camel-core --lib` (695+) and `cargo test -p camel-test --features integration-tests --test kernel_fail_closed_test --test audience_substitution_test` green; expected: pass.

**Acceptance:**
- `cargo test -p camel-component-api -p camel-core --lib` exits 0.
- `cargo test -p camel-component-ws -p camel-component-grpc` exits 0 (compiles the ported external fixtures).
- `cargo test -p camel-builder -p camel-cli` exits 0 (proposal AC: both crates pass tests, not just clippy).
- `cargo test -p camel-test` (default features) exits 0 — proves ws_security_test.rs compiles ported.
- `! grep -n 'pub authenticator' crates/components/camel-component-api/src/consumer.rs` exits 0 (negated grep).
- `cargo clippy -p camel-component-api -p camel-core -p camel-component-ws -p camel-component-grpc -p camel-builder -p camel-cli --all-targets -- -D warnings` exits 0 (builder/cli compile against the shrunken SecurityContext surface — closes the proposal AC).

- [x] 1.3

### Task 1.4: Late-registration real-socket integration test

**Files:**
- `crates/camel-test/tests/late_registration_gate_test.rs` (new)

**Steps:**
1. New integration test (feature `integration-tests`, same cfg gate as `kernel_fail_closed_test.rs`). Loopback arm: start an `http://127.0.0.1:<free-port>` route through `CamelTestContext` (RouteBuilder + provider_registry wiring per `kernel_fail_closed_test.rs` fixtures), await listener up, then `add_route` a late Public route onto the same bind; assert registration Ok and the route serves (HTTP 200 via a real request).
2. Non-loopback arm: start an `Authenticated` route on `http://0.0.0.0:<free-port>` (no exposure acknowledgement; authenticated routes need none), prove the listener is genuinely ready by serving it a valid-credential request through `http://127.0.0.1:<port>` (assert 200), then `add_route` a late `Public` route onto the same bind; assert `Err` names both the bind address and the route id, and a subsequent request to the late route's path returns a 404-equivalent (path not registered) — a connection refusal CANNOT be accepted as proof, it would not distinguish a refused gate from a dead listener.
3. Both arms use real sockets end-to-end (no `from_uri` swap on a timer fixture).

**Tests:**
- `late_public_route_real_nonloopback_refused_e2e`: as step 2; command `cargo test -p camel-test --features integration-tests --test late_registration_gate_test`; expected: pass.
- `late_public_route_real_loopback_served_e2e`: as step 1; command same; expected: pass.

**Acceptance:**
- `cargo test -p camel-test --features integration-tests --test late_registration_gate_test` passes (2 tests).
- `cargo clippy -p camel-test --features integration-tests --test late_registration_gate_test -- -D warnings` exits 0 (feature-enabled, target-scoped — the plain `--all-targets` form compiles the test empty without the feature).

- [x] 1.4

### Task 1.5: Docs alignment

**Files:**
- `crates/components/camel-ws/CONTEXT.md` (modified — remove legacy-arm references if any)
- `crates/components/camel-component-grpc/CONTEXT.md` (modified — remove legacy/authenticator-slot references if any)
- `crates/components/camel-component-api/CONTEXT.md` (modified — remove `SecurityContext.authenticator` references if any)
- `docs/src/components/ws-soap.md` (modified — only if it mentions the legacy bearer path; the 2026-08-22 kernel-flow rewrite is already correct)
- `docs/src/components/grpc.md` (modified — only if it mentions the legacy path; already correct from the same rewrite)

**Steps:**
1. Grep the five files for `authenticator`, `legacy`, `LegacyPrincipal`, `GrpcPrincipal`; update stale mentions to the kernel-only model (Public pass-through without a plan; kernel path with one).
2. If a file needs no change, verify and report "no drift" — do not edit for the sake of editing.

**Tests:**
- `docs_source_scan`: `grep -rn 'LegacyPrincipal\|GrpcPrincipal\|legacy_authenticator' crates/components/camel-ws/ crates/components/camel-component-grpc/ docs/src/` returns zero hits; command `cargo xtask lint-context-citations`; expected: pass (0 violations).

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- The grep above returns zero hits.

- [x] 1.5
