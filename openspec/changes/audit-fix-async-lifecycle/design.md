# Design: audit-fix-async-lifecycle

## Approach

Five independent fixes, one per crate. No cross-crate dependencies. Each
fix is a targeted async-lifecycle correction that eliminates a detached
task, misrouted error, or serialization bottleneck.

### Fix 1: camel-health (rc-7wus)

**Problem:** `HealthServer::stop` (server.rs:133-151) uses
`SHUTDOWN_TIMEOUT = 5s` const, but `handler_timeout` defaults to 6s
(`DEFAULT_HANDLER_TIMEOUT`). The `match tokio::time::timeout(SHUTDOWN_TIMEOUT, handle).await`
has `Ok(_) => {}` — silently drops `JoinError` (panic). On timeout
(`Err(_)`), it warns but does NOT `.abort()` — the detached task can
linger and rebind the port.

**Fix:**
1. Compute `shutdown_timeout = self.handler_timeout + 2s` at `stop()` time.
   This guarantees shutdown waits at least as long as the slowest probe
   handler, plus a 2s margin for axum's graceful-shutdown coordination.
2. Match `timeout(shutdown_timeout, handle).await` with three arms matching
   the actual `JoinHandle<()>` shape (the spawned closure returns `()`, so
   the result is `Result<Result<(), JoinError>, Elapsed>` — two levels):
   - `Ok(Ok(()))` — clean join (task exited normally).
   - `Ok(Err(join_err))` — JoinError (panic or cancel). Log at `error!`.
   - `Err(_)` — timeout. Call `handle.abort()` then `let _ = handle.await`
     to prevent a detached task from rebinding.
3. Log `JoinError` panic arm at `error!` (was silently swallowed via
   `Ok(_) => {}`).

### Fix 2: camel-function (rc-b50f)

**Problem:** `FunctionRuntimeService::stop` (service.rs:298-304) uses `?`
on `provider.shutdown(handle)`, short-circuiting on first error. Remaining
handles are never cancelled or shut down — containers and health tasks
orphan. Inconsistency: `rollback_start` (service.rs:185) already uses
`let _ =` for best-effort drain.

**Fix:** Replace the `?` with a best-effort loop matching
`rollback_start`'s pattern:
```rust
let mut first_err: Option<ProviderError> = None;
for handle in handles {
    handle.cancel.cancel();
    if let Err(e) = self.provider.shutdown(handle).await {
        if first_err.is_none() { first_err = Some(e); }
    }
}
self.invoker.started.store(false, Ordering::SeqCst);
self.status.store(STATUS_STOPPED, Ordering::SeqCst);
if let Some(e) = first_err {
    return Err(CamelError::ProcessorError(e.to_string()));
}
Ok(())
```
Status and `started` flag are always set to STOPPED/false regardless of
partial failure — a half-drained service is not "started."

### Fix 3: camel-jms (rc-0zsm)

**Problem:** `LazyJmsProducer::poll_ready` (component.rs:834-838) returns
`ProcessorError` on `BridgeState::Stopped`. ADR-0024 §Decision mandates
`ConsumerStopping` for shutdown signals — the route compiler
(`route_compiler.rs:425`) and DSL (`dsl/compile.rs:745,770`) do kind-
matching on `ConsumerStopping`. `ProcessorError` doesn't match, so clean
shutdowns are treated as generic 500s and exception policies never fire.

**Fix:** Change `CamelError::ProcessorError(format!(...))` to
`CamelError::ConsumerStopping` in the `BridgeState::Stopped` arm.
`ConsumerStopping` is a **unit variant** (no payload), so the broker-name
diagnostic string is dropped — this matches the ADR-0024 contract and the
inner `JmsProducer` precedent (producer.rs:143). The outer
`LazyJmsProducer` was missed during the ADR-0024 migration.

Note: `BridgeState::Degraded` is NOT a shutdown signal (broker is alive
but unhealthy) and stays as `ProcessorError`.

### Fix 4: camel-master (rc-97gf)

**Problem:** `stop_delegate` (leadership.rs:84-137) has three early-return
arms (delegate Err at :97, panic at :100, cancel at :107) that skip the
epoch-bridge drain at :122-134. The bridge remains detached and can stamp
stale exchanges with its snapshot epoch after the leader yields.

