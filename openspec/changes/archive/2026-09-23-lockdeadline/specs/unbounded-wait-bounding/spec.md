# Delta: unbounded-wait-bounding

## ADDED Requirements

### Requirement: Deadline-bounded global test-lock acquisition

Every acquisition of a process-global test-serialization mutex
(`REGISTRY_TEST_LOCK`, `SERVER_MUTEX`, `TEST_MUTEX`,
`SENTINEL_TOPOLOGY_LOCK`, `TOPOLOGY_LOCK`, `ACK_TEST_LOCK` — empty
`tokio::sync::Mutex<()>` statics whose guards are held for the
duration of a test) SHALL acquire through the shared
`camel_component_api::test_support::acquire_deadline` helper. All
inventoried sites convert; this requirement admits no
`allow-test-wait` adjudication for these six locks.

#### Scenario: Mechanical conversion preserves guard shape

- **GIVEN** a test body containing `let _guard = LOCK.lock().await;`
- **WHEN** the site is converted
- **THEN** the site reads `let _guard = acquire_deadline(&LOCK, "<LOCK> <family>", TEST_LOCK_DEADLINE).await;` (helper imported from `camel_component_api::test_support`), and the guard's drop semantics are unchanged

#### Scenario: Stalled holder fails the waiting test, not the binary

- **GIVEN** a peer test holds the lock and stalls (deadlock or
  crashed peer under the guard)
- **WHEN** a queued test's acquisition exceeds the deadline
- **THEN** the waiting test panics with a message naming the lock,
  the acquisition site, and the deadline, instead of parking
  indefinitely

#### Scenario: Deadline exceeds worst healthy queue, not one holder

- **GIVEN** lock families whose tests serialize on one mutex inside
  a single test binary (44/23/18/6/3/3 holders), with worst-case
  healthy holder chains up to 602 s under load
- **WHEN** the deadline is chosen
- **THEN** a single uniform `TEST_LOCK_DEADLINE` of 900 s covers
  every family's worst-case healthy queue with ~1.5× margin, so a
  timeout indicates a stalled holder, not queue depth

#### Scenario: One helper, no per-crate reimplementations

- **WHEN** any crate needs bounded test-lock acquisition
- **THEN** it uses `camel_component_api::test_support::acquire_deadline`; no parallel helper is introduced in any crate

#### Scenario: Lint sources and ratchet stay untouched

- **WHEN** the 97 inventoried sites are converted
- **THEN** `scripts/xtask/src/lint_unbounded_wait.rs` and
  `scripts/xtask/ratchet-unbounded-wait.max` are not modified
  (the drainscope mission owns them), and
  `cargo xtask lint-unbounded-wait` findings only decrease

#### Scenario: Helper timeout path is observable and sleep-free

- **GIVEN** the helper's unit tests with a spawned holder that
  acquires the lock and awaits a pending future
- **WHEN** `acquire_deadline` with a short deadline is called
- **THEN** it panics naming the lock, the deadline, and the
  acquisition call site (verified by capturing the panic payload),
  and the tests use no `tokio::time::sleep` (lint-test-sleep clean)
