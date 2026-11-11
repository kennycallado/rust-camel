# Proposal: sql-pool-error-rollback

## Why

A failed single-mode SQL statement can return a pooled connection with an
open transaction to the pool (bd rc-en1ud, confirmed on main 3a28ec959).
Single-mode execution acquires one pooled connection and runs the statement
directly (`execute_modify`, `fetch_single_rows`, and the StreamList acquire
site in `execute_select`). On error the connection drops back into the pool
unrolled: sqlx 0.9 rolls back on drop only for `sqlx::Transaction`, never
for a released `PoolConnection`, and the pool's `test_before_acquire` ping
succeeds inside an open transaction, so it does not detect the state.

Repro (demo team, SQLite, RC-46): one `sql:` call containing
`BEGIN IMMEDIATE; INSERT ...; INSERT duplicate; COMMIT` fails at the
duplicate insert. With `max_connections=1` the next call acquires the same
connection inside the still-open transaction and blocks until the acquire
timeout.

The batch path is already safe (`pool.begin()` + `tx.commit()`, rollback on
drop) and stays untouched.

## What Changes

- Add a private release helper in `crates/components/camel-sql/src/producer.rs`
  that, on any single-mode error, rolls back on the SAME pooled connection
  before release and returns the original error untouched.
- A benign "no transaction is active" rollback result is logged at debug and
  the connection is kept. Any other rollback failure detaches the connection
  from the pool and closes it, so it can never be reused poisoned.
- Apply the helper at every single-mode error site: `execute_modify`
  (including the `expected_update_count` mismatch return), `fetch_single_rows`,
  and the StreamList branch of `execute_select`. Success paths gain no
  rollback round trip.
- Spec delta: ADD to `openspec/specs/sql-pool/` the requirement that a failed
  single-mode statement never returns a connection with an open transaction
  to the pool.
- One paragraph in `docs/src/components/database.md`: manual multi-statement
  transactions are safe against mid-statement failures because the component
  rolls back on its own connection; `transactionMode=Managed` is still
  pending (rc-qv2iz) and is not documented as working.

Excluded: `transactionMode=Managed` (rc-qv2iz), batch-path changes, new
public API.

## Acceptance criteria

- Regression test `failing_manual_begin_does_not_poison_the_pool`
  (`max_connections=1`): failing call returns Err, next call completes
  within 2 s, and the inserted row is absent (rolled back).
- Regression test `failed_statement_without_transaction_keeps_error_and_pool_usable`:
  plain failing statement preserves its error message, the pool stays usable,
  and the connection is RETAINED (`pool.size() == 1` — kept, not discarded).
- The rollback classifier is unit-tested fail-closed: the benign SQLite
  "no transaction is active" result keeps the connection; any other rollback
  error dispositions discard. The discard-on-drop cleanup guard is
  unit-tested: armed drop detaches+closes (pool shrinks), disarmed drop
  returns the connection (pool size unchanged).
- Regression test `failing_select_keeps_error_and_pool_usable`:
  materialized SELECT from a nonexistent table preserves the error, keeps
  the connection (`pool.size() == 1`), and the pool stays usable.
- StreamList error cleanup is covered by a test whose lazy stream errors
  during consumption, retains the connection (`pool.size() == 1`), and
  leaves the pool usable afterwards.
- A non-benign rollback failure cannot be deterministically forced against a
  live SQLite connection with the existing harness (no fault-injection seam);
  the discard decision is therefore covered by the classifier and guard unit
  tests, and the limitation is recorded in the park report.
- `cargo test -p camel-sql` green; `cargo fmt --check --all` green;
  `bash scripts/gates-local.sh` exit 0.

## Risk budget

- In bounds: producer.rs error-path restructuring, one helper function, two
  regression tests, spec delta, one docs paragraph.
- Out of bounds: batch path, pool factory, public API, Managed transaction
  mode, driver-specific workarounds.

Affected crates: `camel-sql`. Docs: `docs/src/components/database.md`.
Bd: rc-en1ud (P1 bug).
