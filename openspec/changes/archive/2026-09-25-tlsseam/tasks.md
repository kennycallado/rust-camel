# Tasks: tlsseam

Single-phase change (no delivery-phase grouping). Tasks are strictly
ordered: each builds on the end state of the previous one, all inside
`crates/components/camel-ws/src/lib.rs` unless noted.

## camel-component-api

### Task 1: TlsReloadRegistry dual-handle global

**Files:**
- `crates/components/camel-component-api/src/tls_source.rs` (modified)

**Steps:**
1. Replace the body of `TlsReloadRegistry::global()`'s private static
   with a private backing accessor:
   `fn backing() -> &'static Arc<TlsReloadRegistry>` holding
   `static BACKING: OnceLock<Arc<TlsReloadRegistry>>`, initialized
   with `Arc::new(TlsReloadRegistry::default())`.
2. Rewrite `pub fn global() -> &'static TlsReloadRegistry` to
   `Self::backing().as_ref()` (the `&'static Arc` derefs to a
   `&'static Self` — signature and instance identity unchanged for
   all existing callers).
3. Add `pub fn global_arc() -> Arc<TlsReloadRegistry` returning
   `Self::backing().clone()`.
4. Keep the doc comment: state that both handles wrap the same
   allocation and that isolated test instances come from `Default`.

**Tests:**
- name: `global_and_global_arc_share_instance`
  setup: dual-handle accessors exist; `Default` is derived.
  action: `let arc = TlsReloadRegistry::global_arc();` then
  `assert!(std::ptr::eq(TlsReloadRegistry::global(), arc.as_ref()));`
  then register a freshly-authored dummy handler via `arc.register(...)`
  with a unique `(scheme, host, port)` triple. The dummy implements
  the two `TlsReloadHandler` methods (`matches` returning true only
  for that triple, and an async `reload` returning `Ok(())`) — no
  reusable dummy exists in this file's test module yet.
  assert: `TlsReloadRegistry::global().find(scheme, host, port)` is
  `Some`; then `global().unregister(scheme, host, port)` cleans up so
  sibling tests see no residue.
  command: `cargo test -p camel-component-api --lib tls_source`
  expected: fails before step 1-3 (`global_arc` does not exist), passes after.
- name: `default_instance_is_isolated_from_global`
  setup: global has a registered dummy (registered inside the test
  itself on the global handle, unique triple).
  action: `let fresh = TlsReloadRegistry::default();`
  assert: `fresh.find(...)` for the global's triple is `None`.
  command: `cargo test -p camel-component-api --lib tls_source`
  expected: passes after implementation.

**Acceptance:**
- `cargo test -p camel-component-api --lib` exits 0.
- `cargo clippy -p camel-component-api --all-features -- -D warnings` exits 0.
- `TlsReloadRegistry::global()` signature unchanged (rg confirms no
  caller outside this file needed edits).

- [x] 1.1

## camel-component-ws — seam

### Task 2: ServerRegistry dual-handle, owned TLS registry, instance lifetimes

**Files:**
- `crates/components/camel-ws/src/lib.rs` (modified)

**Steps:**
1. Add field `tls: Arc<camel_component_api::tls_source::TlsReloadRegistry>`
   to `struct ServerRegistry`.
2. Convert the global static to the dual-handle pattern (mirroring
   Task 1): private `fn backing() -> &'static Arc<Self>` with
   `static BACKING: OnceLock<Arc<ServerRegistry>>`, initialized with
   `inner: Mutex::new(HashMap::new()), staged:
   Mutex::new(HashMap::new()), tls: TlsReloadRegistry::global_arc()`;
   `pub fn global() -> &'static Self` becomes `Self::backing().as_ref()`;
   add `pub fn global_arc() -> Arc<Self>` cloning the backing.
3. Add `pub fn new() -> Self` constructing an isolated pair: empty
   `inner`, empty `staged`, `tls: Arc::new(TlsReloadRegistry::default())`.
   Add `impl Default for ServerRegistry` delegating to `new()` (satisfies
   clippy `new_without_default`).
