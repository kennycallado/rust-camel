# Tasks: redis-response-timeout

## camel-redis (component)

### Task 1.1: `MultiplexedExecutor` response-timeout builder and config-carrying build

**Files:**
- `crates/components/camel-redis/src/executor.rs` (modified)
- `crates/components/camel-redis/tests/pub_surface.rs` (modified)

**Steps:**
1. In `crates/components/camel-redis/src/executor.rs`, add field `response_timeout: Option<Duration>` to `struct MultiplexedExecutor` (private), initialized to `None` in `pub fn new`.
2. Add builder method on `impl MultiplexedExecutor`:
   `pub fn with_response_timeout(mut self, timeout: Duration) -> Self { self.response_timeout = Some(timeout); self }`
   Document: driver-level per-command deadline for multiplexed connections; `None` (default) keeps the driver's own default (500 ms in redis 1.6.0); does not affect `connection_timeout_secs` (TCP connect only). Cross-reference note in the doc comment: distinct from the repository service crate's `MultiplexedRepoExecutor::with_response_timeout` test seam (camel-redis-repo), which sizes that crate's LOCAL backstop — this builder sizes the DRIVER deadline.
3. In `get_conn`, branch on `self.response_timeout`:
   - `None`: keep the current `client.get_multiplexed_async_connection()` call unchanged.
   - `Some(t)`: call `client.get_multiplexed_async_connection_with_config(&redis::AsyncConnectionConfig::new().set_response_timeout(Some(t)).set_connection_timeout(None))`. The `set_connection_timeout(None)` disables the driver's parallel 1 s connect default so the component's existing `connection_timeout_secs` wrapper stays the sole connect bound (otherwise the driver's inner error eclipses the component's connect-timeout message at the same instant).
   Both branches stay inside the existing `tokio::time::timeout(Duration::from_secs(connection_timeout_secs), …)` connect wrapper; the timeout-error messages stay byte-identical.
4. Import `redis::AsyncConnectionConfig` (adjust the existing `use redis::…` group).
5. In `tests/pub_surface.rs`, add a compile-proof referencing the new builder:
   `let _: fn(MultiplexedExecutor, std::time::Duration) -> MultiplexedExecutor = MultiplexedExecutor::with_response_timeout;`

**Tests:**
- `with_response_timeout_is_pub`: pub_surface target compiles with the fn-pointer reference above → `cargo test -p camel-component-redis --test pub_surface` (expected: pass — surface regression guard).
- Existing executor unit tests (e.g. `multiplexed_executor_lazy_connects_via_topology`, `refresh_triggers_reresolution_and_failure_is_not_cached`) pass unchanged → `cargo test -p camel-component-redis --lib` (expected: pass — default path must be byte-identical).

**Acceptance:**
- `cargo test -p camel-component-redis --test pub_surface` exits 0.
- `cargo test -p camel-component-redis --lib` exits 0 (pre-existing tests untouched).
- `cargo clippy -p camel-component-redis -- -D warnings` exits 0.
- `cargo fmt --check` clean for the two files.

- [x] 1.1

### Task 1.2: Component paused-clock silent-peer tests for the driver deadline

**Files:**
- `crates/components/camel-redis/tests/response_timeout.rs` (new)

**Steps:**
1. Create the integration-test target `tests/response_timeout.rs`. It exercises the PUBLIC surface only: `MultiplexedExecutor::new`, `with_response_timeout`, `get_conn`, `refresh`, and `topology_from_config` (all `pub`).
2. Port the handshake-completing silent stub from the repository service crate (`crates/services/camel-redis-repo/src/executor.rs`, `FakeRedisServer::start_silent` + `StubConnection` + `is_handshake_command`): bind `127.0.0.1:0`, accept connections, parse one complete RESP request frame at a time from a persistent read buffer (the driver pipelines `CLIENT SETINFO` ×2 and possibly `SELECT`/`AUTH` in one TCP read — a per-request buffer would deadlock), answer handshake commands with `+OK\r\n`, and consume every later command frame without reply so a connected client's command await pends until its response deadline fires. A zero-byte silent peer is NOT usable for command-deadline tests: the driver completes `setup_connection` before returning the connection, so the connect would fail instead.
3. Add a second helper `never_handshake_peer() -> SocketAddr`: a `TcpListener` that accepts and never writes or reads (raw `std::future::pending()` park) — the handshake never completes.
4. Build the executor for the stub via the real path: `RedisEndpointConfig::from_uri(&format!("redis://{addr}/"))` (adjust host/port if `from_uri` fills them), `topology_from_config(&config)` for a standalone topology, `MultiplexedExecutor::new(config, topology).with_response_timeout(...)`. Keep `config.connection_timeout_secs` at its default unless a test says otherwise.
5. Write the four tests below. Tests 1-3 run under plain `#[tokio::test]`, complete `get_conn()`/`refresh()` against the stub FIRST (real time — `start_paused` with pending real-socket handshakes lets virtual time auto-advance spuriously past live timers), then call `tokio::time::pause()` and only then start the measured query. Test 4 uses `#[tokio::test(start_paused = true)]` (the peer never responds, so no real I/O ever completes; virtual time auto-advances to the 1 s connect timer deterministically). Every measured query records `let started = tokio::time::Instant::now();` immediately before it and asserts elapsed bounds, so a regression to the driver's 500 ms default cannot false-pass inside a 600 ms guard.

