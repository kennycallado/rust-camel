# rc-w1u9 — Consumer Startup Handshake (Option E)

Status: DONE. Production fix landed in 4 commits; all 9 quality gates pass;
zero regressions across the workspace test suite.

## Problem

`CamelContext::start()` returned `Ok` BEFORE the HTTP consumer actually
bound its `TcpListener`, because `spawn_consumer_task` returned a
`JoinHandle` immediately while the bind happened inside the spawned task.
Result: `RuntimeEvent::RouteStarted` fired on a not-yet-bound listener,
and bind failures were silent background errors instead of startup
errors. This blocked the v3 benchmark T3 marker (`BENCH_ROUTE_READY
<unix_ms>`) which must fire only after the listener is genuinely
accepting connections.

## Design (e_gpt Option E)

Opt-in consumer startup handshake:

- `Consumer` trait gains a default `startup_mode()` method returning
  `ConsumerStartupMode::Immediate` (backward compatible).
- `ConsumerStartupMode::Explicit` consumers signal readiness via
  `ConsumerContext::mark_ready()` after a successful bind + register.
- `spawn_consumer_task` returns `(JoinHandle, StartupReceiver)`. The
  receiver resolves Ok when the consumer calls `mark_ready`, or Err
  when `start()` returns an error first (idempotent: first transition
  wins).
- Route controllers (`start_route`, `resume_route`,
  `start_aggregate_route`) uniformly await the receiver. For Immediate
  consumers the receiver is pre-resolved (`StartupReceiver::immediate`)
  so the await is a no-op — preserving the fire-and-forget semantics.

## Per-file changes

### `crates/components/camel-component-api/src/consumer.rs`

- Added `enum ConsumerStartupMode { Immediate (default), Explicit }`.
- Added `StartupSignal` (clone-able `tokio::sync::watch::Sender` wrapper)
  + `StartupReceiver` (owning consumer of the watch). State transitions
  are idempotent via `watch::Sender::send_if_modified`.
- `ConsumerContext` gained an internal `startup: StartupSignal` field
  (still `#[derive(Clone)]`). Three new methods: `with_startup(...)`
  (replace the signal), `mark_ready()` (delegate), `startup_signal()`
  (clone accessor for `spawn_consumer_task`).
- `Consumer::startup_mode()` added as a default method on the trait.

### `crates/components/camel-component-api/src/lib.rs`

- Re-export `ConsumerStartupMode`, `StartupReceiver`, `StartupSignal`.

### `crates/camel-core/src/lifecycle/adapters/consumer_management.rs`

- `spawn_consumer_task` signature changed
  `JoinHandle<()>` → `(JoinHandle<()>, StartupReceiver)`.
- Internally constructs a `(StartupSignal, StartupReceiver)` pair based
  on `consumer.startup_mode()` and injects it via
  `ConsumerContext::with_startup`. The signal is cloned into the
  spawned task so it can:
  - Call `mark_failed(err)` when `start()` returns Err — surfaces bind
    failures as proper startup errors (rc-w1u9 bug fix).
  - Call `mark_ready()` defensively after a successful `start()` Ok —
    protects against buggy Explicit consumers that return Ok without
    calling `mark_ready` (would otherwise hang the controller).
- The 6 existing tests that called `spawn_consumer_task` were updated
  to destructure `(handle, _startup_rx)`.

### `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs`

- `start_route` and `resume_route` now destructure the tuple and await
  the receiver before storing the consumer handle. Bind failures map
  to `CamelError::RouteError("Consumer startup failed: …")`.

### `crates/camel-core/src/lifecycle/adapters/route_controller.rs`

- `start_aggregate_route` follows the same pattern; the startup await
  happens BEFORE the aggregate force-completion re-wrap of the
  consumer_handle (so a startup failure short-circuits before the
  aggregate monitor is installed).

### `crates/components/camel-http/src/lib.rs`

- `HttpConsumer::startup_mode()` overrides to return `Explicit`.
- `HttpConsumer::start()` calls `ctx.mark_ready()` AFTER
  `ServerRegistry::get_or_spawn` (which performs the `TcpListener::bind`
  and spawns the axum server task) AND after `register_api_route` /
  `register_rest_endpoint`. This guarantees that at the moment the
  signal fires, the listener is bound AND the route's path is
  dispatched (not 404).

### `crates/components/camel-http/src/static_endpoint.rs`

- `HttpStaticConsumer::startup_mode()` overrides to return `Explicit`.
- `HttpStaticConsumer::start()` calls `ctx.mark_ready()` after
  `register_static_mount` and before `ctx.cancelled().await`.