4. Drop the `&'static self` bounds: `stage_listener`, `get_or_spawn`,
   `get_or_spawn_with_listener`, `ref_count_for_test`,
   `bound_addr_for_test` take `&self`. The two test accessors'
   BODIES currently read `Self::global().inner` — rewrite them to read
   `self.inner` so an isolated instance reports its own map.
5. Convert `pub fn reset()` to `pub fn reset(&self)` (keep
   `#[cfg(test)]`), operating on `self.inner`/`self.staged` only.
   Mechanically update its 27 test call sites in this file to
   `ServerRegistry::global().reset()` — the migration tasks move them
   to isolated instances later.
6. Inside `get_or_spawn` and `get_or_spawn_with_listener`, replace
   `camel_component_api::tls_source::TlsReloadRegistry::global().register(handler)`
   with registration into the owning instance: clone
   `let tls_registry = self.tls.clone();` before the
   `cell.get_or_try_init` closure and call
   `tls_registry.register(handler)` inside it.
7. Add `pub fn tls_registry(&self) -> &TlsReloadRegistry` accessor
   (return `self.tls.as_ref()`).

**Tests:**
- name: `server_registry_handles_share_instance`
  setup: dual-handle exists.
  action: `assert!(std::ptr::eq(ServerRegistry::global(),
  ServerRegistry::global_arc().as_ref()));`
  assert: pointer equality holds.
  command: `cargo test -p camel-component-ws --lib`
  expected: fails before, passes after.
- name: `isolated_server_registry_is_fresh`
  setup: `ServerRegistry::new()` exists.
  action: `let reg = ServerRegistry::new();`
  assert: `reg.ref_count_for_test(1) == 0`,
  `reg.bound_addr_for_test(1).is_none()`,
  `reg.tls_registry().find("wss", "127.0.0.1", 1).is_none()`, and
  `ServerRegistry::global().ref_count_for_test(1) == 0`.
  command: `cargo test -p camel-component-ws --lib`
  expected: passes after implementation.
- name: `isolated_reset_scopes_to_own_instance`
  setup: two isolated registries, each with one live server spawned
  via `get_or_spawn` on distinct ephemeral ports (bind two listeners,
  take their ports; spawn plain-ws servers).
  action: `reg_a.reset();`
  assert: `reg_a.bound_addr_for_test(port_a).is_none()` AND
  `reg_b.bound_addr_for_test(port_b).is_some()` (reg_b's server
  survived).
  command: `cargo test -p camel-component-ws --lib`
  expected: passes after implementation.
- name: `global_spawn_registers_tls_in_global_registry`
  setup: holds the `REGISTRY_TEST_LOCK` guard via `acquire_deadline`
  for the whole test (it mutates process globals while unmigrated
  lock-holders still exist); a wss server spawned through
  `ServerRegistry::global()` (staged-listener pattern on an ephemeral
  port).
  action: query `ServerRegistry::global().tls_registry().find("wss", "", port)`.
  assert: handler found (production reload flow preserved — the
  runtime bus reads `TlsReloadRegistry::global()` and both handles
  wrap the same allocation).
  cleanup: `ServerRegistry::global().release(port)` is a no-op (the
  server and entry are process-lifetime) and `reset()` aborts servers
  but does NOT unregister TLS handlers — so under the guard call
  `reset()` AND explicitly
  `TlsReloadRegistry::global().unregister("wss", "", port)` so no
  server, entry, or handler leaks to sibling tests.
  command: `cargo test -p camel-component-ws --lib`
  expected: passes after implementation.

**Acceptance:**
- `cargo test -p camel-component-ws --lib` exits 0 (all 160 tests).
- `cargo clippy -p camel-component-ws --all-targets -- -D warnings` exits 0.
- `rg "&'static self" crates/components/camel-ws/src/lib.rs` returns no hits.

- [x] 2.1

### Task 3: WsConsumer constructor injection

**Files:**
- `crates/components/camel-ws/src/lib.rs` (modified)

