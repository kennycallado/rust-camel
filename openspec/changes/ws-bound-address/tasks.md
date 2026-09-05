# Tasks: ws-bound-address

### Task 1: ServerRegistry listener injection and stored bound address

**Files:**
- `crates/components/camel-ws/src/lib.rs` (modified)

**Steps:**
1. Add a `bound_addr: SocketAddr` field to `ServerHandle` (the OnceCell payload — NOT `ServerRegistryInner`, which is built at map insert before spawn and cannot be written back through the Mutex from the init closure); set it inside `spawn_server` where `local_addr()` is captured. Every holder reads the stored value, never a listener re-read. This is structurally correct for both orderings (injected-first and legacy `get_or_spawn`-first).
2. Refactor `spawn_server` (lib.rs:~268-320) to accept a `listener: tokio::net::TcpListener` parameter. BOTH legacy paths bind BEFORE delegating: plain `get_or_spawn` binds `TcpListener::bind((host, port))` exactly as today, captures `local_addr()` (currently discarded at lib.rs:299), and delegates with the live listener; legacy TLS `get_or_spawn` binds its listener first, then converts it via `into_std()` and serves with `axum_server::from_tcp_rustls` (axum-server 0.8.0 takes `std::net::TcpListener`), retaining `Handle<SocketAddr>::listening()` semantics. The injected path receives the already-bound listener through the same `spawn_server` routes.
3. Add `pub async fn get_or_spawn_with_listener(&'static self, listener: tokio::net::TcpListener, tls_config: Option<WsTlsConfig>, runtime: Arc<dyn RuntimeObservability>, route_id: String) -> Result<(WsAppState, SocketAddr, Option<axum_server::Handle<SocketAddr>>), CamelError>` mirroring `get_or_spawn` (lib.rs:115-204): read `listener.local_addr()` BEFORE any registry mutation, key the map by the actual port, exactly-once spawn inside the existing `OnceCell::get_or_try_init`, ref-count, TLS-mode mismatch on same actual port → same error as today. When an existing entry wins, the redundant injected listener is dropped; ref-count and error behavior unchanged.
4. Keep `get_or_spawn` signature and observable behavior byte-compatible (it returns the same `(WsAppState, Option<Handle>)` as today — no address added).
5. `#[cfg(test)] reset()` (lib.rs:214-223) needs no logic change: injected entries abort and clear like any other (the new field lives in the same map).

**Tests (all in lib.rs test module, guarded by the existing `REGISTRY_TEST_LOCK` where the suite convention requires):**
- `with_listener_port_zero_returns_real_bound_addr`
  - setup: empty registry (call `ServerRegistry::reset()` first under the lock)
  - action: bind `TcpListener::bind("127.0.0.1:0")`, call `get_or_spawn_with_listener` with plain (None) TLS config
  - assert: returned `SocketAddr` port equals `listener.local_addr()` port and is non-zero; a `TcpStream::connect` to the returned address succeeds
  - command: `cargo test -p camel-component-ws --lib with_listener_port_zero -- --nocapture`
  - expected: fails before step 3 exists (method not found), passes after
- `with_listener_same_port_reuses_entry`
  - setup: `ServerRegistry::reset()`; one `get_or_spawn_with_listener` spawn on a bound socket, port P
  - action: second `get_or_spawn_with_listener` with a clone of the same socket (`std::net::TcpListener::try_clone()` before the tokio conversion)
  - assert: returns without error and without rebinding; `ServerRegistry::ref_count_for_test(P)` (new `#[cfg(test)]` accessor, `fn ref_count_for_test(&'static self, port: u16) -> usize`, added in this task) = 2; two client connects to P both succeed
  - command: `cargo test -p camel-component-ws --lib with_listener_same_port`
  - expected: fails before, passes after
- `legacy_get_or_spawn_after_injected_reuses_entry`
  - setup: `ServerRegistry::reset()`; injected spawn on port P via `get_or_spawn_with_listener`, plain TLS mode
  - action: mixed-entry `get_or_spawn("127.0.0.1", P, None, …)` with matching TLS mode
  - assert: no rebinding error; `ref_count_for_test(P)` = 2; client connect to P still served
  - command: `cargo test -p camel-component-ws --lib legacy_get_or_spawn_after_injected`
  - expected: fails before, passes after
- `legacy_get_or_spawn_unchanged_after_refactor`
  - setup: `ServerRegistry::reset()`; pick a free port P by binding `127.0.0.1:0`, reading the port, and dropping the probe listener (test-infra port pick, not the removed helper)
  - action: plain `get_or_spawn("127.0.0.1", P, None, …)`; then TLS `get_or_spawn("127.0.0.1", P, Some(tls), …)` on the same P
  - assert: first call returns `(WsAppState, Handle-option)` exactly as the pre-refactor API (binds: TCP connect to P succeeds; `ref_count_for_test(P)` = 1); second call errors with the TLS-mode mismatch, unchanged
  - command: `cargo test -p camel-component-ws --lib legacy_get_or_spawn_unchanged`
  - expected: passes before AND after (behavior-preserving guard)
- `with_listener_tls_mismatch_errors`
  - setup: plain (None) spawn on port P via listener injection
  - action: `get_or_spawn_with_listener` with `Some(tls_config)` on the same P
  - assert: returns `Err` (TLS-mode mismatch, same error variant as `get_or_spawn` today, lib.rs:198)
  - command: `cargo test -p camel-component-ws --lib with_listener_tls_mismatch`
  - expected: fails before, passes after
