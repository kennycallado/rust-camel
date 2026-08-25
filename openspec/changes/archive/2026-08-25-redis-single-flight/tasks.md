# Tasks: redis-single-flight

## camel-redis (component)

### Task 1.1: Single-flight gate in `get_conn`

**Files:**
- `crates/components/camel-redis/src/executor.rs` (modified)

**Steps:**
1. Add private field `connect_gate: std::sync::Arc<tokio::sync::Mutex<()>>` to `struct MultiplexedExecutor`, initialized `Arc::new(tokio::sync::Mutex::new(()))` in `pub fn new`.
2. Restructure `get_conn` to (preserving the exact current connect body — response_timeout branch, `connection_timeout_secs` wrapper, byte-identical error messages):
   - Step A (fast path, unchanged): `let guard = self.conn.lock().await; if let Some(c) = guard.as_ref() { return Ok(c.clone()); }` — drop guard.
   - Step B (gate): `let _gate = self.connect_gate.lock().await;` — held to the end of the leader path.
   - Step C (double-check): lock `self.conn` again; if `Some(c)` (a leader stored while this caller waited), return `Ok(c.clone())`; drop guard.
   - Step D (leader, current body verbatim): `topology.resolve(ServerKind::Master)` → connect (async-block branch on `response_timeout` inside the `tokio::time::timeout(Duration::from_secs(timeout_secs), …)` wrapper with the identical two error mappings) → `let mut guard = self.conn.lock().await; *guard = Some(new_conn.clone()); Ok(new_conn)`.
3. Do NOT touch `refresh()` or `reconnect()` (their drop-then-get_conn flow now collapses through the gate).
4. Lock-ordering rule (add as a `//` comment near the field): the gate is always acquired BEFORE `conn` for the double-check and store sections; the fast path takes only `conn`. The reverse nesting (holding `conn` while acquiring the gate) never occurs.
5. Doc comment on `connect_gate`: single-flight guard collapsing concurrent cache-miss builds to one leader (storm elimination at failover); waiters double-check the cache after the gate; gate is cancellation-safe (dropped leader releases it).
6. Add a `#[cfg(test)]` accessor next to the existing `conn_arc`:
   `#[cfg(test)] pub(crate) fn gate_arc(&self) -> Arc<tokio::sync::Mutex<()>> { Arc::clone(&self.connect_gate) }` — same pattern as `conn_arc`, for sibling-module tests.

NOTE on a property this enables testing: whenever a leader holds the gate, the cache is empty BY CONSTRUCTION (a refresh leader dropped it, a cold-start leader never filled it) — so "fast path serves while the gate is held" cannot arise at runtime. The gate-free fast path is therefore proven by holding the gate EXTERNALLY in a unit test, not by racing a refresh leader.

**Tests:**
- `cached_fast_path_skips_gate` (new, `#[cfg(test)] mod tests`, `#[tokio::test]`): executor with a healthy cached connection built against a `#[cfg(test)]` inline answering stub — a minimal TcpListener local to this test that serves the FULL handshake: a persistent-buffer read loop parsing one complete RESP request frame at a time and answering EVERY frame with `+OK\r\n` (the driver pipelines multiple handshake frames — `CLIENT SETINFO` ×2 plus possibly `SELECT`/`AUTH` — a single-frame reply breaks the handshake); handshake completes, `get_conn()` returns and caches. Then: spawn a gate-holder task that acquires the gate via `gate_arc().lock().await`, sets an `Arc<AtomicBool>` `acquired = true` IMMEDIATELY after acquiring, and parks on a `tokio::sync::Notify`; the main task awaits `acquired == true` (yield_now polling) so the gate is PROVABLY held before the probe; then `get_conn()` from the main task inside `tokio::time::timeout(Duration::from_secs(5), …)` → MUST return `Ok` from the cache without waiting on the held gate (timeout = failure: fast path contended the gate); then notify + join the gate-holder. Command: `cargo test -p camel-component-redis --lib cached_fast_path_skips_gate`. Expected: guards the ordering fast-path-before-gate (compile-fails pre-change: `gate_arc` does not exist).
- Existing executor tests (`refresh_triggers_reresolution_and_failure_is_not_cached`, `multiplexed_executor_reconnect_reresolves`, `multiplexed_executor_lazy_connects_via_topology`, all others): pass unchanged → `cargo test -p camel-component-redis --lib` (expected: PASS — uncontended gate transparent).