**Steps:**
1. Add field `server_registry: Arc<ServerRegistry>` to `WsConsumer`.
2. Add `pub fn with_server_registry(cfg: WsServerConfig, runtime:
   Arc<dyn RuntimeObservability>, server_registry: Arc<ServerRegistry>)
   -> Self` mirroring the existing `WsConsumer::new` body but storing
   the provided registry (match the exact existing `new` signature
   types for cfg/runtime).
3. Rewrite `pub fn new(...)` to delegate:
   `Self::with_server_registry(cfg, runtime, ServerRegistry::global_arc())`.
4. Replace the three production `ServerRegistry::global()` uses in
   `start` (`get_or_spawn`), `start_with_listener`
   (`get_or_spawn_with_listener`), and `stop` (`release`) with
   `self.server_registry`.
5. Leave `WsEndpoint::create_consumer` untouched — it calls `new`,
   which now flows through `global_arc()` (same instance).

**Tests:**
- name: `consumer_injection_uses_provided_registry`
  setup: pre-bound ephemeral listener (ADR-0070 staged pattern),
  `let reg = Arc::new(ServerRegistry::new());`, consumer built with
  `WsConsumer::with_server_registry(cfg, test_rt(), reg.clone())`.
  action: `consumer.start_with_listener(ctx, listener).await`.
  assert: `reg.ref_count_for_test(port) == 1` while
  `ServerRegistry::global().ref_count_for_test(port) == 0`; after
  `consumer.stop().await`, `reg.bound_addr_for_test(port).is_some()`
  (process-lifetime entry kept, mirroring the cleanup-contract test).
  command: `cargo test -p camel-component-ws --lib`
  expected: fails before (constructor absent), passes after.
- name: `consumer_default_keeps_global_registry`
  setup: staged ephemeral listener; consumer via existing
  `WsConsumer::new(cfg, test_rt())`; holds `REGISTRY_TEST_LOCK` while
  global state is touched (lock still exists at this task).
  action: start via `start_with_listener`, then
  `ServerRegistry::global().reset()` after stop for cleanup.
  assert: `ServerRegistry::global().ref_count_for_test(port) == 1`
  after start (production default identity preserved).
  command: `cargo test -p camel-component-ws --lib`
  expected: passes after implementation.

**Acceptance:**
- `cargo test -p camel-component-ws --lib` exits 0.
- `cargo clippy -p camel-component-ws --all-targets -- -D warnings` exits 0.

- [x] 3.1

## camel-component-ws — test migration

### Task 4: Migrate TLS-reload tests to isolated registries

**Files:**
- `crates/components/camel-ws/src/lib.rs` (modified — test module only)

**Steps:**
1. In `wss_release_unregisters_tls_reload_handler`,
   `wss_multiple_refs_release_does_not_unregister`,
   `ws_plaintext_does_not_register_tls_reload_handler`, and the
   TLS-reload tests in the 5430-5600 region that assert on
   `TlsReloadRegistry::global().find(...)`: construct
   `let reg = Arc::new(ServerRegistry::new());`, build the consumer
   via `WsConsumer::with_server_registry(cfg, test_rt(), reg.clone())`,
   and replace every `TlsReloadRegistry::global().find("wss", "", port)`
   with `reg.tls_registry().find("wss", "", port)`.
2. Replace direct `ServerRegistry::global().get_or_spawn*` /
   `.release(port)` uses in those tests with `reg.get_or_spawn*` /
   `reg.release(port)`.
3. Delete the `acquire_deadline(&REGISTRY_TEST_LOCK, ...)` guard lines
   from these tests (they no longer touch process globals).

**Tests:**
- name: `wss_release_unregisters_tls_reload_handler` (modified; note:
  despite the historical name, release is a no-op — the assertion is
  that the handler REMAINS registered)
  setup: isolated `reg`; wss consumer injected with `reg`; server
  started on an ephemeral port.
  action: original sequence — verify handler found and functional
  (`reload()` succeeds), then `reg.release(port)`.
  assert: `reg.tls_registry().find("wss", "", port)` is STILL `Some`
  after release (process-lifetime semantics, exactly as the current
  test asserts on the global); additionally
  `TlsReloadRegistry::global().find("wss", "", port)` is `None`
  throughout (isolated visibility).
  command: `cargo test -p camel-component-ws --lib wss`
  expected: passes after migration.