**Tests:** (arrange → act → assert)
- `configured_large_timeout_outlives_driver_default`: executor `.with_response_timeout(Duration::from_secs(10))` on the handshake-completing silent stub, `get_conn()` returns Ok (real time), `tokio::time::pause()` → `tokio::time::timeout(Duration::from_secs(1), conn.query_async::<_, ()>(redis::cmd("PING")))` → the 1 s guard fires (`Err(_elapsed)`) with elapsed ≈ 1 s virtual: the command was still Pending at the 1 s boundary (had the driver's 500 ms default governed, the command would have FAILED inside the guard at 500 ms). Pre-1.1 behavior: driver 500 ms default governing → guard variant fails. Command: `cargo test -p camel-component-redis --test response_timeout configured_large`. Expected: fails to compile before task 1.1 (builder absent); PASS after.
- `configured_small_timeout_fires_before_driver_default`: executor `.with_response_timeout(Duration::from_millis(100))`, `get_conn()` Ok (real time), `pause()` → `started = tokio::time::Instant::now()`, `timeout(Duration::from_millis(600), query…)` → the command completes in Err and `started.elapsed()` is ≥ 100 ms AND < 500 ms (the configured deadline fired, not the 500 ms driver default — a regression to the default lands exactly at 500 ms and fails the bound). Command: `cargo test -p camel-component-redis --test response_timeout configured_small`. Expected: fails to compile before task 1.1; PASS after.
- `refresh_rebuild_carries_configured_deadline`: executor `.with_response_timeout(Duration::from_millis(100))`, `get_conn()` Ok, `refresh()` Ok (both real time; query the connection RETURNED by `refresh()`), `pause()` → `started = Instant::now()`, `timeout(Duration::from_millis(600), query…)` → Err with `started.elapsed()` in [100 ms, 500 ms) — guards the rebuild path against silently dropping the config (default-governing lands at 500 ms). Command: `cargo test -p camel-component-redis --test response_timeout refresh_rebuild`. Expected: PASS after 1.1 (regression guard).
- `response_timeout_does_not_alter_connect_timeout`: `#[tokio::test(start_paused = true)]`; executor `.with_response_timeout(Duration::from_millis(100))` with `config.connection_timeout_secs = 1`, topology pointing at `never_handshake_peer()` → `get_conn()` → `Err(CamelError::ProcessorError(msg))` where `msg` contains `"timed out after 1s"` (the component's connect-timeout message — enabled on the `Some` branch by `set_connection_timeout(None)`; the driver's parallel 1 s connect default no longer eclipses it; virtual time jumps deterministically to the 1 s timer since no real I/O ever completes). Command: `cargo test -p camel-component-redis --test response_timeout connect_timeout`. Expected: PASS after 1.1 (fails if the `Some` branch omits `set_connection_timeout(None)` and keeps the driver default message path).

**Acceptance:**
- All four tests pass: `cargo test -p camel-component-redis --test response_timeout` exits 0.
- No wall-clock sleeps in the tests (`rg "sleep" tests/response_timeout.rs` returns nothing; only virtual-time guards).
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings` and `cargo fmt --check` exit 0.

- [x] 1.2

## camel-redis-repo (repository service crate)

### Task 2.1: Pass the margin value at construction and prove local backstop governs

**Files:**
- `crates/services/camel-redis-repo/src/connection.rs` (modified)
- `crates/services/camel-redis-repo/src/executor.rs` (modified — `#[cfg(test)]` seam only if needed for the second test)

**Steps:**
1. In `connection.rs`, add `const DRIVER_RESPONSE_TIMEOUT: Duration = Duration::from_secs(35);` with doc comment: strictly above the crate-local 30 s `DEFAULT_RESPONSE_TIMEOUT` (ADR-0063 Decision 13) so the local backstop always classifies first; 5 s margin; binding contract is strict ordering, the figure is an implementation constant.
2. In `connect_executor_with_topology`, change the executor construction from `MultiplexedExecutor::new(endpoint.clone(), topology)` to `MultiplexedExecutor::new(endpoint.clone(), topology).with_response_timeout(DRIVER_RESPONSE_TIMEOUT)` before wrapping in `MultiplexedRepoExecutor::new(...)`. `connect_executor` returns the concrete `MultiplexedRepoExecutor`, so tests can still apply the `#[cfg(test)]` seam `.with_response_timeout(short)` after construction.
3. Reuse `FakeRedisServer::start_silent()` (already imported in connection.rs tests via `crate::executor::FakeRedisServer`): the handshake completes (construction's eager `refresh()` succeeds), then application commands pend — exactly what both tests below need.

**Tests:** (arrange → act → assert)
- `production_local_backstop_governs_over_driver_deadline`: plain `#[tokio::test]`; build through the production `connect_executor(&silent_endpoint)` in REAL time (eager connect + handshake complete against the silent stub; construction succeeds), then `tokio::time::pause()` → `tokio::time::timeout(Duration::from_secs(40), executor.execute(cmd("PING")))` → Err is `CamelError::Io(msg)` where `msg` starts `"redis command response timed out after"` and elapsed ≈ 30 s virtual (the LOCAL backstop format fired first — the driver's 35 s deadline did not; virtual time jumps straight to 30 s under pause). Command: `cargo test -p camel-redis-repo --lib production_local_backstop`. Expected: FAIL before this task's step 2 (pre-fix driver default 500 ms fires first with a driver-format error), PASS after.
- `short_backstop_wins_under_driver_deadline`: plain `#[tokio::test]`; obtain the executor via production `connect_executor(&silent_endpoint)` (real time; concrete `MultiplexedRepoExecutor` return type), apply the `#[cfg(test)]` seam `.with_response_timeout(Duration::from_millis(150))` (local backstop), `pause()` → `execute(cmd("PING"))` → local-format error `"redis command response timed out after 150ms"`-style message (exact `Duration` debug render). Companion to the first test (it does NOT discriminate pre-fix — 150 ms also beats the old 500 ms driver default; the ordering proof against the old default rests on the first test alone). Command: `cargo test -p camel-redis-repo --lib short_backstop_wins`. Expected: PASS after step 2.

**Acceptance:**
- `cargo test -p camel-redis-repo --lib` exits 0 (all pre-existing tests included).
- `cargo clippy -p camel-redis-repo -- -D warnings` exits 0.
- `cargo fmt --check` clean for the touched files.

- [x] 2.1

## Documentation

### Task 3.1: Amend ADR-0063 Decision 13 "Effective bound today" paragraph

**Files:**
- `docs/adr/0063-redis-repository-service.md` (modified)

**Steps:**
1. Locate Decision 13's closing sentences beginning "Effective bound today: the driver default (500 ms in redis 1.6.0) fires before the 30-second backstop, so a slow-but-healthy peer can trip the driver deadline and cause refresh churn; plumbing `set_response_timeout` through the component's `get_conn` so the service crate's own value governs end to end is tracked as a follow-up."
2. Replace those sentences with the landed state: the follow-up landed (OpenSpec change `redis-response-timeout`, bd rc-dq7a) — `MultiplexedExecutor` now accepts `with_response_timeout(Duration)` applied in `get_conn` (initial connect and every `refresh`/`reconnect` rebuild); the repository service crate constructs its executor with a 35 s driver response timeout (30 s backstop + 5 s margin), so the service crate's own 30 s contract governs end to end and the driver deadline is defense-in-depth only; component consumers that do not call the builder keep the driver default.
3. Keep the amendment inside Decision 13's paragraph flow (no new Decision heading; no Status change — the ADR stays Accepted, amended in place as its own text already announced the follow-up).

**Tests:**
- `adr_amendment_rendered`: `rg -n "Effective bound today" docs/adr/0063-redis-repository-service.md` returns no match after the edit, and `rg -n "with_response_timeout" docs/adr/0063-redis-repository-service.md` returns ≥ 1 match → command `rg -c "with_response_timeout" docs/adr/0063-redis-repository-service.md` (expected: ≥ 1). Docs task; no cargo invocation.

**Acceptance:**
- The stale "Effective bound today" sentence is gone (`rg "Effective bound today" docs/adr/0063-redis-repository-service.md` exits 1).
- The paragraph states the 35 s margin and that the local 30 s contract governs.
- Prose follows project English/STE conventions; no other section of the ADR touched.

- [x] 3.1
