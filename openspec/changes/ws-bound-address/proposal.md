# Proposal: ws-bound-address

## Why

`camel-component-ws` tests obtain server ports via `free_port()` (bind `127.0.0.1:0`, read port, drop listener — 22 callsites in `src/lib.rs`). Between the drop and the consumer's real bind inside `WsConsumer::start`, another process can claim the port: the **port-toctou** flake class (epic rc-99d5 taxonomy). The existing 5s `connect_until_ready` retry (rc-y24l) is a mitigation of this race, not a cure. ADR-0069 §5 explicitly deferred this work: "OS-selected consumer ports require a separate operator-facing bound-address API, filed on its own merits" (bd rc-9xsv is that filing).

The structural blocker: the test must build the WS URI *before* the listener exists, because binding happens inside `start`. The repo already solved this shape in `camel-component-grpc`: `GrpcServerRegistry::get_or_spawn_with_listener(listener, …)` keyed by the listener's *actual* port, plus `GrpcConsumer::start_with_listener(ctx, listener)`.

## What Changes

Mirror the grpc precedent in `camel-component-ws`:

1. `ServerRegistry::get_or_spawn_with_listener(listener, tls_config, runtime, route_id)` — accepts a pre-bound `tokio::net::TcpListener`, keys the registry by the listener's real `local_addr` port, returns `(WsAppState, SocketAddr, Option<axum_server::Handle<SocketAddr>>)` so callers learn the bound address.
2. `WsConsumer::start_with_listener(ctx, listener)` — parallel to `start`, using the injected listener instead of binding from the URI port.
3. Plain `get_or_spawn` / `start` remain, signatures unchanged (additive API only).
4. Migrate the 22 ws lib-test callsites to bind-`0` + `local_addr()` + injected listener; retire the ws-local `free_port` helper.

## Acceptance Criteria

- `grep -c 'free_port' crates/components/camel-ws/src/lib.rs` = 0 (helper removed).
- New Rust library tests, each named and asserting exactly:
  - `with_listener_port_zero_returns_real_bound_addr`: spawn with a `127.0.0.1:0` listener → returned `SocketAddr` port equals the listener's `local_addr()` port and is non-zero; a TCP connect to the returned address succeeds.
  - `with_listener_same_port_reuses_entry`: second `get_or_spawn_with_listener` on the same pre-bound socket → same entry serves both (ref-count 2, no rebind error).
  - `with_listener_tls_mismatch_errors`: plain entry on port P, then TLS `get_or_spawn_with_listener` on P → returns an error.
  - `start_with_listener_round_trips_without_port_guess`: bind-0 → `local_addr()` → URI → `start_with_listener` → producer round-trips a message; no `free_port` and no readiness retry used for port acquisition.
  - `injected_entry_survives_consumer_stop`: start consumer via injected listener on P, round-trip a message, stop the consumer → a NEW consumer on P reuses the entry and round-trips again without rebinding.
  - `reset_clears_injected_entry_allowing_rebind`: spawn via injected listener on P, run test-only `ServerRegistry::reset()` → a fresh bind to `127.0.0.1:P` succeeds (entry gone) and a new spawn serves traffic.
- Full `camel-component-ws` suite green (lib + integration binaries).

## Risk Budget

- Registry re-keying (actual port, not requested port) must not regress TLS-path lifecycle; covered by scenarios.
- No breaking changes: existing `get_or_spawn` callers (non-ws) unaffected.
- Scope guard: `camel-test`'s `find_free_port` (47 callsites) and wasm (33) are follow-ups, NOT this change.
