# Proposal: wasm-bound-address

## Why

The `camel-component-wasm` integration tests (4 source-world binaries,
29 callsites) still acquire ports with bind-read-drop probes: four local
copies of `async fn free_port()` bind `127.0.0.1:0`, read the port, drop
the listener, and format the guest's `bind` config with the port. Between
drop and the host-side `TcpListener::bind` in
`WasmSourceConsumer::start()` (source_consumer.rs:284) the port is free —
the port-toctou race class that rc-9xsv (ws) and rc-h0aw (camel-test)
eliminated structurally. The wasm source consumer is the last inbound
transport whose tests probe instead of staging (bd rc-wgba, epic rc-99d5;
papal e_opus R6 confirmed the split).

Key enabler (verified): the host binds on behalf of the guest — the guest
declares a bind address, the host shim binds a real `tokio` listener and
serves axum on it. The binder is host code we control, so the canon
`staged-listener-binding` capability applies directly, third extension
after camel-http and camel-component-ws.

## What Changes

- `camel-component-wasm`: new `staged_listener` module — process-global
  one-shot staged map keyed by exact `(host-string, port)`, same error
  string family as canon (`listener already staged for {h}:{p}`,
  `staged listener conflict on port {p}: staged under host {h},
  requested {r}`).
- `WasmSourceConsumer::start()` bind site: consult the staged map after
  the operator/guest bind agreement (12b) and the ADR-0061 exposure gate;
  exact hit → serve the staged listener, no second bind; same-port
  different-host staged → deterministic `EndpointCreationFailed` error;
  miss → normal `TcpListener::bind` (behaviorally compatible).
- `tests/common/mod.rs`: `stage_wasm_source_listener(host) -> u16`
  helper (bind `{host}:0`, stage, return actual port) mirroring the
  camel-test helpers; migrate the acquisition sites across
  `source_integration.rs`, `source_stream_integration.rs`,
  `source_bind_gate.rs`, `source_auth_e2e.rs` (29 calls total: 28 via
  the helper, one asserted-unbound site via a fixed reserved address);
  delete the four `free_port()` copies.
- Delta spec: `staged-listener-binding` capability gains a wasm-source
  requirement and extends the no-port-probes requirement.

Excluded: producer side (dial-only, binds nothing); no wasm server
registry (consumers are route-lifecycle, not port-addressed); no reset
API (ephemeral ports never collide across one-shot keys; each test
binary is a fresh process); no WIT/guest contract change.

## Acceptance criteria

- `cargo test -p camel-component-wasm` green (lib + all integration
  binaries), including new tests: staged-listener served end-to-end,
  duplicate staging rejected, wrong-host conflict deterministic, refused
  route does not consume the staged slot.
- `grep -rn 'fn free_port' crates/components/camel-component-wasm/`
  returns no matches; 28 of 29 callsites use the staged helper and the one
  asserted-unbound site names a fixed reserved address (`127.0.0.1:1`).
- Zero behavior change on the unstaged path: existing suite untouched
  semantics; staging empty by default.
- All AGENTS.md quality gates green in the worktree.

## Risk budget

Risk accepted: a `pub` staged surface on a component crate (papal e_opus
R1 precedent: same class as `get_or_spawn_with_listener` — operator
surface, not a core test-only event); exact-string host keys with no
normalization (canon semantics). Out of bounds: any WIT change, any
guest-world contract change, any new crate dependency.
