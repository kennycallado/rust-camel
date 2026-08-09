# Tasks: audit-fix-async-lifecycle

## camel-health

### Task 1.1: HealthServer abort on shutdown timeout + derive shutdown_timeout from handler_timeout

**Files:**
- `crates/camel-health/src/server.rs` (modified)

**Steps:**
1. Remove the `SHUTDOWN_TIMEOUT` const (line 16). Replace with a computed
   value at call time in `stop()`.
2. In `stop()` (lines 133-151), compute
   `let shutdown_timeout = self.handler_timeout + Duration::from_secs(2);`
   at the top of the method, before the `if let Some(handle)` block.
3. Replace the `match tokio::time::timeout(SHUTDOWN_TIMEOUT, handle).await`
   block with three arms matching `Result<Result<(), JoinError>, Elapsed>`:
   - `Ok(Ok(())) => {}` — clean join.
   - `Ok(Err(join_err)) => { tracing::error!("Health server task failed during shutdown: {}", join_err); }`
     — JoinError (panic or cancel), log at `error!`.
   - `Err(_) => { tracing::warn!("Health server did not shut down within {:?}, aborting", shutdown_timeout); handle.abort(); let _ = handle.await; }`
     — timeout, abort the handle to prevent rebind race, await the abort.
4. Remove the now-unused `use tokio::task::JoinHandle;` import only if it
   becomes unused after the edit (it likely stays used by the struct field).

**Tests:** (executable spec)
- `test_stop_aborts_on_timeout`: Start a server with `set_handler_timeout(Duration::from_secs(10))`. Call `stop()`. Assert `stop()` returns within `~12s` (handler_timeout + 2s margin + small buffer). Assert `server.status() == ServiceStatus::Stopped`. Verify the port is releasable by starting a new server on the same port immediately after.
  - **command:** `cargo test -p camel-health --lib test_stop_aborts_on_timeout`
  - **expected:** passes after implementation (fails or hangs before — the old code never aborts)
- `test_shutdown_timeout_derives_from_handler_timeout`: Start a server with `set_handler_timeout(Duration::from_millis(100))`. Call `stop()`. Assert `stop()` returns `Ok(())` and completes promptly (well under 5s, since the derived shutdown timeout is now 100ms + 2s). Assert `server.status() == ServiceStatus::Stopped`.
  - **command:** `cargo test -p camel-health --lib test_shutdown_timeout_derives_from_handler_timeout`
  - **expected:** passes after implementation
- `test_stop_logs_panic_on_join_error`: Start a server, then cause the spawned task to panic during shutdown (inject a panic by dropping the listener or using a test-specific shutdown path that panics). Call `stop()`. Assert `stop()` returns `Ok(())` (panic is logged at `error!`, not propagated). Assert `server.status() == ServiceStatus::Stopped`. This exercises the `Ok(Err(join_err))` arm (was silently swallowed via `Ok(_) => {}`).
  - **command:** `cargo test -p camel-health --lib test_stop_logs_panic_on_join_error`
  - **expected:** passes after implementation

**Acceptance:**
- `cargo test -p camel-health --lib` passes (all existing + new tests).
- `cargo clippy -p camel-health -- -D warnings` exits 0.
- `SHUTDOWN_TIMEOUT` const is removed (no orphan const left).
- The `stop()` method handles `Ok(Ok(()))`, `Ok(Err(JoinError))`, `Err(Elapsed)` — three arms, not four.

- [x] 1.1

## camel-function

### Task 2.1: FunctionRuntimeService::stop drains all providers on partial failure

**Files:**
- `crates/services/camel-function/src/service.rs` (modified)
- `crates/services/camel-function/src/provider/mod.rs` (modified — add `fail_on_shutdown` to `FakeProvider`)

**Steps:**
1. In `service.rs` `stop()` (lines 298-308), replace the `?`-short-circuit loop:
   ```rust
   // OLD:
   for handle in handles {
       handle.cancel.cancel();
       self.provider.shutdown(handle).await.map_err(|e| CamelError::ProcessorError(e.to_string()))?;
   }
   ```
   with a best-effort drain:
   ```rust
   let mut first_err: Option<ProviderError> = None;
   for handle in handles {
       handle.cancel.cancel();
       if let Err(e) = self.provider.shutdown(handle).await {
           if first_err.is_none() {
               first_err = Some(e);
           }
       }
   }
   self.invoker.started.store(false, Ordering::SeqCst);
   self.status.store(STATUS_STOPPED, Ordering::SeqCst);
   if let Some(e) = first_err {
       return Err(CamelError::ProcessorError(e.to_string()));
   }
   Ok(())
   ```