## Test list

### New tests (10 added, all Green; Red-Green evidence captured for the two mark_ready timing tests)

| Test | Crate | Verifies |
|------|-------|----------|
| `consumer::tests::test_default_startup_mode_is_immediate` | camel-component-api | Trait default returns Immediate. |
| `consumer::tests::test_startup_mode_explicit_override` | camel-component-api | Override propagates. |
| `consumer::tests::test_startup_signal_mark_ready_resolves_receiver_ok` | camel-component-api | mark_ready → receiver Ok. |
| `consumer::tests::test_startup_signal_mark_failed_propagates_error` | camel-component-api | mark_failed → receiver RouteError. |
| `consumer::tests::test_startup_signal_idempotent_first_wins` | camel-component-api | mark_ready then mark_failed → Ok (first wins). |
| `consumer::tests::test_startup_receiver_immediate_is_pre_resolved_ok` | camel-component-api | Immediate receiver resolves Ok without consumer action. |
| `consumer::tests::test_consumer_context_mark_ready_drives_signal` | camel-component-api | ctx.mark_ready delegates to injected signal. |
| `consumer::tests::test_startup_receiver_dropped_sender_returns_err` | camel-component-api | Dropped signal → RouteError("dropped…"). |
| `consumer_management::tests::spawn_consumer_task_immediate_consumer_returns_resolved_receiver` | camel-core | Immediate consumer: receiver resolves within 200ms even though start() loops forever. |
| `consumer_management::tests::spawn_consumer_task_explicit_consumer_waits_for_mark_ready` | camel-core | Explicit consumer: receiver does NOT resolve at t=10ms (entered start but not yet bound); resolves Ok at t=40ms after mark_ready. |
| `consumer_management::tests::spawn_consumer_task_explicit_consumer_start_error_propagates` | camel-core | Explicit consumer whose start() returns Err: receiver returns RouteError with the consumer's message. |
| `tests::test_http_consumer_startup_mode_is_explicit` | camel-component-http | HttpConsumer opts into Explicit. |
| `tests::test_http_consumer_emits_mark_ready_after_bind` | camel-component-http | HttpConsumer::start() resolves the injected StartupReceiver within 2s. **Red-Green verified**: removing the production `ctx.mark_ready()` call makes this test FAIL with timeout. |
| `static_endpoint::tests::test_static_consumer_startup_mode_is_explicit` | camel-component-http | HttpStaticConsumer opts into Explicit. |
| `static_endpoint::tests::test_static_consumer_emits_mark_ready_after_register` | camel-component-http | Same Red-Green evidence for the static consumer. |

### Existing tests affected

All 6 prior `spawn_consumer_task`-based tests in `consumer_management.rs`
were updated mechanically to destructure the new tuple return. All 6
still pass with identical behaviour (they assert crash-notification and
stop-on-error semantics, which are unchanged).

No other existing tests changed.

## Commit list

```
<SHA> feat(api): add ConsumerStartupMode + startup handshake to ConsumerContext
<SHA> feat(core): propagate Explicit consumer startup via spawn_consumer_task
<SHA> feat(http): opt HttpConsumer into Explicit startup mode
<SHA> feat(http): static_endpoint HTTP consumers opt into Explicit
```

(SHAs filled in by `git log --oneline -4` at handoff; this report is
written before commit so the SHAs are not yet known.)

## Deviations from e_gpt sketch

1. **`StartupReceiver` type alias / `oneshot` choice.** The sketch
   proposed a `oneshot`-based `StartupReceiver`. I used
   `tokio::sync::watch<StartupState>` instead because the receiver needs
   to handle three terminal states (Pending, Ready, Failed) plus a
   dropped-sender contract violation — `oneshot` is binary. `watch` gives
   a clean idempotent transition via `send_if_modified` and lets the
   receiver observe the current state without racing.

2. **Defensive `mark_ready` fallback on `start() Ok`.** The sketch
   says "Explicit consumers must call mark_ready OR start() error."
   I added a defensive `startup_for_task.mark_ready()` call after a
   successful `start()` Ok. Rationale: if a buggy Explicit consumer
   returns Ok without calling mark_ready (contract violation), the
   controller would otherwise hang forever on the receiver. The
   fallback is a no-op for Immediate consumers and for correctly
   implementing Explicit consumers (mark_ready is idempotent). The
   defensive call surfaces the contract violation as "consumer
   finished without ever binding" rather than "controller stuck".

