## ADDED Requirements

### Requirement: A failed single-mode statement never returns an open-transaction connection to the pool

The SQL producer SHALL roll back on the same pooled connection after any
single-mode statement error before that connection is released to the pool,
SHALL preserve the original statement error unchanged, and SHALL fail closed:
a successful rollback SHALL return the connection to the pool; benign
classification of a rollback ERROR is reserved for a database error whose
message is exactly SQLite's `cannot rollback - no transaction is active`, and
ANY other rollback error SHALL cause the connection to be discarded (detached
from the pool and closed). A benign rollback error SHALL be treated as
expected: the connection is kept for reuse. While cleanup is in flight, an
abrupt cancellation of the cleanup MUST also discard the connection rather
than return it to the pool.

#### Scenario: Failing manual BEGIN does not poison the pool

- **GIVEN** a SQLite pool with `max_connections=1` and a seeded items table
- **WHEN** one single-mode modify call runs `BEGIN IMMEDIATE; INSERT INTO items (id,name) VALUES (9,'x'); INSERT INTO items (id,name) VALUES (9,'y'); COMMIT;` and fails mid-transaction
- **THEN** the call returns Err, and a following call on the same pool completes within a 2-second bound with row 9 absent (the abandoned transaction was rolled back)

#### Scenario: Rollback failure discards the connection

- **GIVEN** a failed single-mode statement has left an open transaction on the acquired connection
- **WHEN** the rollback attempt on that connection fails with any error other than SQLite's benign "no transaction is active" result
- **THEN** the connection is detached from the pool and closed instead of being returned for reuse (fail closed), and the original statement error is preserved

#### Scenario: Cancellation during cleanup discards the connection

- **GIVEN** cleanup (the rollback attempt) has started on the connection after a statement failure
- **WHEN** the cleanup future is cancelled before the rollback resolves
- **THEN** the connection is discarded instead of being returned to the pool with unknown state

#### Scenario: Ordinary failure keeps its message and the pool usable

- **GIVEN** a plain failing statement with no manual BEGIN
- **WHEN** the statement fails
- **THEN** the original error message is preserved unchanged and the next call on the same pool succeeds
