# Tasks: audit-fix-direct-startup-race

## camel-component-direct

### Task 1.1: Override startup_mode to Explicit and call mark_ready after registration

**Files:**
- `crates/components/camel-direct/src/lib.rs` (modified)
- `crates/components/camel-direct/CONTEXT.md` (modified)

**Steps:**
1. Add `ConsumerStartupMode` to the existing `camel_component_api` import block at the top of `crates/components/camel-direct/src/lib.rs`. If no such import exists, add `use camel_component_api::ConsumerStartupMode;` near the other `use` statements.
2. In `impl Consumer for DirectConsumer` (around line 282), add the `startup_mode` override immediately after the opening brace of the impl block, before the `async fn start` method:

   ```rust
   fn startup_mode(&self) -> ConsumerStartupMode {
       ConsumerStartupMode::Explicit
   }
   ```

   Reference: HTTP does this at `crates/components/camel-http/src/lib.rs:1694`.

3. In `DirectConsumer::start()` (around line 283), insert `context.mark_ready();` between the end of the registration block (the closing `}` of the scope that begins with `{ let mut reg = self.registry.lock()`, contains the `reg.insert(self.name.clone(), tx)` call at line 299, and closes at line 300) and the variable setup that follows (`let name = self.name.clone()` at line 302). The call MUST be AFTER the lock guard scope closes and BEFORE the event loop begins at line 313. Reference: HTTP calls `ctx.mark_ready()` at `crates/components/camel-http/src/lib.rs:1413` after bind.

4. Add a new unit test `test_direct_consumer_startup_mode_is_explicit` in the `#[cfg(test)] mod tests` block. Mirror `test_http_consumer_startup_mode_is_explicit` at `crates/components/camel-http/src/lib.rs:4040`. The test creates a `DirectConsumer` via `DirectComponent::new()` → `create_endpoint("direct:ready-check", &NoOpComponentContext)` → `create_consumer(rt())`, then asserts `consumer.startup_mode() == ConsumerStartupMode::Explicit`. Use the existing `rt()` test helper already used in other direct tests.

5. Add a new async unit test `test_direct_consumer_marks_ready_after_registration` in the `#[cfg(test)] mod tests` block. Mirror `test_http_consumer_emits_mark_ready_after_bind` at `crates/components/camel-http/src/lib.rs:4068`. Do NOT copy HTTP's `#[allow(clippy::await_holding_lock)]` or `REGISTRY_TEST_MUTEX` guard — Direct's registry is per-`DirectComponent`, so no global test mutex is needed and no lock is held across `.await`. The test:
   - Builds `DirectConsumer` directly via the struct literal `DirectConsumer { name: "ready-probe-direct".into(), registry: registry.clone(), cancel: None, runtime: rt() }` — mirroring `test_direct_consumer_respects_cancellation` (`crates/components/camel-direct/src/lib.rs:911-916`) — so the test retains a `registry` handle to inspect. Do NOT use `create_consumer`, which boxes the consumer and hides the registry.
   - Creates a `ConsumerContext` with a channel pair, cancel token, and route ID — the same pattern as the HTTP test at line 4090-4092.
   - Injects a `StartupSignal` pair via `ctx.with_startup(signal)` (line 4095-4096 pattern).
   - Spawns `consumer.start(ctx)` in a tokio task (line 4100-4102 pattern).
   - Awaits `tokio::time::timeout(Duration::from_secs(2), startup_rx.await_ready())` and asserts `Ok` (line 4108-4112 pattern).
   - After readiness resolves, asserts that the consumer's name IS present in the `DirectRegistry` (the `registry` handle from the struct literal) — proving `mark_ready` was called AFTER the registry insert, not before.
   - Cancels the consumer via its cancel token to unwind the spawned task and avoid leaking.

6. In `crates/components/camel-direct/CONTEXT.md`, add a new section titled "Startup handshake" after the existing "Log-level policy" section. Content:

   ```
   ## Startup handshake

   `DirectConsumer` declares `ConsumerStartupMode::Explicit`. Its `start()` calls
   `ConsumerContext::mark_ready()` immediately after inserting into the shared
   `DirectRegistry`, before entering the event loop. The runtime's `start_context`
   starts routes sequentially by `startup_order`; a producer route with a higher
   `startup_order` than the consumer route will not be driven until the consumer's
   `StartRoute` completes (registration visible + `mark_ready` resolved).

   **Residual operator window:** if a producer route and its consumer route share
   the same `startup_order` (default 1000), ordering within the tier is by the
   controller's stable list order. Operators who need a strict guarantee set the
   consumer's `startup_order` lower so it starts first. This matches Apache
   Camel's own guidance (start direct consumers before their producers).
   ```

**Tests:** (executable spec — name, setup, action, assert)
- `test_direct_consumer_startup_mode_is_explicit`: A `DirectConsumer` created from `DirectComponent::new().create_endpoint("direct:ready-check", &NoOpComponentContext).create_consumer(rt())` → `consumer.startup_mode()` → asserts `== ConsumerStartupMode::Explicit`. Command: `cargo test -p camel-component-direct --lib test_direct_consumer_startup_mode_is_explicit`. Expected: passes after step 2.
- `test_direct_consumer_marks_ready_after_registration`: Injected `StartupSignal` pair via `ConsumerContext::with_startup(signal)` → spawn `start()` → `timeout(2s, startup_rx.await_ready())` resolves `Ok` AND the registry contains the consumer's name at that point → cancel to unwind. Command: `cargo test -p camel-component-direct --lib test_direct_consumer_marks_ready_after_registration`. Expected: passes after step 3; fails (timeout) without the `mark_ready` call.
- `test_poll_ready_endpoint_not_registered`: existing test (line 961) → unchanged → must stay green. Command: `cargo test -p camel-component-direct --lib test_poll_ready_endpoint_not_registered`. Expected: passes (no change to poll_ready).
- `test_poll_ready_endpoint_registered`: existing test (line 987) → unchanged → must stay green. Command: `cargo test -p camel-component-direct --lib test_poll_ready_endpoint_registered`. Expected: passes.
- `test_direct_duplicate_consumer_returns_error`: existing duplicate-consumer test → unchanged → must stay green (Err path returns before mark_ready). Command: `cargo test -p camel-component-direct --lib test_direct_duplicate_consumer_returns_error`. Expected: passes.

**Acceptance:**
- `cargo test -p camel-component-direct --lib` passes (all existing + 2 new tests green)
- `cargo clippy -p camel-component-direct --all-targets -- -D warnings` exits 0
- `cargo fmt --check --all` exits 0
- `test_poll_ready_endpoint_not_registered` is unchanged and green
- `CONTEXT.md` has the "Startup handshake" section

- [x] 1.1