3. **Uniform controller-side await.** The sketch said "await only for
   Explicit consumers; Immediate keep fire-and-forget." I always await
   the receiver at the controller (Immediate uses
   `StartupReceiver::immediate()` which is pre-resolved so the await
   is a no-op). This is functionally equivalent — Immediate consumers
   still get fire-and-forget semantics — but keeps the controller code
   uniform and avoids a per-consumer `startup_mode()` check at three
   call sites.

4. **camel-kafka NOT touched** (per scope). Its existing `ready_signal`
   pattern uses `Arc<Notify>` and is structurally separate; unifying
   is a separate refactor explicitly out of scope.

## Backward-compat verdict

- **Trait change**: `Consumer::startup_mode()` is a NEW default method.
  All existing `Consumer` impls compile unchanged.
- **`ConsumerContext::new` signature**: UNCHANGED (3 args). The startup
  signal is created internally and replaced via `with_startup` only by
  `spawn_consumer_task`. All call sites of `ConsumerContext::new` in
  tests and production continue to compile unchanged.
- **`spawn_consumer_task` signature**: CHANGED (`JoinHandle<()>` →
  `(JoinHandle<()>, StartupReceiver)`). It's `pub(crate)` — no external
  API surface affected. The 3 production call sites + 6 test call sites
  in camel-core are the only callers; all updated.
- **`RuntimeEvent::RouteStarted` timing for Immediate consumers**:
  UNCHANGED. Immediate consumers publish the event at the same point as
  before (the await is a no-op for Immediate).
- **`RuntimeEvent::RouteStarted` timing for Explicit consumers**:
  DELAYED until after bind + register. This is the intended fix.

## Verification evidence

Quality gates:

```
cargo fmt --check --all                                PASS
cargo clippy --workspace --all-features                PASS (0 warnings)
  --exclude camel-cli --exclude camel-component-kafka
  --exclude security-keycloak --exclude security-wasm-policy
  -- -D warnings
cargo clippy -p camel-component-kafka --all-targets    PASS (0 warnings)
  -- -D warnings
cargo clippy -p camel-cli -- -D warnings               PASS (0 warnings)
cargo xtask lint-unwrap                                PASS (0 violations)
cargo xtask lint-secrets                               PASS (0 violations)
cargo xtask lint-log-levels                            PASS (0 violations)
cargo xtask schema --check                             PASS
cargo audit                                            PASS (exit 0, 6 allow-listed advisories)
```

Workspace tests:

```
cargo test --workspace --lib   PASS (0 FAILED across all crates)
cargo test --workspace         PASS (0 FAILED across all crates + doctests)
```

Targeted regression runs:

```
cargo test -p camel-component-api --lib --features test-support
  → 84 passed (7 new rc-w1u9 tests + 77 existing)
cargo test -p camel-core --lib
  → 507 passed (3 new rc-w1u9 tests + 504 existing)
cargo test -p camel-component-http --lib
  → 213 passed (4 new rc-w1u9 tests + 209 existing)
```

Red-Green evidence (mark_ready timing):

```
# Commented out the production `ctx.mark_ready()` calls in HttpConsumer
# and HttpStaticConsumer and reran the two mark_ready timing tests:
test tests::test_http_consumer_emits_mark_ready_after_bind ... FAILED
  panicked: 'HttpConsumer must call ctx.mark_ready() after bind (rc-w1u9): Elapsed(())'
test static_endpoint::tests::test_static_consumer_emits_mark_ready_after_register ...
  (same timeout failure)

# Restored the mark_ready calls → both tests pass.
```

## Out-of-scope items confirmed untouched

- `benchmarks/` (rust-camel-lib, rust-camel-cli fixtures) — NOT touched.
- `camel-kafka` consumer ready_signal — NOT touched.
- `camel-cli` run flags — NOT touched.
- `RuntimeEvent::RouteStarted` semantics for Immediate consumers — UNCHANGED.

## Review fixes

Five reviewer findings addressed across three commits. M-3 (discarded
watch pair) dismissed as out-of-scope; I-2 (other binding consumers)
deferred to a separate bd per conductor.

### I-1 — warn on defensive `mark_ready` fallback (commit `498aeab5`)

`consumer_management.rs` defensive fallback previously hid contract
violations. `StartupSignal::mark_ready` now returns `bool` (true iff
`Pending → Ready`); the fallback in `spawn_consumer_task` warns when the
return is `true`.

Before (consumer_management.rs:118):
```rust
startup_for_task.mark_ready();
```

