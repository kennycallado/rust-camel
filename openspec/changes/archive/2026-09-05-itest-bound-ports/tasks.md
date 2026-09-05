# Tasks: itest-bound-ports

## camel-component-http

### Task 1: HTTP ServerRegistry staged listeners

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified) — package `camel-component-http`

**Steps:**
1. Change `ServerRegistry.inner` from `Mutex<HashMap<ServerKey, Arc<OnceCell<ServerHandle>>>>` to `Mutex<RegistryState>` where `struct RegistryState { entries: HashMap<ServerKey, Arc<OnceCell<ServerHandle>>>, staged: HashMap<ServerKey, tokio::net::TcpListener> }` (private struct in the same module). Update EVERY existing `inner.lock()` access to `guard.entries`: `get_or_spawn` (~line 824), `reset()` (~1018 — must now clear BOTH `entries` and `staged`), and the four test-mod lock sites (~5024, 5032, 5126, 5157).
2. Add `bound_addr: std::net::SocketAddr` to `ServerHandle`.
3. Add `pub async fn stage_listener(&'static self, listener: tokio::net::TcpListener) -> Result<(), CamelError>`: derive key from `listener.local_addr()` (IP string, port); under the registry mutex, `Err(EndpointCreationFailed(format!("listener already staged for {host}:{port}")))` if the key is occupied, else insert.
4. Extract the vacant-entry creation of `get_or_spawn` (the `cell.get_or_try_init` body) into a private `async fn spawn_entry(key: ServerKey, source: ListenerSource, max_request_body: usize, max_response_body: usize, max_inflight_requests: usize, runtime: Arc<dyn RuntimeObservability>, route_id: String, tls_config: Option<crate::config::ServerTlsConfig>) -> Result<Arc<ServerHandle>, CamelError>` where `enum ListenerSource { Bind, Staged(tokio::net::TcpListener) }`. `Bind` keeps today's behavior (bind `host:port`, then the existing `into_std()` + `run_axum_server_tls(std_listener, rustls_cfg, monitor-handle args as today)` call — `run_axum_server_tls` ALREADY accepts `std::net::TcpListener`, no signature change). `Staged(l)` passes `l.into_std()` through the same TLS path, and the plain path serves the owned tokio listener directly via the existing `run_axum_server` (which already takes the bound listener). Record `bound_addr` from the served listener's `local_addr()` in both paths.
5. In `get_or_spawn`, while holding the registry-mutex guard AND only when the entry cell is vacant (`cell.get().is_none()`): if `guard.staged` holds the exact `(host, port)` key, take it and pass `ListenerSource::Staged` into the init closure; else if any staged key has the same `port` with a different host string, return `Err(EndpointCreationFailed(format!("staged listener conflict on port {port}: staged under host {staged_host}, requested {host}")))` WITHOUT creating the entry; else `ListenerSource::Bind`. Occupied cells never touch the staged map. The guard is released before the OnceCell init awaits (as today).
6. Add `pub async fn get_or_spawn_with_listener(&'static self, listener: tokio::net::TcpListener, max_request_body: usize, max_response_body: usize, max_inflight_requests: usize, runtime: Arc<dyn RuntimeObservability>, route_id: String, tls_config: Option<crate::config::ServerTlsConfig>) -> Result<HttpRouteRegistry, CamelError>`: key from `listener.local_addr()`, identical eviction/compat checks, spawns via `ListenerSource::Staged(listener)` directly. If the key already holds a live entry, run the same compat checks and REUSE the entry; the passed listener is simply dropped (the existing entry already serves the key — consistent with the vacant-path contract, no error).
7. Add `pub fn bound_addr(&'static self, host: &str, port: u16) -> Option<std::net::SocketAddr>` reading the initialized entry under the mutex.

