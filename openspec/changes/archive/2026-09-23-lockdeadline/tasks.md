# Tasks: lockdeadline

Single-phase mechanical sweep: one shared helper + 97 site
conversions. All conversions use the SAME edit shape and the SAME
deadline constant. Lint sources and ratchet files are NEVER touched.
ORDERING: tasks 2-5 import the helper landed by task 1 — task 1
must complete before tasks 2-5 dispatch (2-5 are mutually
independent). Task 6 runs LAST, after 2-5.

## Task 1 — Helper: `acquire_deadline` + `TEST_LOCK_DEADLINE`

Files:
- `crates/components/camel-component-api/src/test_support.rs` (modified)

Steps:
1. In `test_support.rs` (feature-gated `test-support` module), add a
   public constant:
   `pub const TEST_LOCK_DEADLINE: std::time::Duration = std::time::Duration::from_secs(900);`
   with a doc comment summarizing the derivation (worst healthy
   serialization chain 602 s = 43×14 s; 900 s ≈ 1.5× margin; bd
   rc-88old).
2. Add the helper (sync `#[track_caller]` outer fn returning an
   async block — named lifetime, both borrows tied to `'a`):
   ```rust
   /// Acquire a process-global test-serialization lock with a deadline.
   ///
   /// Test locks are empty `Mutex<()>` guards held for a whole test body.
   /// A stalled holder parks every queued test forever (bd rc-88old,
   /// rc-y24l camel-ws hang class). Bounding the acquisition turns the
   /// wedge into one failing test with a named lock + site.
   #[track_caller]
   pub fn acquire_deadline<'a, T: ?Sized>(
       lock: &'a tokio::sync::Mutex<T>,
       what: &'a str,
       deadline: std::time::Duration,
   ) -> impl std::future::Future<Output = tokio::sync::MutexGuard<'a, T>> + 'a {
       let caller = std::panic::Location::caller();
       async move {
           match tokio::time::timeout(deadline, lock.lock()).await {
               Ok(guard) => guard,
               Err(_) => panic!(
                   "test lock {what} not acquired within {deadline:?} \
                    — holder stalled (bd rc-88old), site {caller}"
               ),
           }
       }
   }
   ```
3. Append a `#[cfg(test)]` unit-test module at the bottom of
   `test_support.rs` with the three tests below (the crate's self
   dev-dependency enables `test-support` for
   `cargo test -p camel-component-api`).

Tests:
- name: `acquire_deadline_uncontended_returns_guard`
  setup: a local `tokio::sync::Mutex<()>` in `#[tokio::test]`.
  action: `let _guard = acquire_deadline(&lock, "UNIT_LOCK", Duration::from_secs(60)).await;` then drop.
  assert: acquisition returns promptly (no panic); guard drops.
  command: `cargo test -p camel-component-api acquire_deadline_uncontended`
  expected: fails before the helper exists (unresolved symbol); passes after.
- name: `acquire_deadline_stalled_holder_panics_naming_lock`
  setup: `#[tokio::test]` `#[should_panic(expected = "test lock STALLED_UNIT_LOCK not acquired within")]`; a oneshot channel `(tx, rx)`; spawn a holder task that acquires the lock, sends `tx.send(())` as the acquired-acknowledgment, then awaits `std::future::pending()` (NO `tokio::time::sleep`). The test awaits `rx.await` FIRST so the holder provably holds the lock before the acquisition attempt (deterministic, race-free).
  action: `acquire_deadline(&lock, "STALLED_UNIT_LOCK", Duration::from_millis(100)).await`.
  assert: panic message contains the lock name and `100ms`.
  command: `cargo test -p camel-component-api acquire_deadline_stalled`
  expected: fails before implementation; passes after.
- name: `acquire_deadline_panic_names_call_site`
  setup: same deterministic oneshot-ack stalled-holder setup (holder acquires, `tx.send(())`, then `pending()`) inside a manually built current-thread tokio runtime; the test awaits the ack `rx` before proceeding; wrap the acquisition in `std::panic::catch_unwind(std::panic::AssertUnwindSafe(..))`.
  action: downcast the payload (`String`/`&str` box) to a message.
  assert: message contains `STALLED_UNIT_LOCK`, `100ms`, AND `test_support.rs` (proves `#[track_caller]` captured the acquisition site).
  command: `cargo test -p camel-component-api acquire_deadline_panic_names`
  expected: fails before implementation; passes after.

Acceptance:
- `cargo test -p camel-component-api acquire_deadline` exits 0 (3 tests).
- `cargo fmt --check` clean; `cargo clippy -p camel-component-api --all-targets -- -D warnings` exits 0.
- `cargo xtask lint-test-sleep` exits 0 (no sleep introduced).
- Spec scenarios covered: stalled-holder panic, deadline derivation, one-helper, observable sleep-free timeout path.

- [x] task-1-helper

## Task 2 — camel-ws: convert 44 REGISTRY_TEST_LOCK sites