2. In `provider/mod.rs`, add a `fail_on_shutdown: bool` field to the
   **`FakeProviderConfig`** struct (around line 65, alongside existing
   `fail_on_spawn`/`fail_on_health` fields). Default it to `false` in the
   `Default` impl. In the `FakeProvider::shutdown` method (around line 132),
   check `self.config.lock().expect("config").fail_on_shutdown`: if `true`,
   push to `shutdowns` list (so test can verify drain order) then return
   `Err(ProviderError::Internal("configured shutdown failure".into()))`.
   This lets tests verify partial failure drains all providers.
3. Update all existing `FakeProviderConfig { .. }` construction sites in
   tests to include the new field (set `fail_on_shutdown: false` by default,
   or use `..Default::default()` if already present).

**Tests:** (executable spec)
- `stop_drains_all_providers_on_partial_failure`: Create a `FunctionRuntimeService` with a `FakeProvider` that has `fail_on_shutdown: true` (all shutdown calls return `Err`). Start the service with three runners, then call `stop()`. Assert `stop()` returns `Err(CamelError::ProcessorError(_))`. Assert all three runner keys appear in `FakeProvider::shutdowns` (verifying the drain continued past the first error and visited every handle). Assert `service.status() == ServiceStatus::Stopped` and `started.load() == false`.
  - **command:** `cargo test -p camel-function --lib stop_drains_all_providers_on_partial_failure`
  - **expected:** passes after implementation (before fix, only one handle is shut down)

**Acceptance:**
- `cargo test -p camel-function --lib` passes (all existing + new tests).
- `cargo clippy -p camel-function -- -D warnings` exits 0.
- `stop()` sets `STATUS_STOPPED` and `started=false` unconditionally before returning the error.

- [x] 2.1

## camel-component-jms

### Task 3.1: LazyJmsProducer::poll_ready returns ConsumerStopping on BridgeState::Stopped

**Files:**
- `crates/components/camel-jms/src/component.rs` (modified)

**Steps:**
1. In `LazyJmsProducer::poll_ready` (lines 834-838), change the
   `BridgeState::Stopped` arm from:
   ```rust
   return Poll::Ready(Err(CamelError::ProcessorError(format!(
       "JMS broker '{}' is stopped",
       self.broker_name
   ))));
   ```
   to:
   ```rust
   return Poll::Ready(Err(CamelError::ConsumerStopping));
   ```
   `ConsumerStopping` is a unit variant (no payload), matching ADR-0024 §Decision
   and the inner `JmsProducer` precedent (producer.rs:143).
   NOTE: The `call()` method's `BridgeState::Stopped` arm is
   intentionally left as `ProcessorError` — it is a separate code path with
   different semantics (the exchange is already in-flight when `call` runs,
   so a Stopped bridge during `call` is an operational error, not a
   readiness signal). Only `poll_ready` signals shutdown readiness.

**Tests:** (executable spec)
- `lazy_producer_poll_ready_returns_consumer_stopping_on_stopped`: Create a `JmsBridgePool` with a slot in `BridgeState::Stopped` (pattern from existing test at line 1598: `let (state_tx, state_rx) = watch::channel(BridgeState::Stopped);`). Insert the slot into the pool's `slots` map. Create a `LazyJmsProducer` referencing this pool. Call `poll_ready()` via `tower::Service::poll_ready`. Assert the result is `Poll::Ready(Err(CamelError::ConsumerStopping))` — NOT `ProcessorError`.
  - **command:** `cargo test -p camel-component-jms --lib lazy_producer_poll_ready_returns_consumer_stopping_on_stopped`
  - **expected:** passes after implementation (before fix, returns `ProcessorError`)

**Acceptance:**
- `cargo test -p camel-component-jms --lib` passes (all existing + new tests).
- `cargo clippy -p camel-component-kafka -p camel-component-jms -- -D warnings` exits 0 (JMS uses the kafka clippy gate per AGENTS.md).
- The `BridgeState::Stopped` arm in `poll_ready` returns `CamelError::ConsumerStopping` (unit variant, no format string).

- [x] 3.1

## camel-component-master

