# Design: ws-bound-address

## Context

`ServerRegistry` (`crates/components/camel-ws/src/lib.rs:103-224`) keys servers by requested `u16` port; `get_or_spawn(host, port, tls, runtime, route_id)` binds internally (`spawn_server`, lib.rs:299) and the plain-ws path discards `local_addr()`. The TLS path already holds an `axum_server::Handle<SocketAddr>` but `WsConsumer::start` discards it (lib.rs:1180-1182). Tests therefore freeze a `free_port()` guess into the endpoint URI before any bind exists.

Precedent (exact shape): `GrpcServerRegistry::get_or_spawn_with_listener` (`crates/components/camel-component-grpc/src/server.rs:187`) + `GrpcConsumer::start_with_listener` (`consumer.rs:434-450`), registry keyed by the injected listener's actual port.

## Architecture

Components layer, `camel-component-ws` only. No new dependencies (ADR-0069 §6); `camel-test` depends on this crate, so the API is importable where needed. `ConsumerContext` (camel-component-api) is NOT the surface — it is constructed before `start` and cannot carry the address back without write-back plumbing; ADR-0069 §5 frames this as an operator/test-facing registry surface.

## Decisions

1. **Registry key = actual port; entry owns the bound address.** `get_or_spawn_with_listener` reads `listener.local_addr()` before any registry mutation; a port-0 listener keys under its real port (same solution as grpc). The bound `SocketAddr` is stored inside the registry entry (`ServerHandle`/OnceCell payload) so every holder — including later `get_or_spawn(host, P, …)` callers — receives the *entry's* address, never a re-read from a listener that may already be gone. When an existing entry wins the race, the redundant injected listener is simply dropped (caller-held); ref-count and error behavior are unchanged.
2. **Internal refactor, additive surface.** `spawn_server` gains a listener parameter (plain path binds then delegates; injected path receives the listener). `get_or_spawn` keeps its exact signature and behavior. New `get_or_spawn_with_listener` returns `(WsAppState, SocketAddr, Option<Handle>)`. Plain path captures `local_addr()` too (was discarded) — no signature change needed there, the addr is returned by the new method only.
3. **Consumer surface.** `WsConsumer::start_with_listener(ctx, listener)` mirrors grpc: derives the server key from `listener.local_addr()`, reuses URI path/auth config from the endpoint. The URI's host:port is informational under this entry point; the listener is authoritative. TLS mode comes from consumer config as today, validated against any existing entry on the same port (mismatch → error, unchanged).
4. **TLS path.** Pinned `axum-server 0.8.0` accepts `std::net::TcpListener`: the injected `tokio::net::TcpListener` is converted via `into_std()` and served with `from_tcp_rustls` (or `from_tcp(...).acceptor(...)` with the rustls acceptor). `Handle<SocketAddr>::listening()` continues to yield the real address. Both plain and TLS paths accept injection.
5. **Lifecycle unchanged.** Stopping an injected consumer never tears down the process-lifetime server (existing semantics); the test-only `reset()` aborts tasks and clears injected entries exactly like non-injected ones, permitting rebind on the next spawn.
6. **Test migration.** Mechanical per callsite: `TcpListener::bind("127.0.0.1:0").await` → `local_addr()` → build URI with real port → `start_with_listener`. `connect_until_ready` stays where network timing still applies (client dial), but its 5s retry is no longer load-bearing for port races in migrated tests. Retire `free_port` (lib.rs:1847) after the last callsite moves.

## Risks / Trade-offs

- Registry map type unchanged (`u16` keys) — only the key *source* changes for the injected path.
- OnceCell spawn semantics, ref-counting, and `REGISTRY_TEST_LOCK` serialization are untouched.
- Migration is test-only code; production paths (`get_or_spawn` callers) see zero behavior change.

## Scope Guards

Out of scope: `camel-test::find_free_port` migration, wasm suites, producer-side changes (producer dials out; no listener involved), any `ConsumerContext` change.