- name: `ws_plaintext_does_not_register_tls_reload_handler` (modified)
  setup/assert: same shape — no handler in `reg.tls_registry()` nor in
  the global for a plain-ws server.
  command: `cargo test -p camel-component-ws --lib plaintext`
  expected: passes after migration.

**Acceptance:**
- `cargo test -p camel-component-ws --lib` exits 0.
- `rg -n 'TlsReloadRegistry::global\(\)' crates/components/camel-ws/src/lib.rs`
  shows hits ONLY inside test assertions that verify isolation
  (negative-visibility `find(...).is_none()` checks); no production
  registration or lookup path routes through the process global
  anymore.

- [x] 4.1

### Task 5: Migrate registry-mechanics and reset tests to isolated instances

**Files:**
- `crates/components/camel-ws/src/lib.rs` (modified — test module only)

**Steps:**
1. For each test that calls `ServerRegistry::reset()` (25 sites) or
   drives `ServerRegistry::global()` directly for spawn/ref-count/
   eviction/staging mechanics (the 3063-3670 region and the 4690-5090
   consumer-shared-server tests): introduce
   `let reg = ServerRegistry::new();` (or `Arc::new(ServerRegistry::new())`
   where an injected consumer needs a clone) and route all registry
   calls through it.
2. Shared-server tests that spawn two consumers on one port must
   inject the SAME `reg` into both consumers via
   `with_server_registry` and keep asserting same-`WsAppState`
   semantics.
3. `ServerRegistry::global().reset()` calls from Task 2's mechanical
   fix become `reg.reset()`.
4. Delete `acquire_deadline(&REGISTRY_TEST_LOCK, ...)` guard lines
   from every migrated test.
5. Tests in this region that pin the ADR-0070 staged-listener contract
   keep their semantics: staging happens on the isolated instance
   (`reg.stage_listener(...)`).

**Tests:**
- name: `injected_entry_survives_consumer_stop` (existing test,
  migrated shape)
  setup: one isolated `reg`; consumers A and B (`consumer_a`,
  `consumer_b` in the test body) both injected with clones of it on
  the same ephemeral port.
  action: run the original sequence (start both, stop one).
  assert: original assertions kept (shared entry survives the stop);
  `reg.ref_count_for_test(port)` matches the original expectation;
  `ServerRegistry::global().ref_count_for_test(port) == 0`.
  command: `cargo test -p camel-component-ws --lib`
  expected: passes after migration.
- name: `reset_clears_injected_entry_allowing_rebind` (existing test,
  migrated shape)
  setup: isolated `reg` hosting a server on an ephemeral port.
  action: `reg.reset();` then rebind the same port.
  assert: rebound entry exists in `reg`; other tests' servers
  unaffected (they hold their own registries).
  command: `cargo test -p camel-component-ws --lib reset`
  expected: passes after migration.

**Acceptance:**
- `cargo test -p camel-component-ws --lib` exits 0.
- `rg -n 'ServerRegistry::global\(\)' crates/components/camel-ws/src/lib.rs`
  shows hits ONLY in: the two global-semantics tests
  (`consumer_default_keeps_global_registry`,
  `global_spawn_registers_tls_in_global_registry`), pointer-identity
  assertions (`server_registry_handles_share_instance`), and
  negative-isolation assertions of migrated tests
  (e.g. `ServerRegistry::global().ref_count_for_test(port) == 0`);
  no production path routes through `global()` (production defaults
  go via `global_arc()`).

- [x] 5.1

### Task 6: Finish migration — delete REGISTRY_TEST_LOCK

**Files:**
- `crates/components/camel-ws/src/lib.rs` (modified — test module only)