Files:
- `crates/components/camel-ws/src/lib.rs` (modified)

Steps:
1. In the `#[cfg(test)]` module (near `static REGISTRY_TEST_LOCK` at ~line 2399), add to the existing imports:
   `use camel_component_api::test_support::{acquire_deadline, TEST_LOCK_DEADLINE};`
2. Replace every occurrence of
   `let _guard = REGISTRY_TEST_LOCK.lock().await;` (44 sites) with
   `let _guard = acquire_deadline(&REGISTRY_TEST_LOCK, "REGISTRY_TEST_LOCK (camel-ws ServerRegistry)", TEST_LOCK_DEADLINE).await;`
3. Do NOT modify any other logic; guard drop points stay identical.

Tests:
- name: zero-remaining-sites
  action: `rg -c 'REGISTRY_TEST_LOCK\.lock\(\)\.await' crates/components/camel-ws/src/lib.rs`
  assert: no matches (exit 1 from rg = zero sites).
- name: conversion-count
  action: `rg -cU 'acquire_deadline\(\s*&REGISTRY_TEST_LOCK' crates/components/camel-ws/src/lib.rs`
  assert: exactly 44.
- name: suite-green
  action: `cargo test -p camel-component-ws --lib`
  assert: exit 0 (the 44 lock-holding tests still pass locally).
  command: `cargo test -p camel-component-ws --lib`
  expected: passes after conversion.

Acceptance:
- Both rg checks hold; `cargo test -p camel-component-ws --lib` exits 0.
- `cargo fmt --check` clean; `cargo clippy -p camel-component-ws --all-targets -- -D warnings` exits 0 (all 44 sites are `#[cfg(test)]`; plain clippy would not compile them).
- Spec scenario covered: mechanical conversion preserves guard shape.

- [x] task-2-camel-ws

## Task 3 — camel-dsl: convert 23 SERVER_MUTEX sites

Files:
- `crates/camel-dsl/tests/rest_negotiation_e2e.rs` (modified, 15 sites)
- `crates/camel-dsl/tests/rest_stream_contract_e2e.rs` (modified, 8 sites)

Steps:
1. In each of the two test files add:
   `use camel_component_api::test_support::{acquire_deadline, TEST_LOCK_DEADLINE};`
   (dev-dependency with `test-support` already declared in `crates/camel-dsl/Cargo.toml` — no manifest edit).
2. Replace every
   `let _guard = SERVER_MUTEX.lock().await;` with
   `let _guard = acquire_deadline(&SERVER_MUTEX, "SERVER_MUTEX (dsl rest e2e)", TEST_LOCK_DEADLINE).await;`
3. No other logic changes.

Tests:
- name: zero-remaining-sites
  action: `rg -n 'SERVER_MUTEX\.lock\(\)\.await' crates/camel-dsl/tests/`
  assert: no matches.
- name: conversion-count
  action: `rg -cU 'acquire_deadline\(\s*&SERVER_MUTEX' crates/camel-dsl/tests/rest_negotiation_e2e.rs crates/camel-dsl/tests/rest_stream_contract_e2e.rs`
  assert: 15 and 8.
- name: suites-green
  action: run both e2e binaries (local axum servers, no infra):
  `cargo test -p camel-dsl --test rest_negotiation_e2e --test rest_stream_contract_e2e`
  assert: exit 0.
  expected: passes after conversion.

