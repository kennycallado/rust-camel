# Design: lockdeadline — deadline-bounded global test-lock acquisition

## Context

All six locks are empty `tokio::sync::Mutex<()>` statics held for the
whole test body (serialization guards, never data carriers). Every
one of the 97 sites is a mechanical shape:
`let _guard = LOCK.lock().await;` (94 sites) or
`let _ack_guard = ACK_TEST_LOCK.lock().await;` (3 sites). No site
re-reads the guard — the mutex is pure mutual exclusion. Each lock
static is scoped to exactly one integration-test binary (or one
in-crate `#[cfg(test)]` module), so queue depth is bounded by that
binary's holder count.

## Helper

Location: `camel_component_api::test_support` (module already gated
behind the `test-support` Cargo feature; tokio is a regular
workspace dep with `full` features, so `tokio::time::timeout` is
available).

```rust
#[track_caller]
pub fn acquire_deadline<'a, T: ?Sized>(
    lock: &'a tokio::sync::Mutex<T>,
    what: &'a str,
    deadline: std::time::Duration,
) -> impl std::future::Future<Output = tokio::sync::MutexGuard<'a, T>> + 'a
```

(Named lifetime required: two borrowed inputs feed the returned
future, so anonymous elision would not compile.)

- The outer function is SYNCHRONOUS and `#[track_caller]`: it
  captures `std::panic::Location::caller()` into a local BEFORE
  returning the async block. Sync caller capture is fully
  well-defined (no reliance on `#[track_caller]` behavior across
  async polling); the recorded site is the acquisition call site.
- Async body: `tokio::time::timeout(deadline, lock.lock())`; on
  `Err(_)` panic with
  `"test lock {what} not acquired within {deadline:?} — holder stalled (bd rc-88old), site {caller}"`.
- Cancellation safety: dropping the `lock()` future on timeout loses
  the waiter's queue position (fairness) but preserves mutex and
  wait-queue integrity — correct for a panicking test.
- The `lock.lock()` inside the timeout future argument is
  deadline-bounded per the lint's enclosure rule, so the helper
  itself is not an unbounded-wait site.
- Guard-drop semantics are identical: the returned
  `MutexGuard<'_, T>` releases on drop at the same lexical point as
  before.
- Call sites are unchanged ergonomically:
  `let _guard = acquire_deadline(&LOCK, "LOCK family", TEST_LOCK_DEADLINE).await;`

Placement rationale (ADR-0055 publish-topology): camel-dsl cannot
dev-depend on camel-test (camel-test → camel-config → camel-dsl
closes a publish-order cycle). `camel-component-api` is upstream of
all four consumers and every one of them already dev-depends on it
with `features = ["test-support"]` enabled — zero manifest changes,
zero new edges. camel-test has it as a regular dep with
`test-support` enabled. One helper, no per-crate reimplementation.

## Deadline

One uniform constant, `TEST_LOCK_DEADLINE = 900 s`, exported next to
the helper and used by all six families. Derivation — a waiting
test's healthy worst case is (holders − 1) × longest holder, because
each lock family lives in a single test binary:

| Family (binary) | Holders | Holder bound | Worst healthy queue |
|---|---|---|---|
| REGISTRY_TEST_LOCK (camel-ws lib tests) | 44 | ~14 s (5 s connect bound + short body) | 43×14 = 602 s |
| SERVER_MUTEX (camel-dsl rest e2e) | 23 | ~27 s (server lifecycle) | 22×27 = 594 s |
| TEST_MUTEX (camel-test http_static) | 18 | ~35 s (static-port server lifecycle) | 17×35 = 595 s |
| SENTINEL_TOPOLOGY_LOCK (redis_repositories) | 6 | 90 s (internal failover timeouts) | 5×90 = 450 s |
| TOPOLOGY_LOCK (redis_sentinel) | 3 | 90 s | 2×90 = 180 s |
| ACK_TEST_LOCK (wasm source_bind_gate) | 3 | ~10 s (ack gate, `#[ignore]` ADR-0054) | 2×10 = 20 s |

900 s covers the largest chain (602 s) with ~1.5× margin for CI
load jitter; any holder exceeding its bound fails its OWN internal
deadline first. A 900 s acquisition timeout therefore indicates a
stalled holder, not queue depth. The trade-off: a wedged binary
surfaces in ≤15 min instead of never. Deadline choice prioritizes
zero false positives on healthy-but-loaded CI over fast wedge
detection.

## Non-goals

- No change to `scripts/xtask/src/lint_unbounded_wait.rs` or
  `ratchet-unbounded-wait.max` (mission 238 owns both). If the lint
  should grow a lock-acquisition rule, that is a filed bd, not this
  change.
- Stream I/O blind spot and the remaining unadjudicated inventory
  are out of scope.
- No `allow-test-wait` adjudications: all 97 sites convert. No
  non-test locks exist for these six names.

## Test plan

Helper unit tests (in `test_support.rs` `#[cfg(test)]` module; the
crate's self dev-dependency enables `test-support` for
`cargo test -p camel-component-api`):

1. `acquire_deadline_uncontended_returns_guard` — free lock acquires
   immediately; guard drops cleanly.
2. `acquire_deadline_stalled_holder_panics_naming_lock` —
   `#[should_panic(expected = "...")]`; a spawned holder acquires the
   lock and awaits `std::future::pending()` (no sleep — lint-test-sleep
   clean); a 100 ms deadline acquisition panics naming the lock and
   the deadline.
3. `acquire_deadline_panic_names_call_site` — same stalled-holder
   setup inside `std::panic::catch_unwind(AssertUnwindSafe(..))` on a
   current-thread runtime; the downcast panic payload is asserted to
   contain the lock name, the deadline, AND the caller file
   (`test_support.rs`), proving the `#[track_caller]` site capture.

Conversion verification: `rg` proves zero remaining
`<LOCK>.lock().await` sites; affected crates compile; locally
runnable suites (camel-ws lib tests, camel-dsl rest e2e,
camel-component-api) pass. Redis suites (infra-gated) and wasm
`#[ignore]` gates are compile-verified; runtime verification
deferred to CI.

## Boundaries

Test-only change: no production surface, no data/control plane
interaction (ADR-0045 untouched). The helper lives in a
feature-gated test-support module — it never leaks into production
builds (ADR-0041 single-source discipline for shared test helpers).