**Acceptance:**
- `cargo test -p camel-component-redis --lib` exits 0 (all existing tests + the new one).
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` clean.
- `get_conn` connect body (resolve → connect wrapper → error messages → store) textually identical to pre-change except for the gate/double-check insertion.

- [x] 1.1

### Task 1.2: Shared handshake stub in `tests/common/mod.rs` with hold mode

**Files:**
- `crates/components/camel-redis/tests/common/mod.rs` (new)
- `crates/components/camel-redis/tests/response_timeout.rs` (modified — import from `common`, delete the local stub copy)

**Steps:**
1. Create `tests/common/mod.rs` (canonical Rust integration-test sharing: a module included by each test target via `mod common;`, NOT compiled as its own test target — no `#[test]` in it).
2. Move the RESP handshake stub (listener + per-connection frame parser + handshake-command set + silent mode) and `never_handshake_peer` from `tests/response_timeout.rs` into `common/mod.rs`.
3. Define ONE hold-gate API on the stub, used consistently by all tests:
   - `HandshakeStub::start_silent() -> (SocketAddr, HandshakeStubHandle)` — current behavior (handshake answered immediately, application commands consumed silently).
   - `HandshakeStub::start_silent_held() -> (SocketAddr, HandshakeStubHandle)` — identical, except handshake replies are withheld from the start.
   - `HandshakeStubHandle::hold(&self)` — latch: handshake replies for NEW connections are withheld from now on (connections already past their handshake are unaffected).
   - `HandshakeStubHandle::release(&self)` — one-shot unlock: sets the shared hold state back to `Free` (via `watch::send`) AND wakes all currently-withheld handshakes (`tokio::sync::Notify::notify_waiters()`). Both actions are required: connections that open AFTER release read `Free` and proceed without waiting; connections already parked on the notify are woken.
   - Implementation: the per-connection task holds a `tokio::sync::watch::Receiver<HoldState>` (`HoldState::Free | Held`) plus a shared `Arc<Notify>`; the handshake handler, when `Held`, registers with the Notify using tokio's `Notified::enable()` pattern (create the `Notified` future, call `enable()` to register interest, re-check the watch state, then poll the enabled future) — this avoids missed wakeups regardless of runtime flavor — before replying.
   - `HandshakeStub::start_rejecting_after_hold() -> (SocketAddr, HandshakeStubHandle)` — accepts connections and HOLDS their handshakes as above; on `release()`, closes every held connection without replying AND immediately closes (RST-free shutdown) every FUTURE accepted connection — every connect attempt fails after the hold. Used to make a failing leader deterministic (park, then fail on release) and to keep subsequent waiter attempts failing instantly.
   - The hold-gate items (`start_silent_held`, `hold`, `release`, `HoldState`, `start_rejecting_after_hold`) are used only by the `single_flight` test target (tasks 2.1/2.2), not by `response_timeout`; mark them `#[allow(dead_code)]` with a `// used by the single_flight test target` comment so each target compiles warning-free under `clippy --all-targets -D warnings`.
4. Rewrite `tests/response_timeout.rs` to `mod common;` + `use common::…` — ZERO behavioral change (same 4 tests, same assertions; the duplicated ~150 lines of parsing code are deleted).

**Tests:**
- `cargo test -p camel-component-redis --test response_timeout` — the same 4 tests pass against the shared stub (expected: PASS, unchanged behavior).
- The hold-gate API compiles and is exercised by task 2.1's tests (no additional test in this task).