**Tests:** (in the existing `#[cfg(test)]` mod of lib.rs; `LIMITS` below = the same default-limit constants the existing registry tests in this file use; CLONE-FIXTURE = bind `std::net::TcpListener` on the target addr, `let probe = l.try_clone().expect("clone")` (std handle to the SAME socket, held open by the test), `l.set_nonblocking(true)`, `let listener = tokio::net::TcpListener::from_std(l).expect("from_std")` — `tokio::net::TcpListener` has no `try_clone`, so clones come from the std handle)
1. `name`: `staged_listener_first_spawn_serves_without_second_bind`
   `setup`: listener via CLONE-FIXTURE on `127.0.0.1:0`; `P = local_addr().port()`; staged via `stage_listener(listener)`.
   `action`: `ServerRegistry::global().get_or_spawn("127.0.0.1", P, LIMITS).await`.
   `assert`: Ok; `bound_addr("127.0.0.1", P) == Some(listener_local_addr)`; a plain HTTP request to the port returns a response from the served socket (any status — the clone shares the socket, so `accept` on `probe` would compete with the server; service is proven by the HTTP response plus `bound_addr`).
   `command`: `cargo test -p camel-component-http --lib staged_listener_first_spawn`
   `expected`: fails before steps 3-6 exist (no `stage_listener`), passes after.
2. `name`: `staged_entry_reused_by_second_caller`
   `setup`: staged spawn done as in test 1 (slot consumed).
   `action`: second `get_or_spawn("127.0.0.1", P, LIMITS)`.
   `assert`: Ok; `bound_addr("127.0.0.1", P)` unchanged and equal to the staged listener's address (entry reused, no second bind, no re-stage possible).
3. `name`: `unstaged_spawn_binds_legacy`
   `setup`: no staging; fresh port P2.
   `action`: `get_or_spawn("127.0.0.1", P2, LIMITS)`.
   `assert`: Ok; connect to `P2` succeeds; `bound_addr("127.0.0.1", P2) == Some(("127.0.0.1", P2).into())`.
4. `name`: `wrong_host_staged_port_fails_deterministically`
   `setup`: listener staged under `("127.0.0.1", P)`; no entry exists for P.
   `action`: `get_or_spawn("localhost", P, LIMITS)`.
   `assert`: `Err` whose message contains `staged listener conflict on port`; afterwards `get_or_spawn("127.0.0.1", P, LIMITS)` succeeds and serves the staged listener (slot untouched by the failed call).
5. `name`: `duplicate_stage_same_key_rejected`
   `setup`: listener A (with `probe` clone handle from CLONE-FIXTURE) staged under its key; `let b = tokio::net::TcpListener::from_std(probe.try_clone().expect("clone2")).expect("from_std2")` after `set_nonblocking(true)` — a second tokio handle to the SAME socket (a fresh bind on an occupied port is impossible without SO_REUSEPORT).
   `action`: `stage_listener(b)`.
   `assert`: `Err` whose message contains `listener already staged`; `get_or_spawn` on the key yields `bound_addr == A.local_addr()` (first retained).
6. `name`: `distinct_keys_stage_independently`
   `setup`: listeners staged under `("127.0.0.1", P1)` and `("127.0.0.1", P2)`.
   `action`: `get_or_spawn` both keys.
   `assert`: each `bound_addr` equals its own listener's address; both connect.
7. `name`: `tls_prebound_listener_served`
   `setup`: staged listener; TLS config built with the SAME cert/key fixture helper the existing TLS registry tests in this file use.
   `action`: `get_or_spawn_with_listener(listener, LIMITS, Some(tls_config))`; then an HTTPS GET via the crate's existing test TLS client helper.
   `assert`: TLS handshake succeeds on the staged port; response served; `bound_addr` equals the staged local addr.
8. `name`: `with_listener_direct_spawn_keyed_by_actual_addr`
   `setup`: un-staged listener bound to `127.0.0.1:0`.
   `action`: `get_or_spawn_with_listener(listener, LIMITS, None)`.
   `assert`: registry key is the listener's actual port (connect succeeds); a second caller via legacy `get_or_spawn("127.0.0.1", actual_port, LIMITS)` reuses the entry (Ok, no second bind).

**Acceptance:**
- `cargo test -p camel-component-http --all-targets` green (existing suite + 8 new).
- `cargo clippy -p camel-component-http --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` clean; `cargo xtask lint-unwrap` reports no new `unwrap` in the diff.

- [x] 1

## camel-component-ws

### Task 2: WS ServerRegistry staged listener consumption

**Files:**
- `crates/components/camel-ws/src/lib.rs` (modified)