Acceptance:
- rg checks hold; both test binaries exit 0.
- `cargo fmt --check` clean; `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0.

- [x] task-3-camel-dsl

## Task 4 — camel-test: convert 27 sites (TEST_MUTEX 18, SENTINEL_TOPOLOGY_LOCK 6, TOPOLOGY_LOCK 3)

Files:
- `crates/camel-test/tests/http_static_test.rs` (modified, 18 sites)
- `crates/camel-test/tests/redis_repositories_test.rs` (modified, 6 sites)
- `crates/camel-test/tests/redis_sentinel_test.rs` (modified, 3 sites)

Steps:
1. In each file add:
   `use camel_component_api::test_support::{acquire_deadline, TEST_LOCK_DEADLINE};`
   (camel-test has camel-component-api as a REGULAR dependency with `test-support` enabled — no manifest edit).
2. `http_static_test.rs`: replace every
   `let _guard = TEST_MUTEX.lock().await;` with
   `let _guard = acquire_deadline(&TEST_MUTEX, "TEST_MUTEX (http_static)", TEST_LOCK_DEADLINE).await;`
3. `redis_repositories_test.rs`: replace every
   `let _guard = SENTINEL_TOPOLOGY_LOCK.lock().await;` with
   `let _guard = acquire_deadline(&SENTINEL_TOPOLOGY_LOCK, "SENTINEL_TOPOLOGY_LOCK (redis repositories)", TEST_LOCK_DEADLINE).await;`
4. `redis_sentinel_test.rs`: replace every
   `let _guard = TOPOLOGY_LOCK.lock().await;` with
   `let _guard = acquire_deadline(&TOPOLOGY_LOCK, "TOPOLOGY_LOCK (redis sentinel)", TEST_LOCK_DEADLINE).await;`

Tests:
- name: zero-remaining-sites
  action: `rg -n '(TEST_MUTEX|SENTINEL_TOPOLOGY_LOCK|TOPOLOGY_LOCK)\.lock\(\)\.await' crates/camel-test/tests/`
  assert: no matches.
- name: conversion-count
  action: `rg -cU 'acquire_deadline\(\s*&TEST_MUTEX' crates/camel-test/tests/http_static_test.rs` → 18;
  `rg -cU 'acquire_deadline\(\s*&SENTINEL_TOPOLOGY_LOCK' crates/camel-test/tests/redis_repositories_test.rs` → 6;
  `rg -cU 'acquire_deadline\(\s*&TOPOLOGY_LOCK[,\s]' crates/camel-test/tests/redis_sentinel_test.rs` → 3 (delimiter excludes SENTINEL_ prefixed matches).
- name: compile-all-three
  action: `cargo check -p camel-test --tests`
  assert: exit 0. (redis suites need infra at runtime — runtime verification deferred to CI; http_static needs the `integration-tests` feature to compile: `cargo check -p camel-test --features integration-tests --test http_static_test` exits 0.)
  expected: compiles after conversion.

Acceptance:
- rg checks hold (18/6/3 conversions, zero remaining).
- Both `cargo check` invocations exit 0.
- `cargo fmt --check` clean; `cargo clippy -p camel-test --all-targets -- -D warnings` exits 0.

- [x] task-4-camel-test

## Task 5 — camel-component-wasm: convert 3 ACK_TEST_LOCK sites

Files:
- `crates/components/camel-component-wasm/tests/source_bind_gate.rs` (modified)

Steps:
1. Add `use camel_component_api::test_support::{acquire_deadline, TEST_LOCK_DEADLINE};`
   (dev-dependency with `test-support` already present — no manifest edit).
2. Replace every
   `let _ack_guard = ACK_TEST_LOCK.lock().await;` (3 sites) with
   `let _ack_guard = acquire_deadline(&ACK_TEST_LOCK, "ACK_TEST_LOCK (wasm source bind)", TEST_LOCK_DEADLINE).await;`

Tests:
- name: zero-remaining-sites
  action: `rg -n 'ACK_TEST_LOCK\.lock\(\)\.await' crates/components/camel-component-wasm/tests/source_bind_gate.rs`
  assert: no matches.
- name: conversion-count
  action: `rg -cU 'acquire_deadline\(\s*&ACK_TEST_LOCK' crates/components/camel-component-wasm/tests/source_bind_gate.rs`
  assert: exactly 3.
- name: suite-compiles
  action: `cargo test -p camel-component-wasm --test source_bind_gate -- --list`
  assert: exit 0, converted tests listed (all `#[ignore]`-gated per ADR-0054 — runtime deferred to CI).
  expected: compiles and lists after conversion.

Acceptance:
- All three checks hold.
- `cargo fmt --check` clean; `cargo clippy -p camel-component-wasm --all-targets -- -D warnings` exits 0.

- [x] task-5-wasm

## Task 6 — Global sweep verification gate (runs LAST, after tasks 2–5)

Files: none (verification only; fixes belong back in the owning task)

Steps:
1. Workspace zero-site check — run exactly:
   `rg -n '(REGISTRY_TEST_LOCK|SERVER_MUTEX|TEST_MUTEX|SENTINEL_TOPOLOGY_LOCK|TOPOLOGY_LOCK|ACK_TEST_LOCK)\.lock\(\)\.await' crates/`
   Expected: no output (all six families converted).
2. Total conversion count — run exactly:
   `rg -cU 'acquire_deadline\(\s*&' crates/ --glob '!crates/components/camel-component-api/**' | awk -F: '{s+=$NF} END {print s}'`
   Expected: 97 (44+23+18+6+3+3; glob excludes the helper's own unit tests in camel-component-api).
3. Mission-238 boundary — run exactly:
   `git diff --name-only main...HEAD -- scripts/xtask/`
   Expected: empty (lint_unbounded_wait.rs and ratchet-unbounded-wait.max untouched).
4. Unbounded-wait gate — run exactly:
   `cargo xtask lint-unbounded-wait`
   Expected: exit 0 — findings decrease from the 393 ceiling toward 296 (97 lock sites leave the scan; the ceiling file itself is untouched).

Tests:
- name: global-sweep
  action: the four commands above, in order.
  assert: no output / 97 / empty diff / exit 0 respectively.
  command: as inlined in steps 1-4.
  expected: all four hold when tasks 1-5 are complete.

Acceptance:
- All four checks hold. Any failure routes back to the owning task (2=camel-ws, 3=camel-dsl, 4=camel-test, 5=wasm, 1=helper).
- Spec scenarios covered: lint sources and ratchet stay untouched; one helper used everywhere (97 conversions, zero remaining sites).

- [x] task-6-global-gate
