# Proposal: wasm-source-auth-kernel

## Why

The WASM source-consumer surface is the 5th inbound network transport and the
only one outside the ADR-0061 unified transport auth kernel (bd rc-f79u).
Verified gaps in `crates/components/camel-component-wasm`:

1. The guest-declared `listener_spec.bind` is parsed and bound directly
   (`source_consumer.rs:179-199`) with no loopback check, no exposure
   acknowledgment, bypassing the per-bind exposure gate (`camel-auth`
   `bind_gate.rs`) that every other transport runs.
2. `run_http_listener` (`source_host.rs:500`) accepts every request and feeds
   it to the guest with no credential extraction, no `kernel_authenticate`,
   no carrier. The consumer inherits the no-op `set_security_context` default
   and drops the `SecurityContext` the route controller already builds.
3. A wasm-source route can therefore never meaningfully declare `Authenticated`
   (a carrier can never exist), and `examples/wasm-source-webhook` serves
   unauthenticated traffic on `0.0.0.0:8080` today.

## What Changes

Converge the wasm source onto the kernel using the gRPC "boundary-auth" shape
(Gen B, per the e_opus consultation in
`docs/consultations/rc-f79u-wasm-source-auth.md`):

- Host-edge kernel handshake inside `run_http_listener`: extract credentials
  from request headers/cookie/query, `kernel_authenticate`, deny 401
  (authentication only; authorization stays pipeline-owned) BEFORE the request
  is forwarded to the guest.
- Carrier threading: `HttpMeta` gains a private `principal` field; at the
  host edge `accept_http` moves the minted `AuthenticatedPrincipal` into a
  guarded `SourceHostState` slot (one-outstanding-request invariant), and
  `submit_exchange` takes it and calls `install_carrier` on the native
  Exchange before it enters the pipeline.
- Bind governance: operator URI `?bind=` is authoritative. When the
  guest-declared `listener_spec.bind` differs from the operator-declared
  bind, the consumer fails during startup — after the guest reveals its
  listener spec and before `TcpListener::bind` — naming both. With no
  operator bind, the guest bind is used but gated. Before
  `TcpListener::bind`, `start()` runs `camel_auth`
  `enforce_bind_exposure_gate` on the resolved bind with the compiled plan
  snapshot and the operator's `BindExposureAcks` (wired from the CLI as mcp
  does).
- Docs: amend ADR-0061 (5th transport), update crate `CONTEXT.md` inbound
  posture, `CONTEXT-MAP.md`, and fix the webhook example.

Excluded: WIT contract changes (the guest never sees auth or denials), any
camel-http dependency (option A rejected), and authorization policy work
beyond what the existing kernel provides.

## Acceptance criteria

- An `Authenticated` wasm-source route rejects a missing/invalid credential
  with 401 and the guest never sees the request.
- A valid credential yields a pipeline Exchange carrying the typed carrier
  (`read_carrier` succeeds); a provider-A token does not satisfy a provider-B
  route.
- A non-loopback `Public` wasm-source bind without `allow_public_exposure`
  fails at `start()`; loopback passes; an operator/guest bind conflict fails
  at consumer startup before the TCP listener is bound.
- No new crate dependencies; ADR-0031 guest-owned `run(listener)` loop intact.

## Risk budget

Acceptable: contained changes to `source_consumer.rs`/`source_host.rs` and
docs. Out of bounds: WIT breakage, component-to-component dependencies,
weakening the fail-closed posture of the existing kernel, and any change to
the 202-immediate-ack semantics for accepted requests.