### Task 4.1: stop_delegate drains epoch-bridge on all delegate-outcome paths

**Files:**
- `crates/components/camel-master/src/leadership.rs` (modified)

**Steps:**
1. In `stop_delegate` (lines 84-137), restructure so the bridge drain block
   (lines 120-135) always runs. Replace the three early-return arms
   (`:98`, `:102-104`, `:108-111`) with outcome-capture:
   ```rust
   let delegate_result: Result<(), CamelError> = match timeout(drain_timeout, &mut handle).await {
       Ok(Ok(Ok(()))) => Ok(()),
       Ok(Ok(Err(err))) => Err(err),
       Ok(Err(e)) if e.is_panic() => {
           error!(error = %e, "master delegate task panicked");
           Err(CamelError::ProcessorError(format!("master delegate task panicked: {e}")))
       }
       Ok(Err(e)) => {
           warn!(error = %e, "master delegate task cancelled");
           Err(CamelError::ProcessorError(format!("master delegate task cancelled: {e}")))
       }
       Err(_) => {
           warn!("master delegate shutdown timed out, aborting");
           handle.abort();
           Ok(())
       }
   };
   ```
2. The epoch-bridge drain block (already at lines 122-134) stays as-is and
   runs unconditionally after the match.
3. After the bridge drain block, return `delegate_result` (the `?`
   propagation happens after the drain).

**Tests:** (executable spec)
- `stop_delegate_drains_bridge_on_delegate_error`: Construct a `DelegateState::Active` with a delegate task that returns `Err(CamelError::ProcessorError("test delegate failure".into()))` and an epoch-bridge with buffered envelopes. Call `stop_delegate`. Assert: (a) the returned result is `Err` matching the delegate error, (b) the bridge handle has exited (no detached bridge — `bridge_handle.is_finished()` is true after `stop_delegate` returns).
  - **command:** `cargo test -p camel-component-master --lib stop_delegate_drains_bridge_on_delegate_error`
  - **expected:** passes after implementation (before fix, bridge is not drained on delegate error)
- `stop_delegate_drains_bridge_on_delegate_timeout`: Construct a `DelegateState::Active` with a delegate task that never completes (pending forever) and a `drain_timeout` of 100ms. Call `stop_delegate`. Assert: (a) the returned result is `Ok(())` (timeout path), (b) the bridge handle has exited.
  - **command:** `cargo test -p camel-component-master --lib stop_delegate_drains_bridge_on_delegate_timeout`
  - **expected:** passes after implementation

**Acceptance:**
- `cargo test -p camel-component-master --lib` passes (all existing + new tests).
- `cargo clippy -p camel-component-master -- -D warnings` exits 0.
- No early-return between the delegate outcome match and the bridge drain block.

- [x] 4.1

## camel-auth

### Task 5.1: CachingTokenIntrospector per-key dedup (remove head-of-line blocking)

**Files:**
- `crates/services/camel-auth/src/introspection.rs` (modified)

**Steps:**
1. Change the `in_flight` field type from `Mutex<()>` to
   `Mutex<HashMap<String, Arc<Mutex<()>>>>`:
   ```rust
   in_flight: Mutex<HashMap<String, Arc<Mutex<()>>>>,
   ```
2. Update `with_client` to initialize `in_flight: Mutex::new(HashMap::new())`.
3. In `introspect` (around line 216), replace the single `let _guard = self.in_flight.lock().await;`
   with per-key dedup. The guard is scoped inside an async block so it
   drops before cleanup, and the `key_mutex` Arc is dropped explicitly
   before the strong_count check:
   ```rust
   // Get-or-insert per-key mutex
   let key_mutex = {
       let mut in_flight_map = self.in_flight.lock().await;
       in_flight_map
           .entry(key.clone())
           .or_insert_with(|| Arc::new(Mutex::new(())))
           .clone()
   };
   // Do the work inside a block that catches all ? early returns.
   // The _guard drops at the end of the block regardless of outcome.
   let result: Result<IntrospectionResult, AuthError> = async {
       let _guard = key_mutex.lock().await;
       // double-check cache (hit-after-wait)
       // HTTP call + cache insert (same as existing code)
   }.await;
   ```