**Steps:**
1. Extend the ws registry holder: `struct ServerRegistry { inner: Mutex<HashMap<u16, ServerRegistryInner>>, staged: Mutex<HashMap<(String, u16), tokio::net::TcpListener>> }` — the entries map stays port-keyed and untouched; the staged map is separate and keyed by `(host_string, port)` so the wrong-host contract is honored. Update the `global()` initializer.
2. Add `pub async fn stage_listener(&'static self, listener: tokio::net::TcpListener) -> Result<(), CamelError>`: key from `local_addr()`; `Err(EndpointCreationFailed(format!("listener already staged for {host}:{port}")))` if occupied, else insert.
3. In `get_or_spawn`'s vacant-entry creation path ONLY (the path that would bind a fresh listener), acquire the staged lock AFTER releasing the entries guard (no nested guards — lock-order safety): exact `(host, port)` match → take the listener and serve it exactly as `get_or_spawn_with_listener` (bd rc-9xsv) does; any staged key with the same `port` under a different host string → `Err(EndpointCreationFailed(format!("staged listener conflict on port {port}: staged under host {staged_host}, requested {host}")))` without binding; no staged entry for the port → legacy bind, unchanged. The eviction path and the existing-entry reuse path NEVER touch the staged map (process-lifetime entries: staged consumption only on vacant-entry creation).
4. Doc-comment on `stage_listener` stating: one-shot per exact `(host, port)` key; duplicate staging rejected; unclaimed listeners dropped at process end (test bug, not runtime hazard).

**Tests:** (existing `#[cfg(test)]` mod; ARGS below = the exact argument list the existing registry tests in this file pass to `get_or_spawn`; follow the rc-9xsv registry tests' style; CLONE-FIXTURE = the std-bind + `try_clone` + `set_nonblocking(true)` + `from_std` pattern defined in task 1's Tests preamble — `tokio::net::TcpListener` has no `try_clone`)
1. `name`: `ws_staged_listener_consumed_on_vacant_entry`
   `setup`: tokio listener `127.0.0.1:0`, `P = port()`, staged.
   `action`: `ServerRegistry::global().get_or_spawn("127.0.0.1", P, ARGS).await`.
   `assert`: server serves on the staged socket (`TcpStream::connect` to the staged addr succeeds); `ref_count_for_test(P) == 1`; the entry's bound address (rc-9xsv `bound_addr` surface) equals the staged listener's local address — proving the served socket IS the staged socket, consumed one-shot on the vacant-entry path.
2. `name`: `ws_staged_not_consumed_when_entry_exists`
   `setup`: CLONE-FIXTURE listener on `127.0.0.1:0` → `P`, retaining TWO probe clones (`probe1`, `probe2`); create the entry via the EXISTING rc-9xsv `get_or_spawn_with_listener(listener, ARGS-equivalent config)` (binding P a second time is impossible — the entry must come from the listener); the entry now serves P. Then stage `from_std(probe1.try_clone())` (nonblocking set) for `("127.0.0.1", P)` — the slot is empty, staging succeeds.
   `action`: second `get_or_spawn("127.0.0.1", P, ARGS)` (another consumer).
   `assert`: entry reused, `ref_count_for_test(P) == 2`; the staged slot is still occupied — `stage_listener(from_std(probe2.try_clone()))` now returns `Err` containing `listener already staged` (proves the staged handle was never consumed on the reuse path).
3. `name`: `ws_wrong_host_staged_port_fails`
   `setup`: staged under `("127.0.0.1", P)` on a port with NO entry.
   `action`: `get_or_spawn("localhost", P, ARGS)`.
   `assert`: `Err` containing `staged listener conflict on port {P}`; afterwards `get_or_spawn("127.0.0.1", P, ARGS)` succeeds on the staged listener (slot untouched).
4. `name`: `ws_duplicate_stage_rejected`
   `setup`: listener A staged (CLONE-FIXTURE, probe retained); `let b = tokio::net::TcpListener::from_std(probe.try_clone().expect("clone2")).expect("from_std2")` after `set_nonblocking(true)` — same socket, second handle.
   `action`: `stage_listener(b)`.
   `assert`: `Err` containing `listener already staged`; subsequent `get_or_spawn` on the key serves A's socket (first retained).
5. `name`: `ws_distinct_keys_stage_independently`
   `setup`: listeners staged under `("127.0.0.1", P1)` and `("127.0.0.1", P2)`, no entries exist for either port.
   `action`: `get_or_spawn("127.0.0.1", P1, ARGS)` then `get_or_spawn("127.0.0.1", P2, ARGS)`.
   `assert`: each port serves on its own staged socket (both `TcpStream::connect` checks succeed); `ref_count_for_test(P1) == 1` and `ref_count_for_test(P2) == 1` — no cross-key interference.

