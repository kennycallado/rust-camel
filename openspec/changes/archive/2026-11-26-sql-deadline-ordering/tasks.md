# Tasks: sql-deadline-ordering

## camel-test

### Task 1: Land the five production-tier deadline-ordering fixtures

**Files:**
- `crates/camel-test/Cargo.toml` (modified) — add the narrow `integration-sql`
  feature: `integration-sql = ["dep:camel-component-sql", "dep:sqlx"]` under
  `[features]`, keeping the existing `integration-tests-cxf` row untouched.
- `crates/camel-test/tests/sql_deadline_ordering.rs` (new) — the fixture file
  already drafted in this worktree is the reference; the worker verifies it
  matches the contract below and fixes any drift (the feature row and the
  file are present from the probe phase — verify, don't re-create).

**Steps:**
1. Add the `integration-sql` feature to `crates/camel-test/Cargo.toml` exactly
   as above (no other manifest edits; `camel-component-sql` and `sqlx` are
   already declared optional dependencies).
2. In `crates/camel-test/tests/sql_deadline_ordering.rs`, keep the module
   attribute `#![cfg(feature = "integration-sql")]` directly after the module
   doc, and the five `#[tokio::test]` fixtures with the mission-fixed names
   below. Each fixture: a real-clock setup (`held_pool()` — one-connection
   in-memory SQLite pool with the only connection held by the test), then
   `tokio::time::pause()`, then `spawn_block(&pool, block, acquire).await`
   (spawns `TransactedSegment::new("pg", Arc::new(SqlTransactionManager::new(
   "pg", FixedCatalog)).with_acquire_deadline(acquire), OutcomeSegment(NoopBody))
   .with_block_timeout(block)` and yields once so block entry is captured
   pre-advance), then one `tokio::time::advance`, then assert the
   `TransactionError` variant on `PipelineOutcome::Failed(CamelError::Transaction(..))`.
3. Fixture matrix (exact names, deadlines, advance, expected primary):
   - `earlier_admission_deadline_reports_pool_exhausted` — block 30 ms,
     acquire 10 ms, advance 11 ms → `PoolExhausted { datasource: "pg",
     max_connections: 1, waited_ms: 10..=11 }`.
   - `earlier_block_deadline_reports_timeout` — block 30 ms, acquire 60 s,
     advance 31 ms → `Timeout { datasource: "pg", elapsed_ms: 30..=31 }`.
   - `exact_tie_reports_block_timeout` — block 30 ms, acquire 30 ms, advance
     31 ms → `Timeout { elapsed_ms: 30..=31 }`.
   - `both_ready_after_block_deadline_reports_block_timeout` — block 30 ms,
     acquire 40 ms, advance 50 ms → `Timeout { elapsed_ms: 50..=51 }` (the
     strict block-first half of the `block_abs <= acquire_abs` guard).
   - `pending_open_does_not_outlive_block_deadline` — block 30 ms, acquire
     60 s, advance 31 ms → `Timeout { elapsed_ms: 30..=31 }`, then
     `SELECT 1` on the held connection succeeds (the pending open never
     checked a connection out).
4. Run `cargo fmt -p camel-test` (the drafted file must satisfy
   `cargo fmt --check`).
5. Keep every documentation claim aligned with the code (no `start_paused`
   wording — the tests use `pause()` + explicit `advance`; no sub-millisecond
   skew claims; the `pending_open` probe wording names the held connection).

**Tests:** (executable spec — name, arrange, act, assert)
- `earlier_admission_deadline_reports_pool_exhausted`: exhausted pool (held
  connection) → run block(30 ms)/acquire(10 ms), advance 11 ms → the failed
  outcome carries `CamelError::Transaction(PoolExhausted { datasource: "pg",
  max_connections: 1, waited_ms: 10..=11 })`.
- `earlier_block_deadline_reports_timeout`: exhausted pool → run
  block(30 ms)/acquire(60 s), advance 31 ms → `Timeout { datasource: "pg",
  elapsed_ms: 30..=31 }`.
- `exact_tie_reports_block_timeout`: exhausted pool → run
  block(30 ms)/acquire(30 ms), advance 31 ms → `Timeout { elapsed_ms:
  30..=31 }` (never `PoolExhausted`).
- `both_ready_after_block_deadline_reports_block_timeout`: exhausted pool →
  run block(30 ms)/acquire(40 ms), advance 50 ms → `Timeout { elapsed_ms:
  50..=51 }` (never `PoolExhausted`).
- `pending_open_does_not_outlive_block_deadline`: exhausted pool → run
  block(30 ms)/acquire(60 s), advance 31 ms → `Timeout { elapsed_ms:
  30..=31 }`; then `SELECT 1` on the held connection returns 1.
- command: `cargo test -p camel-test --features integration-sql --test
  sql_deadline_ordering` — expected after this task: 5 passed, 0 failed.

**Acceptance:**
- `cargo test -p camel-test --features integration-sql --test
  sql_deadline_ordering` exits 0 with `5 passed`.
- `cargo fmt --check --all` exits 0.
- `cargo clippy -p camel-test --features integration-sql --all-targets -- -D warnings` exits 0.
- `cargo xtask lint-publish-cycles` exits 0 (no new camel-* dev/build edge).
- `git diff --quiet crates/camel-core/src/transacted_segment.rs` is true
  (no production change).

- [x] 1

### Task 2: Wire the suite — lanes (three trigger crates), lane-script argv, CI step

**Files:**
- `AGENTS.md` (modified) — in the `## COMPONENT LANES` fenced block, directly
  after the `camel-core | local | cargo test -p camel-core --test
  hexagonal_architecture_boundaries_test` row, add the block:
  ```
  # --- rc-9awnc deadline-ordering suite (mission 618): rides camel-test (ADR-0055
  #     leaf sink — sql must not dev-depend on core); triggered by every crate
  #     that holds the code under test (duplicate-owner precedent, kernel_fail_closed)
  camel-test | local | cargo test -p camel-test --features integration-sql --test sql_deadline_ordering
  camel-core | local | cargo test -p camel-test --features integration-sql --test sql_deadline_ordering
  camel-component-sql | local | cargo test -p camel-test --features integration-sql --test sql_deadline_ordering
  ```
- `scripts/gates-lanes-test.sh` (modified) — the new camel-test row runs first
  for a camel-test trigger, so prepend
  `'test -p camel-test --features integration-sql --test sql_deadline_ordering'$'\n'`
  to the three `assert_argv_exact 'test -p camel-test ...'` lists at lines
  2114 (case 49a), 2133 (49b), and 2231 (49f); in case 49f change
  `STUB_TEST_FAIL_AT=2` to `STUB_TEST_FAIL_AT=3` (the first failure is now the
  `--no-run` row; the first SUITE is the third invocation, which keeps the
  `FAIL lane camel-test :: suites=` aggregation assert valid).
- `.github/workflows/ci.yml` (modified) — directly after the
  `Compile-gate narrow cxf integration feature` step, add a narrow step
  (name `Test narrow sql integration feature (rc-9awnc deadline ordering)`)
  with `run: cargo test -p camel-test --features integration-sql --test
  sql_deadline_ordering` and a comment noting it is test-only, sqlite-backed,
  virtual-time, and Docker-free.

**Steps:**
1. Add the three lane rows to `AGENTS.md` exactly as above (the parser splits
   on the first two `|`; several rows per crate are normal).
2. Patch the three `assert_argv_exact` lists and the 49f failure index in
   `scripts/gates-lanes-test.sh` as described.
3. Add the CI step after the narrow cxf gate in `.github/workflows/ci.yml`.
4. Run the lane self-test and the lane sweep (see Tests).

**Tests:** (executable spec)
- `gates_lanes_test` (script): setup — the three rows are present in the
  AGENTS.md fenced block and the three argv lists lead with the sql command →
  action — `bash scripts/gates-lanes-test.sh` → assert exit 0 and
  `50/50 cases green`.
- `lane_sweep`: action — `GATES_ONLY=lanes bash scripts/gates-local.sh` →
  assert exit 0 and the run log contains
  `cargo test -p camel-test --features integration-sql --test
  sql_deadline_ordering`. Note: the sweep also runs the heavy pre-existing
  camel-test rows (`--no-run` compile and the two `--workspace`
  dev_watch suites); expect minutes-scale runtime.

**Acceptance:**
- `bash scripts/gates-lanes-test.sh` exits 0 with `50/50 cases green`.
- `GATES_ONLY=lanes bash scripts/gates-local.sh` exits 0 and executes the new
  row.
- `git diff AGENTS.md` shows exactly the three-row block and no other lane
  edits.

- [x] 2

### Task 3: Record the mutation matrix (pre-fix red, guarded green)

**Files:**
- No source files change. The guarded site is
  `crates/camel-core/src/transacted_segment.rs`: the
  `Err(e @ TransactionError::PoolExhausted { .. })` arm guarded by
  `if tokio::time::Instant::now() >= block_abs` (lines 192–203); its body's
  comparison `if block_abs <= acquire_abs {` is the ONLY `<=` in code (same
  text appears in comments at lines 180 and 1237 — line-addressed edits only).
- Evidence lands in the mission park report (the worker returns the captured
  output; the conductor records it).

**Steps:**
1. Confirm the guarded suite is green: `cargo test -p camel-test --features
   integration-sql --test sql_deadline_ordering` → 5 passed.
2. Check the target line: `sed -n '196p' crates/camel-core/src/transacted_segment.rs`
   must print `                    if block_abs <= acquire_abs {`.
3. Mutation (b) strict-first: run
   `sed -i '196s/if block_abs <= acquire_abs {/if block_abs == acquire_abs {/' crates/camel-core/src/transacted_segment.rs`,
   run the suite, capture the failure line, then restore with
   `git checkout -- crates/camel-core/src/transacted_segment.rs`.
4. Mutation (a) pre-fix shape: run
   `sed -i '192,203d' crates/camel-core/src/transacted_segment.rs`, run the
   suite, capture the failure lines, then restore with
   `git checkout -- crates/camel-core/src/transacted_segment.rs`.
5. After each restore: `git diff --quiet crates/camel-core/src/transacted_segment.rs`
   must return 0 before proceeding.
6. Final: re-run the suite → 5 passed.

**Tests:** (executable spec — expected mutation outcomes)
- mutation (b) `<=` → `==`: `both_ready_after_block_deadline_reports_block_timeout`
  FAILS with the exact line
  `expected typed Timeout for a ready PoolExhausted after the block deadline, got PoolExhausted { datasource: "pg", max_connections: 1, waited_ms: 50 }`;
  the other four pass (the tie branch stays correct under `==`).
- mutation (a) guard removed: `exact_tie_reports_block_timeout` FAILS with
  `expected typed Timeout on the exact tie, got PoolExhausted { datasource: "pg", max_connections: 1, waited_ms: 31 }`
  and `both_ready_after_block_deadline_reports_block_timeout` FAILS with
  `expected typed Timeout for a ready PoolExhausted after the block deadline, got PoolExhausted { datasource: "pg", max_connections: 1, waited_ms: 50 }`;
  the other three pass. An `unused variable: open_start` warning is expected
  (the removed arm was its only user).
- restored guard: `cargo test -p camel-test --features integration-sql --test
  sql_deadline_ordering` → 5 passed; guard file byte-identical to the branch merge-base (no production edits).

**Acceptance:**
- Both mutation runs captured with the exact expected failure lines above.
- `git diff --quiet crates/camel-core/src/transacted_segment.rs` returns 0.
- Final suite run: 5 passed.
- `cargo xtask lint-unbounded-wait`, `cargo xtask lint-test-sleep`, and
  `cargo xtask lint-publish-cycles` each exit 0.

- [x] 3