- `reset_clears_injected_entry_allowing_rebind`
  - setup: spawn via injected listener on port P (P = real port from `local_addr()`)
  - action: run `ServerRegistry::reset()`; then `TcpListener::bind(("127.0.0.1", P))`
  - assert: the fresh bind succeeds (entry cleared, no listener leak) and a new `get_or_spawn_with_listener` on the new listener serves a client connect
  - command: `cargo test -p camel-component-ws --lib reset_clears_injected`
  - expected: fails before only if reset skips injected entries; passes after

**Acceptance:**
- `cargo test -p camel-component-ws --lib` green (existing suite + 6 new tests).
- `cargo clippy -p camel-component-ws --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.
- Existing `get_or_spawn` callers in lib.rs compile unchanged (no signature edit outside the new method).

- [x] 1.1

### Task 2: WsConsumer::start_with_listener consumer surface

**Files:**
- `crates/components/camel-ws/src/lib.rs` (modified)

**Steps:**
1. Add `pub async fn start_with_listener(&mut self, ctx: ConsumerContext, listener: tokio::net::TcpListener) -> Result<(), CamelError>` on `WsConsumer` (existing `start` is at lib.rs:1130, taking `&mut self, ctx: ConsumerContext`), mirroring `GrpcConsumer::start_with_listener` (crates/components/camel-component-grpc/src/consumer.rs:434-450) and the existing `WsConsumer::start` (lib.rs:1130-1200): derive the server key from `listener.local_addr()`; URI path/auth/segment config still comes from the endpoint as today; TLS mode from consumer config (lib.rs:1150-1159), validated against any existing entry on the same actual port (mismatch → same error path as `start`).
2. `start_with_listener` calls `ServerRegistry::get_or_spawn_with_listener` (Task 1) and marks `ctx.mark_ready()` on success for the plain path exactly as `start` does (lib.rs:1180-1182); TLS path awaits `Handle::listening()` then marks ready.
3. The URI's host:port is informational under this entry point — the listener is authoritative for binding; do not parse the URI port for the server key.
4. Export/document the method at the same visibility as `start` (pub), no feature gates.

**Tests:**
- `start_with_listener_round_trips_without_port_guess`
  - setup: bind `127.0.0.1:0`, read `local_addr()`, build the echo endpoint with `format!("ws://127.0.0.1:{}/echo", addr.port())` and a consumer exactly like `echo_flow_round_trips_message_through_consumer_and_producer` (lib.rs:2027-2101) — but with NO `free_port` and NO `connect_until_ready` for port acquisition
  - action: `consumer.start_with_listener(ctx, listener)`; producer send + await echo (client dial retries remain allowed for connect timing, not port acquisition)
  - assert: message round-trips; server is reachable at the pre-read address
  - command: `cargo test -p camel-component-ws --lib start_with_listener_round_trips`
  - expected: fails before (method not found), passes after
- `injected_entry_survives_consumer_stop`
  - setup: consumer A started via `start_with_listener` on port P, one message round-tripped
  - action: stop consumer A; start consumer B on the same P via `get_or_spawn`/`start` (mixed entry); round-trip another message
  - assert: B reuses the entry (no rebind error) and the message round-trips
  - command: `cargo test -p camel-component-ws --lib injected_entry_survives_consumer_stop`
  - expected: fails before, passes after

**Acceptance:**
- `cargo test -p camel-component-ws --lib` green including the 2 new tests.
- `cargo clippy -p camel-component-ws --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.

- [x] 1.2

### Task 3: Migrate ws lib tests off free_port and retire the helper

**Files:**
- `crates/components/camel-ws/src/lib.rs` (modified)

**Steps:**
1. Enumerate callsites: `grep -n 'free_port' crates/components/camel-ws/src/lib.rs` (22 callsites + 1 comment expected; helper at lib.rs:1847-1853).
2. Migrate each callsite by shape (all shapes: only port-acquisition and start change; path/auth/TLS/assertions preserved):
   - Consumer-start tests (majority): `let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); let addr = listener.local_addr().unwrap();`, build the URI from `addr.port()`, call `consumer.start_with_listener(ctx, listener)` (Task 2).
   - Direct registry tests (call `get_or_spawn` directly): bind `127.0.0.1:0`, KEEP the listener, read `local_addr()`, call `get_or_spawn_with_listener(listener, …)` and use the returned `SocketAddr`.
   - `producer_retries_on_connection_refused`: build the URI with port `0` directly (`ws://127.0.0.1:0/…`) — an instant deterministic refusal, no bind-close-rebind probe at all.
   - The existing raw-listener test that already binds port 0 and keeps the listener (lib.rs:~2288-2295): leave the bind pattern unchanged; remove only its stale TOCTOU comment.
3. Tests that start RAW external servers (not via WsConsumer) where a listener is already kept: leave their existing bind-0 pattern alone — migrate only `free_port` uses.
4. Delete `fn free_port` (lib.rs:1847-1853) and the stale comment reference after the last callsite moves.
5. `connect_until_ready` (lib.rs:1858-1872) stays (client dial timing), but migrated tests no longer depend on it surviving a port race.

**Tests:**
- `free_port_absent`
  - setup: after migration
  - action: `grep -c 'free_port' crates/components/camel-ws/src/lib.rs`
  - assert: output `0` (helper and all references gone)
  - command: `test "$(grep -c 'free_port' crates/components/camel-ws/src/lib.rs)" = "0"`
  - expected: fails before migration, passes after
- Full-suite regression
  - command: `cargo test -p camel-component-ws` (lib + all `tests/` binaries)
  - assert: all targets green, zero failures
  - expected: green before and after (behavior-preserving migration)

**Acceptance:**
- `grep -c 'free_port' crates/components/camel-ws/src/lib.rs` = 0.
- `cargo test -p camel-component-ws` all targets green.
- `cargo clippy -p camel-component-ws --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.
- No changes outside `crates/components/camel-ws/src/lib.rs` (`git status --short` shows only that file modified).

- [x] 1.3