**Steps:**
1. Migrate every remaining test that acquires
   `acquire_deadline(&REGISTRY_TEST_LOCK, ...)`: server-hosting
   consumer tests switch to `WsConsumer::with_server_registry` with an
   isolated `reg`; direct-registry tests use their own instance.
   TWO tests keep global access by design —
   `consumer_default_keeps_global_registry` (Task 3) and
   `global_spawn_registers_tls_in_global_registry` (Task 2): convert
   their guards to a shared dedicated `GLOBAL_DEFAULT_TEST_LOCK`
   static so the name says what it protects, and keep their cleanup
   (release/reset under the guard) intact.
2. Delete `static REGISTRY_TEST_LOCK` and the comment block above it.
3. Remove the now-unused `acquire_deadline` / `TEST_LOCK_DEADLINE`
   imports if no camel-ws site remains; otherwise keep them for the
   remaining sites only.
4. Run the full suite and confirm no test depends on execution order
   (two consecutive full runs both green).

**Tests:**
- name: suite-level (no single fn)
  setup: all migrations applied.
  action: `cargo test -p camel-component-ws --lib` twice in a row.
  assert: 160+ tests pass both times;
  `rg -c 'REGISTRY_TEST_LOCK' crates/components/camel-ws/src/lib.rs`
  returns 0.
  command: `cargo test -p camel-component-ws --lib`
  expected: passes after migration.

**Acceptance:**
- `cargo test -p camel-component-ws --lib` exits 0 twice consecutively.
- `rg 'REGISTRY_TEST_LOCK' crates/components/camel-ws/src/lib.rs` returns no hits.
- `cargo clippy -p camel-component-ws --all-targets -- -D warnings` exits 0.

- [x] 6.1

## docs + measurement

### Task 7: Docs, measurements, gate sweep

**Files:**
- `crates/components/camel-component-api/CONTEXT.md` (modified)
- `crates/components/camel-ws/CONTEXT.md` (modified)
- `docs/src/concepts/glossary.md` (modified)

**Steps:**
1. camel-component-api/CONTEXT.md: document the dual-handle contract
   (`global()` -> `&'static`, `global_arc()` -> same-instance `Arc`,
   isolated instances via `Default` for test seams).
2. camel-ws/CONTEXT.md: add a "Test seam" note under the
   ServerRegistry section — `ServerRegistry::new()` isolated pair,
   `WsConsumer::with_server_registry` injection, instance-scoped
   `reset()`, `tls_registry()` accessor; production paths unchanged.
3. glossary.md (line ~203): extend the TLS reload registry entry —
   handlers register into the registry instance the server owns; the
   process global remains the production instance; tests may inject
   isolated pairs.
4. Measurement (record BEFORE numbers from this mission log: quiet
   test-phase 0.34s, taskset -c 0 test-phase 1.75s, 160 tests): run
   AFTER numbers on the migrated suite —
   `cargo test -p camel-component-ws --lib` (quiet) and
   `taskset -c 0 cargo test -p camel-component-ws --lib` — and note
   both pairs in bd rc-fx3xy via `bd note rc-fx3xy "..."` run from
   /home/kenny/dev/rust-camel.

**Tests:**
- name: docs-gates
  setup: all code tasks complete.
  action: run the gate commands below.
  assert: every command exits 0.
  command: `cargo fmt --check --all` ; the 4 clippy legs from
  AGENTS.md QUALITY GATES ; `cargo test -p camel-component-ws --lib` ;
  `cargo test -p camel-core --lib` ;
  `cargo test -p camel-core --test tls_reload_test` ;
  `cargo xtask lint-unbounded-wait` ; `cargo xtask lint-cancel-tokens` ;
  `cargo xtask schema --check` ;
  `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p camel-core -p
  camel-builder -p camel-dsl -p camel-endpoint -p
  camel-component-api -p camel-component-ws --no-deps`.
  expected: all green after implementation.

**Acceptance:**
- Every gate command above exits 0.
- `bd show rc-fx3xy --json` shows the measurement note.
- Glossary + both CONTEXT.md files updated (English, STE-plain).

- [x] 7.1