4. After the async block (and before `return result;`), drop `key_mutex`
   and run cleanup. This runs on BOTH success and error paths because the
   async block catches `?` early returns:
   ```rust
   drop(key_mutex);
   {
       let mut in_flight_map = self.in_flight.lock().await;
       if let Some(arc) = in_flight_map.get(&key) {
           if Arc::strong_count(arc) == 1 {
               in_flight_map.remove(&key);
           }
       }
   }
   result
   ```
   After `drop(key_mutex)`, the only strong ref is the one in the map
   (strong_count == 1), so the entry is removed. This cleanup runs on
   every cache miss, decoupled from the capacity-gated `evict_if_needed()`.
   The outer mutex serializes test-and-remove with get-or-insert (step 3),
   preventing races.

**Tests:** (executable spec)
- `concurrent_different_tokens_no_head_of_line_blocking`: Create a `CachingTokenIntrospector` with a `MockServer` where each POST response has a 500ms delay (use `wiremock::ResponseTemplate::new(200).set_delay(Duration::from_millis(500))`). Introspect two different tokens concurrently (`tokio::join!`). Assert the total wall time is < 800ms (parallel, not serial — serial would be ~1000ms). Assert both results are `Ok` with `active: true`. Assert the mock server received 2 requests.
  - **command:** `cargo test -p camel-auth --lib concurrent_different_tokens_no_head_of_line_blocking`
  - **expected:** passes after implementation (before fix, takes ~1000ms due to serialization)
- `concurrent_same_token_dedup_preserved`: Create a `CachingTokenIntrospector` with a `MockServer` that counts POST requests. Introspect the same token concurrently (`tokio::join!`). Assert only 1 HTTP request was made (thundering-herd prevention preserved). Assert both callers receive `Ok`.
  - **command:** `cargo test -p camel-auth --lib concurrent_same_token_dedup_preserved`
  - **expected:** passes after implementation

**Acceptance:**
- `cargo test -p camel-auth --lib` passes (all existing + new tests).
- `cargo clippy -p security-wasm-policy -- -D warnings` is NOT affected; run `cargo clippy -p camel-auth -- -D warnings` instead.
- The `in_flight` field is no longer `Mutex<()>` — it is `Mutex<HashMap<String, Arc<Mutex<()>>>>`.

- [x] 5.1

### Task 5.2: CachingPermissionEvaluator per-key dedup (remove head-of-line blocking)

**Files:**
- `crates/services/camel-auth/src/permission_cache.rs` (modified)

**Steps:**
1. Change the `in_flight` field type from `Mutex<()>` to
   `Mutex<HashMap<String, Arc<Mutex<()>>>>`:
   ```rust
   in_flight: Mutex<HashMap<String, Arc<Mutex<()>>>>,
   ```
2. Update `new` to initialize `in_flight: Mutex::new(HashMap::new())`.
3. In `evaluate` (around line 159), replace the single
   `let _guard = self.in_flight.lock().await;` with per-key dedup
   (same pattern as Task 5.1 step 3 — async block scoped guard +
   key_mutex, catching `?` early returns).
4. After the async block, drop `key_mutex` and run cleanup
   (same pattern as Task 5.1 step 4 — strong_count check + remove).
5. Add a `SlowCountingEvaluator` test helper (extends `CountingEvaluator`
   with a configurable `tokio::time::sleep` delay in `evaluate`).

**Tests:** (executable spec)
- `concurrent_different_requests_no_head_of_line_blocking`: Create a `CachingPermissionEvaluator` wrapping a `SlowCountingEvaluator` with 500ms delay. Evaluate two different permission requests concurrently (`tokio::join!`). Assert the total wall time is < 800ms (parallel). Assert the inner evaluator was called twice (count == 2). Assert both results are `Ok`.
  - **command:** `cargo test -p camel-auth --lib concurrent_different_requests_no_head_of_line_blocking`
  - **expected:** passes after implementation (before fix, takes ~1000ms)
- `concurrent_same_request_dedup_preserved`: Create a `CachingPermissionEvaluator` wrapping a `CountingEvaluator`. Evaluate the same request concurrently (`tokio::join!`). Assert the inner evaluator was called only once (count == 1). Assert both callers receive `Ok`.
  - **command:** `cargo test -p camel-auth --lib concurrent_same_request_dedup_preserved`
  - **expected:** passes after implementation

**Acceptance:**
- `cargo test -p camel-auth --lib` passes (all existing + new tests).
- `cargo clippy -p camel-auth -- -D warnings` exits 0.
- The `in_flight` field is no longer `Mutex<()>`.

- [x] 5.2
