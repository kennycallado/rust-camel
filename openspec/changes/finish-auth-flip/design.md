# Design: finish-auth-flip

Follows ADR-0061 (unified transport auth kernel). This change deletes
residue; it introduces no new security semantics. Single phase.

## Context

ADR-0061 Rule 1: transports authenticate via `kernel_authenticate` and
install the typed carrier; the controller's strict dispatch denies
non-Public routes without it. After the flip, ws and grpc retain
pre-kernel handshake arms for the `security_plan == None` case, fed by a
`SecurityContext.authenticator` slot threaded from route compilation.

## Decisions

**D1 — kernel-only handshake (ws, grpc).** In grpc this also deletes
the four legacy policy-evaluation blocks in `consumer.rs:563-591,
612-640, 655-683, 698-719` (the per-arm scratch evaluation), not just
the authentication helpers. `WsKernelAuth::from_security_context`
and `GrpcKernelAuth::from_security_context` already return `None` without
a plan; today `None` falls back to the legacy arm. After this change:
`None` means Public pass-through (no extraction, no local evaluation),
identical to http (2.9) and mcp (2.6). A declared-security route without
a compiled plan is already impossible for DSL routes (plan compilation is
mandatory at `add_route`) and fails closed for programmatic routes at
strict dispatch. Delete: ws `dispatch_handler` legacy branch +
`LegacyPrincipal`; grpc `authenticate_request` legacy branch +
`GrpcPrincipal` + `extract_principal` + `legacy_authenticator`/
`legacy_sources` params + the `authenticator` slot in `GrpcDispatchEntry`.

**D2 — `SecurityContext.authenticator` field dies.** Its consumers are
the two legacy arms D1 deletes (grpc `consumer.rs:483-497`, ws
`lib.rs:567-598`). Delete the field (`camel-component-api` consumer.rs)
and the plumbing at camel-core's two `set_security_context` sites.
`RouteDefinition.security_authenticator` (DSL/programmatic declaration
marker feeding plan classification, `route_compiler_ext.rs:285`) is
UNCHANGED: it keeps its classification-marker role only; the provider
registry is consumed separately at plan compilation
(`route_controller.rs:489-502`). camel-cli/camel-builder wiring that
creates the declaration marker is untouched.

**D3 — test ports, not deletions.** The 5 ws credential-sources tests
exercise extraction shapes (bearer header, cookie, named header, …).
They move to the kernel path: fixture builds a plan with the same
`credential_sources` over a single-provider registry; assertions keep
grant/deny outcomes and add the carrier assertion where missing. The 3
grpc legacy-path tests in `server_auth_test.rs` are updated to expect
kernel-or-Public semantics (a contextless consumer is Public; a
plan+registry consumer authenticates via the kernel).

**D4 — late-registration real-socket test (deferred from the 2.2
review).** New integration test in camel-test: bind a real HTTP listener
(loopback arm, non-loopback arm) through `CamelTestContext`, then
`add_route` a late Public route. Asserts refusal names bind+route on
non-loopback without ack; loopback accepts and serves. Complements the
unit tests that swap URIs on a timer fixture (constructed state).

## Affected crates

- `camel-component-api` — SecurityContext field removal (contract crate,
  pre-1.0 breaking, same posture as the parent change).
- `camel-ws` — legacy arm deletion, test ports.
- `camel-component-grpc` — legacy arm deletion, test updates.
- `camel-core` — set_security_context threading simplification.
- `camel-cli`, `camel-builder` — no change expected (their wiring
  creates the unchanged declaration marker); verified, not assumed.
- `camel-test` — ws kernel-path fixtures, late-registration test.

## Risks

Programmatic embedders that passed `SecurityContext { authenticator,
.. }` struct-literal style will break at compile time (desired: the
field never authenticated anything post-flip; strict dispatch already
rejects its grants). Doc surfaces mentioning the field (CONTEXT.md
files) update with the code.