**Fix:** Restructure so the bridge drain always runs:
1. Store the delegate outcome in a variable instead of early-returning.
   Delegate-outcome → stored-value mapping:
   - `Ok(Ok(Ok(())))` → `None` (delegate exited cleanly).
   - `Ok(Ok(Err(err)))` → `Some(err)` (delegate returned error).
   - `Ok(Err(e)) if e.is_panic()` → `Some(CamelError::ProcessorError(...))`.
   - `Ok(Err(e))` (cancelled) → `Some(CamelError::ProcessorError(...))`.
   - `Err(_)` (timeout) → `None` after `handle.abort()` (current behavior).
2. Always execute the epoch-bridge drain block.
3. Return `delegate_result?` (which is `Ok(())` for clean exit / timeout,
   or `Err(...)` for error/panic/cancel) after the bridge drain completes.
4. On delegate panic/cancel/timeout, `handle.abort()` before proceeding
   to the bridge drain (same as current timeout arm).

This guarantees the bridge is either drained within its own
`drain_timeout` window or aborted — no detached bridge can survive
regardless of how the delegate exits.

### Fix 5: camel-auth (rc-h6yv)

**Problem:** Both `CachingTokenIntrospector` and
`CachingPermissionEvaluator` hold `in_flight: Mutex<()>` across the
backend HTTP `.await`. Different tokens serialize unnecessarily — under
a slow IdP (10s timeout), Token B waits for Token A's HTTP call to
complete before its own cache check.

**Fix:** Replace `in_flight: Mutex<()>` with per-key dedup:
```rust
in_flight: Mutex<HashMap<String, Arc<Mutex<()>>>>,
```
Flow:
1. Compute cache key (SHA-256 hash, already computed for the result cache).
2. Fast-path cache check (read lock) — unchanged.
3. Get-or-insert per-key `Arc<Mutex<()>>` from the outer Mutex (held
   briefly — only for HashMap insertion, no await).
4. Clone the Arc, drop the outer guard.
5. Lock the per-key mutex (held across the HTTP await).
6. Double-check cache (hit-after-wait).
7. HTTP call + cache insert — unchanged.
8. Drop per-key guard.
9. In-flight cleanup runs on **every** cache miss (before the cache insert
   at step 7), NOT gated by the result-cache capacity check in
   `evict_if_needed()`. The cleanup acquires the outer `in_flight` mutex
   as a single critical section: iterate entries, remove any whose
   `Arc::strong_count == 1` (only the map holds them — no concurrent
   waiter). This prevents unbounded growth under low cache pressure.

The cleanup is race-free because both get-or-insert (step 3) and cleanup
(step 9) acquire the same outer `in_flight` mutex — no `Arc` clone can be
issued between the `strong_count` test and the `remove`.

This eliminates cross-token serialization while preserving same-token
thundering-herd prevention.

## Affected crates

- `camel-health`: server.rs (stop timeout + abort + JoinError handling)
- `camel-function`: service.rs (best-effort drain in stop)
- `camel-component-jms`: component.rs (ConsumerStopping on Stopped)
- `camel-component-master`: leadership.rs (epoch-bridge drain on all paths)
- `camel-auth`: introspection.rs + permission_cache.rs (per-key dedup)

## Architecture boundaries

All fixes are within-component lifecycle corrections. No cross-layer
interface changes. The JMS fix aligns the error variant with the existing
`ConsumerStopping` contract — the DSL and route compiler already
pattern-match on it, so this is a conformance fix, not a new signal.

## Alternatives considered

- **Health: const SHUTDOWN_TIMEOUT bump only.** Rejected — a fixed
  const cannot track a user-configured `handler_timeout` via
  `set_handler_timeout()`. Deriving at call time is correct.
- **Auth: remove in_flight entirely.** Rejected — loses thundering-herd
  prevention for the same token. Per-key dedup is strictly better.
- **Auth: DashMap for in_flight.** Rejected — adds a dependency for a
  brief-held map. `tokio::sync::Mutex<HashMap>` is sufficient because the
  outer lock is never held across an await.
- **Master: abort bridge on early-return.** Rejected — abort skips the
  drain. The bridge may have buffered envelopes that should be forwarded.
  Drain-then-propagate is correct.
