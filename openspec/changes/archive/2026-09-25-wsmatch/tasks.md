# Tasks: wsmatch

## camel-component-ws

### Task 1.1: Regression test — path-sharing servers must not cross-register connections

**Files:**
- `crates/components/camel-ws/src/lib.rs` (modified — tests module only)

**Steps:**
1. In the `tests` module of `crates/components/camel-ws/src/lib.rs`, add test `accept_registers_into_owning_server_registry_only`. Arrange: two isolated `Arc<ServerRegistry::new()` instances; two ephemeral listeners bound to distinct `127.0.0.1:0` ports; two `WsConsumer::with_server_registry` consumers started via `start_with_listener`, BOTH serving the SAME path `/echo-cross`; one echo route each via `spawn_echo_route`; both full-triple keys `(String "127.0.0.1", port, "/echo-cross")` present in `global_registries()` (assert before acting).
2. Act 1 (membership leg): connect client A to server A via `connect_until_ready`, send `"a"`, assert `recv_client_text` == `"a"` (wrap in `tokio::time::timeout(Duration::from_secs(2), …)`); then connect client B to server B, send `"b"`, same bounded echo assert.
3. Assert 1: `global_registries().get(&key_a)` registry `len() == 1` AND `.get(&key_b)` registry `len() == 1`. Pre-fix this fails reliably: both connections insert into whichever same-path entry the DashMap iteration yields first, so one registry holds 2 and the other 0 (a mid-test global-map rehash could in principle mask the red — if the test unexpectedly passes pre-fix, re-run it before concluding the bug is absent).
4. Act 2 (stop-isolation leg): `consumer_a.stop().await.unwrap()`, await route task A. Then client B sends `"still-here"` and asserts bounded echo == `"still-here"`. Client A's next read returns a Close frame or stream error within the same 2s bound.
5. Run `cargo test -p camel-component-ws --lib accept_registers_into_owning_server_registry_only` and record the failure (expected red before Task 1.2).