**Acceptance:**
- `cargo test -p camel-component-redis --test response_timeout` exits 0 (4 passed).
- No RESP-parsing code remains in `tests/response_timeout.rs` (it only imports from `common`).
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings` and `cargo fmt --check` exit 0.

- [x] 1.2

## camel-redis (component), integration

### Task 2.1: Herd harness + collapse/cold-start/dropped-leader integration tests

**Files:**
- `crates/components/camel-redis/tests/single_flight.rs` (new)

**Steps:**
1. New test target using `mod common;` (handshake stub + hold gate) and the public surface (`MultiplexedExecutor::new`, `with_response_timeout`, `get_conn`, `refresh`, `RedisEndpointConfig::from_uri`, `topology_from_config`, the `RedisTopology` trait).
2. Counting topology wrapper (in this file): `struct CountingTopology { inner: Arc<dyn RedisTopology>, count: Arc<AtomicUsize> }` implementing `RedisTopology` by delegating every method to `inner`, incrementing `count` once per `resolve` call before delegating. Constructed as `Arc::new(CountingTopology { inner: Arc::new(topology_from_config(&config)?), count })` — the executor receives it from construction, so EVERY resolve counts (pre-build connects included).
3. All tests use `#[tokio::test(flavor = "current_thread")]` — cooperative interleaving: the leader parks in the held handshake, so every spawned task runs to its own park point before the main task signals release.
4. Herd helper `run_herd<F, Fut>(n: usize, stub: &HandshakeStubHandle, make_caller: F) -> Vec<...>`: spawn n tasks, each incrementing an `Arc<AtomicUsize>` started-counter as its FIRST statement and then performing the executor call; the main task polls `started == n` (loop { check; `tokio::task::yield_now().await; }), then `stub.release()`, then `join_all`s the spawned handles and returns their results. The whole helper runs inside `tokio::time::timeout(Duration::from_secs(30), …)` — panic on timeout (liveness guard, never a CI hang). Task 2.2's tests reuse this helper.

**Tests:** (exact names; arrange → act → assert. Tests that pre-build a connection RESET the counter (`count.store(0, Ordering::SeqCst)`) immediately before the herd phase, so the asserted number counts only the phase under test.)
- `concurrent_refresh_collapses_to_one_resolve`: `start_silent()` stub (unheld); build one healthy connection via `get_conn()` (real time); `count.store(0)`; `handle.hold()`; spawn n=5 refreshers through `run_herd`; all join → assert `count.load() == 1` (exactly one resolve for the whole herd phase) and all 5 results are `Ok` (the five `Ok` rebuild results already prove the herd was served — do NOT PING the returned connections: the silent stub never answers application commands by design). Pre-change: 5 concurrent refreshes each resolve → 5. Command: `cargo test -p camel-component-redis --test single_flight concurrent_refresh`. Expected: FAIL before task 1.1, PASS after.
- `cold_start_get_conn_collapses_to_one`: `start_silent_held()` stub; fresh executor (empty cache) with counting topology (no reset — no pre-build); n=5 `get_conn` callers through `run_herd` → `count == 1`, all `Ok`. Pre-change: 5. Command: `cargo test -p camel-component-redis --test single_flight cold_start`. Expected: FAIL pre-1.1, PASS after.
- `dropped_leader_releases_gate_and_waiter_proceeds`: `start_silent_held()` stub; fresh executor; spawn one leader (`get_conn` — resolve runs, then the connect parks in the held handshake; the leader holds the gate) with a started-counter; await started == 1; spawn one waiter (`get_conn`, parks on the gate) — await its started too; `leader_handle.abort()`; `release()`; join waiter → `Ok`, and `count == 2` (the aborted leader's resolve ALREADY counted — resolve precedes connect — plus the waiter's own leader resolve; the assertion proves the waiter did its own resolve, not inherit anything). Command: `cargo test -p camel-component-redis --test single_flight dropped_leader`. Expected: PASS after 1.1.

**Acceptance:**
- `cargo test -p camel-component-redis --test single_flight` exits 0 (3 passed so far; 2.2 adds more).
- Resolve-count assertions are exact per the numbers above, never `<=`.
- No wall-clock sleeps; liveness timeouts only.
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings`, `cargo fmt --check` exit 0.

- [x] 2.1

### Task 2.2: Straggler + no-inherit integration tests

**Files:**
- `crates/components/camel-redis/tests/single_flight.rs` (modified — appends to the 2.1 target)

**Steps:**
1. Reuse the 2.1 harness in the same file (CountingTopology, run_herd, hold-gate handles).
2. Add the two tests below (same `current_thread` flavor and liveness-timeout discipline).

**Tests:** (exact names; arrange → act → assert)
- `straggler_invalidation_forces_one_sequential_rebuild`: `start_silent()` stub; pre-build conn; `count.store(0)`; `hold()`; spawn ONE refresher (leader — parks in held handshake); await started == 1; `release()`; JOIN the leader (completes: resolve+connect+store — the straggler's drop lands after this store by construction); then spawn the straggler refresher (the released stub answers its handshake immediately — `release()` set the state back to `Free`); join → `Ok`; assert `count == 2` (leader + straggler: bounded, sequential, exactly one extra rebuild). Command: `cargo test -p camel-component-redis --test single_flight straggler`. Expected: PASS after 1.1.
- `waiter_does_not_inherit_leader_failure`: `start_rejecting_after_hold()` stub; fresh executor with counting topology; spawn 3 refreshers, each incrementing the started-counter before entering `refresh`; await started == 3 (all three are parked: the leader in the held handshake holding the gate, the two waiters on the gate); `release()` (the leader's connection closes without reply → leader fails and releases the gate; future connects are rejected instantly); `join_all` → all 3 results are `Err`; assert `count == 3` (each waiter ran its OWN resolve — a shared-future design would show 1) and the cache stays empty (probe via a 4th `get_conn` also failing, or assert via results only — integration tests cannot see `conn_arc`; asserting count == 3 plus all-Err suffices). Command: `cargo test -p camel-component-redis --test single_flight waiter_does_not_inherit`. Expected: FAIL against a hypothetical shared-future implementation (count 1); PASS after 1.1 (gate design).

**Acceptance:**
- `cargo test -p camel-component-redis --test single_flight` exits 0 (5 passed total with 2.1's).
- Resolve-count assertions exact (`== 2`, `== 3`).
- No wall-clock sleeps; liveness timeouts only.
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings`, `cargo fmt --check` exit 0.

- [x] 2.2

## Documentation

### Task 3.1: Document the single-flight contract in component CONTEXT.md

**Files:**
- `crates/components/camel-redis/CONTEXT.md` (modified)

**Steps:**
1. In the connection/executor section (near the `connection_timeout_secs` application sites list updated by change `redis-response-timeout`), add a short paragraph: `get_conn` collapses concurrent cache-miss builds through a single-flight gate (one leader resolving+connecting; waiters double-check into the stored connection; cached fast path gate-free; at most one resolve+connect in flight — straggler invalidation forces at most one extra sequential rebuild; leader failure never cached and never inherited — waiters attempt sequentially, trading persistent-outage tail latency for storm elimination). Reference `executor.rs` and the `redis-single-flight` change.
2. Touch nothing else in the file.

**Tests:**
- `rg -c "single-flight" crates/components/camel-redis/CONTEXT.md` ≥ 1 (docs check; no cargo invocation).

**Acceptance:**
- The paragraph exists and matches the blessed requirement's scoping (herd collapse + straggler bound + no-inherit failure semantics).
- `cargo xtask lint-context-citations` exits 0 (run from worktree root).

- [x] 3.1
