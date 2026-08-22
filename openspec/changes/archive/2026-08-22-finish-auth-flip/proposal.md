# Proposal: finish-auth-flip

## Why

The `unify-transport-auth` flip (squash `429c2004`, ADR-0061) made the
kernel the only path that mints authorization-bearing principals: strict
dispatch denies non-Public routes whose Exchanges lack the typed carrier,
and the pipeline layer is carrier-only. Three residues of the pre-kernel
wiring survived the merge as tracked follow-ups (bd rc-oj1j, rc-f2ms):

1. **ws legacy arm** — `dispatch_handler`'s else-branch (~90 lines:
   `extract_token_multi` + `authenticate_bearer` + local
   `policy.evaluate`) and `LegacyPrincipal` (`camel-ws/src/lib.rs:391`).
   Its own comment says "until the Task 2.9 migration deletes it".
2. **grpc legacy arm** — `authenticate_request`'s legacy branch,
   `GrpcPrincipal { provider_id: "legacy" }`, `extract_principal`, and
   the `legacy_authenticator`/`legacy_sources` parameters
   (`camel-component-grpc/src/server.rs:519-548`).
3. **`SecurityContext.authenticator` threading** — a pub field on
   `camel-component-api`'s `SecurityContext`, threaded camel-core to
   the transports, whose remaining consumers are the two legacy arms
   above (grpc `consumer.rs:483-497`, ws `lib.rs:567-598`; bd rc-f2ms).

e_opus's holistic blessing verified all three are fail-closed dead
weight: the legacy arms are reachable only when no compiled plan exists
(so no enforcement was requested), they never install a carrier, and
`"legacy"` structurally cannot satisfy any real `provider_ref` in
`enforce_dispatch`. Deleting them removes a divergence trap and ~150
lines of security-dead code adjacent to live kernel paths.

## What Changes

- Delete the ws and grpc legacy authn arms; kernel-only handshake
  (`WsKernelAuth`/`GrpcKernelAuth` or the documented Public path).
- Delete `SecurityContext.authenticator` (field + camel-core's
  set_security_context hand-off) once both legacy arms are gone.
  `RouteDefinition.security_authenticator` STAYS unchanged as the
  route-level security-declaration marker that feeds plan
  classification (`route_compiler_ext.rs:285`); the DSL/CLI/builder
  wiring that creates that marker is untouched.
- Port the 5 ws credential-sources legacy tests to the kernel path
  (same scenarios, plan-driven).
- Add the late-registration real-socket integration test deferred from
  the `unify-transport-auth` 2.2 review (loopback + non-loopback arms
  through a genuinely bound HTTP listener).

## Acceptance Criteria

- A workspace grep for `LegacyPrincipal`, `GrpcPrincipal`,
  `legacy_authenticator`, and `SecurityContext.authenticator` returns
  zero production hits.
- All existing kernel-path tests stay green without weakening; ported
  ws tests assert the same grant/deny outcomes via the kernel carrier.
- camel-ws, camel-component-grpc, camel-component-api, camel-core,
  camel-builder, camel-cli pass tests + clippy `-D warnings`.
- The late-registration integration test proves the gate refuses an
  unacked non-loopback Public route on a real socket and accepts a
  loopback one.

## Risk Budget

Low. The deleted arms are unreachable for DSL routes (plan compilation
is mandatory), their grants die at strict dispatch anyway, and the
follow-up decisions were pre-ratified by three gates (bd notes, r_glm
findings, e_opus blessing point 5). The one design question — how
programmatic `RouteDefinition` embedders declare security without the
SecurityContext authenticator slot — is answered by keeping
`security_authenticator` as a declaration marker consumed at plan
compilation, exactly as http/mcp already work.
