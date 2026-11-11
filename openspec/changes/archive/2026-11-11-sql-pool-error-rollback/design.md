# Design: sql-pool-error-rollback

## Approach

Single-mode execution holds one `PoolConnection<Any>` while it plans binds,
binds parameters, and executes. On success the connection is dropped and
returns to the pool unrolled — correct, because no transaction is open.
On any error the same drop releases a connection that may sit inside a
user-opened transaction (manual `BEGIN`), poisoning the pool (bd rc-en1ud).

Fix: an owning release helper on the error path, fail-closed and
cancellation-safe.

```rust
async fn release_after_error(conn: PoolConnection<Any>, original: CamelError) -> CamelError
```

While cleanup is in flight the connection must never be able to slip back to
the pool: dropping the helper's future mid-rollback (route timeout, stream
cancel) would otherwise release the ordinary `PoolConnection` — and sqlx 0.9
returns it to the pool on Drop (`pool/connection.rs` `return_to_pool`).
The helper therefore wraps the connection in a discard-on-drop guard:

```rust
struct DiscardOnDrop(Option<PoolConnection<Any>>);
// Drop impl: if Some, detach() then drop — closed, never re-pooled.
```

The guard disarms only after the rollback resolves as successful or benign
(`take()` then plain drop → normal pool return); cancellation drops the
guard armed. The guard is exercised directly by a unit test (armed drop
shrinks the pool; disarmed drop keeps it).

The rollback executes `ROLLBACK` on the SAME connection (via
`sqlx::query(sqlx::AssertSqlSafe("ROLLBACK"))`, matching the file's existing
SQL-safety idiom), then returns `original` unchanged in every case. The
classifier is fail-closed and matches the DATABASE error layer only:

- rollback succeeds → connection is clean, disarm, return to the pool;
- rollback fails with a DATABASE error whose message is EXACTLY SQLite's
  `cannot rollback - no transaction is active` (via `as_database_error()`,
  unwrapping `AnyDriverError` where the driver wraps it) → the ordinary
  case after a failed statement without an open transaction: log at debug,
  disarm, keep the connection;
- ANY other rollback result — protocol errors, IO errors, database errors
  with different or padded messages — means unknown state: log at warn,
  drop the guard armed — `detach()` closes the connection. It can never be
  reused poisoned.

The decision is pure and unit-testable: `classify_db_message(&str)` does the
exact-message match (positive fixture: the benign string; negative fixtures:
empty, suffixed, and unrelated messages), and `classify_rollback_error`
maps non-database errors (e.g. a `Protocol` error whose text merely contains
the phrase) to discard. The positive database shape is additionally proven
end-to-end by the ordinary-failure regression test asserting Keep on the
real SQLite error.
Callers restructure their error exits from `?`-propagation into
acquire-then-match so the connection is available to move into the helper:

- `execute_modify`: wrap plan + bind + execute in one fallible block; on
  Err, `release_after_error(conn, e)`. The `expected_update_count`
  mismatch return also routes through the helper (its rollback resolves as
  benign when no transaction is open).
- `fetch_single_rows`: same wrap around plan + bind + `fetch_all`.
- `execute_select` StreamList branch: the acquire stays as-is (no
  connection to release on acquire failure); plan, bind, row-iteration, and
  serialization errors each drop the row stream first (it borrows the
  connection), then release through the helper.
- Success paths keep today's drop-to-pool behavior: no rollback round trip.

Original error strings are preserved exactly: the helper returns the caller's
already-mapped `CamelError`; a rollback error is never substituted.

Probe/prepare failure messages ("SQL query prepare failed: {e}") and
`fetch_error` texts stay as today apart from the rollback attempt.

Batch (`execute_batch`) is untouched: it already uses `pool.begin()`/`tx`
and sqlx rolls back on drop.

## Test coverage map

- `failing_manual_begin_does_not_poison_the_pool` (end-to-end, red first):
  the poison scenario, `max_connections=1`, next call bounded at 2 s.
- `failed_statement_without_transaction_keeps_error_and_pool_usable` +
  retention: error message preserved; check `pool.size() == 1` IMMEDIATELY
  after the failing call, BEFORE any follow-up query (a later acquisition
  could otherwise mask a discard by creating a replacement), which also
  asserts Keep on the real SQLite no-active-transaction error; then a
  follow-up call proves the pool usable.
- `failing_select_keeps_error_and_pool_usable`: materialized SELECT error
  path (fetch_single_rows wiring): SELECT from a nonexistent table → Err
  with the original message, `pool.size() == 1` immediately after, follow-up
  select Ok.
- `classify_db_message_is_fail_closed` + `rollback_error_classifier_rejects_non_database_errors`
  (pure unit tests): `classify_db_message` — the exact benign string keeps;
  empty, suffixed, and unrelated messages discard; `classify_rollback_error` —
  a non-database error whose display merely contains the benign phrase
  (e.g. `sqlx::Error::Protocol`) discards.
- `cleanup_guard_discards_when_armed_keeps_when_disarmed` (unit test):
  armed guard drop detaches+closes (`pool.size()` drops to 0); disarmed
  drop returns the connection (`pool.size()` stays 1).
- StreamList error cleanup: a lazy stream whose execution errors during
  consumption releases the connection through the helper; the pool stays
  usable (next call Ok). Test uses the open-transaction variant
  (`BEGIN IMMEDIATE; INSERT (9,'x'); select ...; INSERT dup`) — sqlx-sqlite
  0.9 runs all `;`-separated statements mid-stream, so the error is a real
  UNIQUE failure inside an open transaction; unwired code shows ROW_COUNT 3
  (uncommitted row visible on same-connection reuse), wired code shows 2.
- A non-benign rollback failure against a LIVE connection is not
  deterministically forceable with the current harness (SQLite rollback on
  a healthy connection does not fail; no fault-injection seam exists). The
  discard path is covered by the classifier + guard unit tests; the
  limitation is recorded in `parked-report.md` (task 2.1).

## Affected crates

- `camel-sql`: `src/producer.rs` (guard + classifier + release helper,
  three single-mode error paths), `src/producer.rs` tests (three
  non-StreamList end-to-end regression tests, classifier and guard unit
  tests, StreamList cleanup test). No public API change.

## Architecture boundaries

Component-internal (Components boundary). No Runtime, DSL, Services,
Languages, or Functions changes. Error taxonomy unchanged — the component
still emits `CamelError::ProcessorError` with the same messages (ADR-0012
handler-owned logging for the new rollback warn/debug sites). The data
plane (Exchange body/header content) is unaffected on both success and
error paths.

## Alternatives considered

- `PoolOptions::after_release` hook rolling back on every release —
  rejected: it taxes the hot success path with a round trip per statement
  release, it has no access to the statement error (so it cannot preserve
  or classify failure context), and it cannot tell "needs rollback" apart
  without the same round trip anyway.
- Wrap every single-mode execution in `pool.begin()` like the batch path —
  rejected: a user-issued `BEGIN` inside sqlx's own transaction errors
  ("cannot start a transaction within a transaction"), changing supported
  SQL; plus an extra begin/commit round trip on every success.
- `sqlx::TransactionManager::rollback` instead of a raw `ROLLBACK`
  statement — rejected: driver-uniform raw ROLLBACK is simpler, and these
  connections never carry sqlx-managed transaction state to desync.
- Release hook + `should_close()` heuristic — rejected: cannot detect an
  open transaction portably; `test_before_acquire` already fails at this.