**Acceptance:**
- `cargo test -p camel-component-ws --all-targets` green (153 existing + 5 new).
- `cargo clippy -p camel-component-ws --all-targets -- -D warnings` exits 0; fmt clean; no new `unwrap`.

- [x] 2

## camel-test

### Task 3: camel-test staged-listener helpers and callsite migration

**Files:**
- `crates/camel-test/tests/support/mod.rs` (modified)
- `crates/camel-test/tests/http_test.rs` (modified)
- `crates/camel-test/tests/audience_substitution_test.rs` (modified)
- `crates/camel-test/tests/auth_multi_credential_test.rs` (modified)
- `crates/camel-test/tests/kernel_fail_closed_test.rs` (modified)
- `crates/camel-test/tests/late_registration_gate_test.rs` (modified)

**Steps:**
1. In `tests/support/mod.rs`: delete the `pub fn find_free_port` block at the top of the file. Add `pub async fn stage_http_listener(host: &str) -> u16` — binds `tokio::net::TcpListener` on `{host}:0`, calls `camel_component_http::ServerRegistry::global().stage_listener(listener).await`, panics with a message naming the helper and host on error, returns the actual port. Add `pub async fn stage_ws_listener(host: &str) -> u16` doing the same against `camel_component_ws::ServerRegistry::global()`. The host parameter MUST equal the host each route URI declares (exact-key contract): binding `0.0.0.0:0` and staging under `("0.0.0.0", P)` is valid and intentional.
2. `http_test.rs` (10 sites): 8 sites at lines 38, 125, 329, 376, 409, 467, 629, 925 use `127.0.0.1` routes → `let port = support::stage_http_listener("127.0.0.1").await;`. 2 sites use `0.0.0.0` routes (the `build_secure_route` helper feeding `http://0.0.0.0:{port}/{path}` and the yaml-compiled test's `from: http://0.0.0.0:{port}/yaml-compiled`) → `stage_http_listener("0.0.0.0")`. Line numbers may drift; migrate by the URI host each site formats.
3. `audience_substitution_test.rs` (2 sites): the `HttpComponent` builder site (http `127.0.0.1`) → `stage_http_listener("127.0.0.1")`; the `WsComponent` builder site (ws `127.0.0.1`) → `stage_ws_listener("127.0.0.1")`.
4. `auth_multi_credential_test.rs` (1 site): route is `http://0.0.0.0:{port}/{path}` → `stage_http_listener("0.0.0.0")`.
5. `kernel_fail_closed_test.rs` (2 sites): both routes dial/bind `127.0.0.1` → `stage_http_listener("127.0.0.1")`.
6. `late_registration_gate_test.rs` (2 sites): the loopback e2e site (`http://127.0.0.1:{port}/first|late`) → `stage_http_listener("127.0.0.1")`; the nonloopback site (`0.0.0.0`, intentional coverage per the file's doc comment) → `stage_http_listener("0.0.0.0")`.
7. Verify zero probes remain: `grep -rn find_free_port crates/camel-test/` returns nothing.
8. Run every affected binary in full (not library-only), per bd rc-h0aw's oracle requirement.

**Tests:**
1. `name`: migrated binaries full-target pass (existing suites are the e2e evidence: each exercises `staged-port-survives-to-serve` — the served socket is the staged socket)
   `setup`: helpers staged into the process-global registries as implemented in tasks 1-2.
   `action`: run all five binaries.
   `assert`: every run exits 0, zero failures.
   `command`: `cargo test -p camel-test --features integration-tests --test http_test --test audience_substitution_test --test auth_multi_credential_test --test kernel_fail_closed_test --test late_registration_gate_test`
   `expected`: before migration these binaries pass with the probe helper; after migration they must pass with staging (regression-free) and the helper no longer exists.
2. `name`: `no_port_probes_remain`
   `action`: `grep -rn find_free_port crates/camel-test/`
   `assert`: no output (exit 1 from grep).

**Acceptance:**
- The combined `cargo test -p camel-test --features integration-tests --test` command from test 1 exits 0.
- `grep -rn find_free_port crates/camel-test/` empty.
- `cargo clippy -p camel-test --all-targets --features integration-tests -- -D warnings` exits 0; fmt clean.
- `cargo test -p camel-component-http --all-targets` and `cargo test -p camel-component-ws --all-targets` still green after the helper lands (no cross-crate breakage).

- [x] 3