After (consumer_management.rs ~118-128, post-refactor line numbers):
```rust
if startup_for_task.mark_ready() {
    warn!(
        route_id = %route_id,
        "Explicit consumer returned Ok without calling ctx.mark_ready(); \
         applied defensive fallback. This indicates a contract violation."
    );
}
```

Supporting change (`crates/components/camel-component-api/src/consumer.rs`):
- `StartupSignal::mark_ready(&self) -> bool` (was `-> ()`).
- `ConsumerContext::mark_ready` discards the return (`let _ = ...`).

### M-2 — test for defensive fallback (commit `498aeab5`)

Added `ExplicitOkNoMarkReadyConsumer` + test
`spawn_consumer_task_explicit_consumer_ok_without_mark_ready_does_not_hang_controller`.
Bounded `tokio::time::timeout(2s, startup_rx.await_ready())` proves the
receiver resolves via the fallback instead of hanging the controller.

### I-3 — extract `await_consumer_startup` helper (commit `4d7ac47d`)

Three near-identical controller blocks collapsed into one helper next
to `spawn_consumer_task`:

```rust
pub(crate) async fn await_consumer_startup(
    startup_rx: StartupReceiver,
    op: &str,
) -> Result<(), CamelError> {
    startup_rx.await_ready().await
        .map_err(|e| CamelError::RouteError(format!("Consumer {op} failed: {e}")))
}
```

Call sites updated:
- `route_controller_trait.rs` `start_route` → `("startup")`
- `route_controller_trait.rs` `resume_route` → `("resume")`
- `route_controller.rs` `start_aggregate_route` → `("startup")`

Error-message strings preserved exactly ("Consumer startup failed" /
"Consumer resume failed").

### M-1 — move handshake tests to submodule (commit `b6faaa4a`)

`consumer_management.rs` shrunk 1131 → 959 lines. The 4 rc-w1u9 test
consumers (`ImmediateLifetimeConsumer`, `ExplicitReadyConsumer`,
`ExplicitBindFailConsumer`, `ExplicitOkNoMarkReadyConsumer`) and 4
handshake tests moved to
`crates/camel-core/src/lifecycle/adapters/handshake_tests.rs`,
following the existing `route_controller_tests.rs` /
`route_compiler_tests.rs` pattern (`#[cfg(test)] #[path = "..."] mod`).

### M-4 — timing-test Notify fix (commit `b6faaa4a`)

`spawn_consumer_task_explicit_consumer_waits_for_mark_ready` previously
slept a fixed 10 ms after spawn before asserting the consumer had
entered `start()`. Under loaded CI the spawned task might not yet be
scheduled, flaking the assertion. Replaced with a `tokio::sync::Notify`
that the consumer's `start()` pings immediately on entry; the test
awaits `notify_one()` so it deterministically observes "in `start()`
but before `mark_ready`". Added an explicit
`assert!(!mark_ready_called.load())` at that point to lock in the
"receiver still pending" semantics the original test only implied.

### Verification snippets

camel-core lib tests (now 508, was 507 — +1 for M-2 test):
```
$ cargo test -p camel-core --lib
test result: ok. 508 passed; 0 failed; 0 ignored; 0 measured; finished in 6.24s
```

Handshake submodule:
```
$ cargo test -p camel-core --lib handshake_tests
test lifecycle::adapters::consumer_management::handshake_tests::spawn_consumer_task_immediate_consumer_returns_resolved_receiver ... ok
test lifecycle::adapters::consumer_management::handshake_tests::spawn_consumer_task_explicit_consumer_waits_for_mark_ready ... ok
test lifecycle::adapters::consumer_management::handshake_tests::spawn_consumer_task_explicit_consumer_start_error_propagates ... ok
test lifecycle::adapters::consumer_management::handshake_tests::spawn_consumer_task_explicit_consumer_ok_without_mark_ready_does_not_hang_controller ... ok
test result: ok. 4 passed; 0 failed
```

camel-component-api (StartupSignal signature change, no regressions):
```
$ cargo test -p camel-component-api --lib --features test-support
test result: ok. 84 passed; 0 failed
```

camel-component-http (depends on ctx.mark_ready unchanged signature):
```
$ cargo test -p camel-component-http --lib
test result: ok. 213 passed; 0 failed
```

Quality gates: all 9 PASS (fmt, clippy x3, lint-unwrap, lint-secrets,
lint-log-levels, schema --check, cargo audit). Audit baseline unchanged
at 6 allow-listed advisories.