**Tests:** (executable spec)
- `accept_registers_into_owning_server_registry_only`: two servers same path different host:port, one client each → both full-triple keys present in `global_registries()`; each key's registry `len() == 1`; `stop()` on consumer A leaves client B echo-round-tripping within 2s while client A observes close/error.
  - command: `cargo test -p camel-component-ws --lib accept_registers_into_owning_server_registry_only`
  - expected before Task 1.2: FAIL (either client B's echo times out, or a membership count is 0/2). After Task 1.2: PASS.

**Acceptance:**
- Test compiles against current code (uses only existing symbols: `ServerRegistry`, `WsConsumer::with_server_registry`, `spawn_echo_route`, `connect_until_ready`, `recv_client_text`, `global_registries`) and FAILS when run.

- [x] 1.1

### Task 1.2: Server-scoped connection registry resolution on WsAppState

**Files:**
- `crates/components/camel-ws/src/lib.rs` (modified)
- `crates/camel-test/tests/ws_security_test.rs` (modified — `make_app_state` gains the new `WsAppState` field)

**Steps:**
1. Add field to `pub struct WsAppState`: `pub registries: Arc<DashMap<String, Arc<WsConnectionRegistry>>>` with a doc comment: per-server owned map, path → connection registry, symmetric with `dispatch`/`path_configs`; the accept path resolves here so a connection can only register into its own server's registry.
2. In `spawn_server`, construct `let registries: Arc<DashMap<…>> = Arc::default();` alongside `path_configs` and pass it into the `WsAppState { … }` literal.
3. In `finish_start`, insert into the server-scoped map BEFORE the `state.dispatch` table insert (line ~1671) and before the existing `global_registries().insert(registry_key.clone(), Arc::clone(&self.registry));` (line ~1690): `state.registries.insert(self.cfg.inner.path.clone(), Arc::clone(&self.registry));` — this ordering closes the accept-but-unregistered window entirely: an accept landing between the two inserts is rejected at the dispatch check instead of being admitted as an unregistered writer-only connection.
4. In `ws_handler`, delete the `for entry in registry.iter() { if entry.key().2 == path … }` scan (lines ~1077–1085) and replace with a single server-scoped lookup: resolve `Option<Arc<WsConnectionRegistry>>` from `state.registries.get(&path)`; when `Some`, `registry.insert(connection_key.clone(), out_tx.clone())`. When `None`, leave the connection unregistered (writer-only), matching today's no-match behavior.
5. Update the over-limit block (~line 1112): replace both `registry.get(key)` global reads with `len()`/`remove(&connection_key)` on the resolved server-scoped `Arc<WsConnectionRegistry>` from step 4; drop the now-unused `registry_key` Option dance if no reader remains.
6. In `stop()`, inside the `if let Some(state) = self.server_state.take()` block that removes `path_policies`/`dispatch`/`path_configs` by path, add `state.registries.remove(&self.cfg.inner.path);`. The existing full-key `global_registries().remove(&key)` stays unchanged.
7. Fix ALL out-of-crate `WsAppState { … }` struct-literal construction sites to include the new field — grep the workspace for `WsAppState {` to enumerate. Known sites: the two camel-ws test constructions (~lines 4725 and 4742; use `registries: Arc::default()` or an explicit map when the test relies on accept-path registration) and `make_app_state()` in `crates/camel-test/tests/ws_security_test.rs` (~line 133; `registries: Arc::default()` — that harness drives `dispatch_handler` through its own Router and never uses accept-path registration).
8. Update the two stale warning comments above `injected_entry_survives_consumer_stop` and `consumer_injection_uses_provided_registry`: the "PATH ONLY (host+port ignored)" rationale is obsolete — registration is now server-scoped; keep any still-true note about avoiding collision with the `/echo` global-semantics test only if that test still requires it, otherwise delete the rationale sentences.
9. Record the empirical pre-change lib-suite pass count (`cargo test -p camel-component-ws --lib` on the pre-change tree, or read the last full run), then run Task 1.1's test (now green) and the full suite again — expect pre-change count + 1, all green.

**Tests:** (executable spec)
- `accept_registers_into_owning_server_registry_only` (from Task 1.1): same command; expected PASS after this task.
- Full suite: `cargo test -p camel-component-ws --lib` → all green, count = pre-change count + 1.
- `cargo check -p camel-test --tests` exit 0 (ws_security_test harness compiles with the new field).
- `cargo fmt --check --all` exit 0; `cargo clippy -p camel-component-ws --all-targets -- -D warnings` exit 0; `cargo xtask lint-unbounded-wait` does not exceed the 296 ceiling (no new unbounded waits introduced).

**Acceptance:**
- `ws_handler` contains no `global_registries()` reference (grep: only `finish_start` insert, `stop` remove, producer lookups, and the test helper remain).
- Each key's registry holds exactly its own server's connections in the Task 1.1 test; stop-isolation leg passes.
- Existing tests `consumer_cleanup_removes_registry_entry`, `injected_entry_survives_consumer_stop`, `consumer_injection_uses_provided_registry`, max-connections, broadcast, and targeted-send tests all green unchanged in behavior.

- [x] 1.2

### Task 1.3: CONTEXT.md vocabulary note

**Files:**
- `crates/components/camel-ws/CONTEXT.md` (modified)

**Steps:**
1. Extend the existing `WsConnectionRegistry` entry in the `## Language` section with the resolution rule: the accept path resolves the registry from the accepting server's owned per-path map on `WsAppState` (server-scoped); `GLOBAL_CONNECTION_REGISTRIES` remains the producer-facing full-triple `(host, port, path)` index keyed by consumer start/stop. One to three sentences, no new headings.

**Tests:** (executable spec)
- Documentation-only change: `cargo test -p camel-component-ws --lib` stays green (no code change); grep the file for the new sentence.
  - command: `grep -c "server's owned per-path map" crates/components/camel-ws/CONTEXT.md` → `1`.

**Acceptance:**
- CONTEXT.md `WsConnectionRegistry` entry states the server-scoped resolution rule and the producer-facing role of the global index.
- No other sections modified.

- [x] 1.3
