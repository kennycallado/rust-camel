# ADR-0070: Staged Listeners for Port-Deterministic Tests

## Status

Accepted (2026-09-06). Generalizes ADR-0069 §6.5. The capability spec
`openspec/specs/staged-listener-binding/spec.md` is normative for the
contract details.

## Context

The epic rc-99d5 taxonomy names the `port-toctou` flake class. A test
binds port 0, reads the assigned port, and drops the listener. It then
hands the bare port number to a component that binds later. Between the
drop and the real bind, any process can claim the port. The camel-ws
hang (bd rc-y24l) burned a CI runner for six hours through this class.
The original inventory counted 104 probe callsites.

The fix pattern is now landed in every inbound transport. Four
applications, each reviewed and merged:

| Component | Surface | Change |
|---|---|---|
| camel-component-grpc | `GrpcConsumer::start_with_listener` (consumer.rs:434), server-side `get_or_spawn_with_listener` (server.rs:187) | pre-epic |
| camel-component-ws | `ServerRegistry::stage_listener` / `get_or_spawn_with_listener` (lib.rs:132, :295), `WsConsumer::start_with_listener` (lib.rs:1336) | bd rc-9xsv |
| camel-component-http | `ServerRegistry::stage_listener` / `get_or_spawn_with_listener` (lib.rs:886, :853) | bd rc-h0aw |
| camel-test residue (http_static 20 sites staged, ws_security 8 placeholder) | support helpers + constant placeholders | bd rc-yorz9 |
| camel-component-wasm | `staged_listener::stage_listener` (staged_listener.rs:37), consumed at the source bind site | bd rc-wgba |

The camel-test itest binaries acquire ports through two helpers,
`stage_http_listener` and `stage_ws_listener` (bd rc-h0aw). The wasm
test binaries use `stage_wasm_source_listener` (bd rc-wgba).

## Decision

1. Test port acquisition SHALL come from a staged bound listener. The
   test binds `{host}:0`, parks the listener with the component, and
   reads the actual port from it. Bind-read-drop probes are forbidden.
   A route that later binds the same port receives the held socket.
   No window exists in which the port is free.
2. Every application follows one contract (the capability spec carries
   the scenario detail):
   - Exact `(host-string, port)` key. No DNS or wildcard
     normalization. `0.0.0.0` and `127.0.0.1` are different keys.
   - One-shot consumption. The consumer removes the entry when it
     takes the listener.
   - Empty by default. The unstaged path is behaviorally compatible
     with the pre-change path. An added map lookup is the only
     internal difference.
   - Deterministic conflicts. A staged entry on the same port under a
     different host string fails the consumer before any bind with
     `staged listener conflict on port {p}: staged under host {h},
     requested {r}`. Duplicate staging fails with `listener already
     staged for {h}:{p}`. These strings are contractual. A silent
     fresh bind would risk `EADDRINUSE` flakiness, so the failure is
     explicit instead.
   - One consumption point. Registries consume inside the one-shot
     init winner (the `OnceCell` closure). The wasm source consumer
     consumes at its single bind site. Both close the two-callers race
     by construction.
3. Consumption ordering. A consumer consults the staged map only after
   config agreement validation and security gates. In wasm the
   consumption point sits after the operator/guest bind agreement and
   the ADR-0061 exposure gate. A refused route never consumes a staged
   slot. Staging never bypasses a gate.
4. Production-surface ruling (papal e_opus reviews of rc-h0aw and
   rc-wgba): listener injection on a COMPONENT registry is the
   bound-address operator surface class. ADR-0069 §6.5 blesses
   `get_or_spawn_with_listener` as that class. `stage_listener`
   belongs to the same class. The ADR-0069 §6 fence forbids test-only
   event surfaces in camel-core. No staged API exists in core, and
   none may.
5. The ADR-0069 gate question for new core APIs is "would this API
   exist without tests?" For component staged surfaces the answer is
   yes. An operator can pre-bind a socket and hand it to the
   component. External supervisors do this today through FD
   inheritance.
6. Exception for asserted-unbound addresses. A test that proves an
   address has no listener (connect-refused assertion) cannot stage.
   A staged listener is bound by definition. Such a test names a fixed
   reserved address, `127.0.0.1:1`. The canon spec carries this
   exception in the no-port-probes requirement.
6a. Exception for placeholder ports. The exception applies ONLY where
   the request is served by `tower::ServiceExt::oneshot` in process AND
   the address is never bound nor dialed. Such a test performs no port
   acquisition, so a constant placeholder replaces the probe (bd
   rc-yorz9, ws_security_test). No staging applies, because no
   acquisition happens. A test that binds or dials never qualifies.
7. No reset API. Staged keys are ephemeral ports that never repeat
   within a process. Each test binary is a fresh process. Unconsumed
   slots from refusal-path tests stay inert until process exit.
   camel-http, camel-ws, and camel-component-wasm all ship without
   reset, by the same rationale.
8. New inbound transports MUST offer the staged surface at design
   time. The surface is part of the transport's operator contract, not
   a retrofit.

## Consequences

- The `port-toctou` class is structurally eliminated from every test
  binary and itest suite. `grep -rn 'fn free_port' crates/` returns
  zero matches. The final camel-test residue (two local helpers missed
  by the rc-h0aw inventory) was removed under bd rc-yorz9 during this
  ADR's review. Two named residues remain under their own bd issues:
  the camel-http in-lib helper `setup_consumer_on_free_port`
  (`crates/components/camel-http/src/lib.rs:7387`, 8 sites, bd rc-1dgvg) and the camel-bridge Quarkus env handoff (bd rc-s7dyw,
  an external process that cannot consume a staged listener).
- Process-global staged maps are accepted test-tier state. They are
  empty in production and hold no task references.
- The CLONE-FIXTURE pattern is the sanctioned way to hold two handles
  for one socket in a test: std bind, `try_clone()`,
  `set_nonblocking(true)`, `tokio::net::TcpListener::from_std`. Tokio
  has no `try_clone`.
- ADR-0069 §6.5 anticipated "the bound address a future boot handle
  reports". The staged-listener surface is that address surface. Port
  acquisition needs no separate boot handle.
- A future transport that binds without a staged surface reintroduces
  the class. Review must cite this ADR when rejecting such a design.
