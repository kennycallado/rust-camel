# Delta spec: sql-transactions — slice 2 (sqltrans-exec-verify)

ADDED requirements are copied word-for-word from the frozen family source
(openspec/changes/sqltrans/specs/sql-transactions/spec.md), requirements 2, 3, 4, 5, 8, 9, 10, 11, 12, 14.

The req-15 operational remainder (camel-sql operational note, docker-suite runtime
scenarios, idempotent-key disposition tests) is DELIVERED by this slice against the
already-canonical requirement 15 (openspec/specs/sql-transactions/spec.md) — it is
deliberately NOT restated or reworded here. S1-owned requirements (1, 6, 13, 15) and
S3-owned requirement 7 (Transaction mode semantics) are untouched.

## ADDED Requirements

### Requirement: Lazy begin and block-owned commit/rollback

The system SHALL begin the transaction lazily at the first joined sql step,
commit when the block body returns Ok or a clean `PipelineOutcome::Stopped`
(propagated unchanged per ADR-0025), and roll back on Err or cancellation;
the
owned transaction lives in the block's scope, not in exchange extensions. The
discard guard SHALL be armed before BEGIN is sent and disarmed only after an
acknowledged COMMIT or ROLLBACK; the session's discard guard is the sole
owner of the pooled connection and session state tracks transaction state
explicitly (`Fresh → Active → Finalizing → Done | Detached`), lending short
`&mut` borrows of the guarded connection per operation (BEGIN, joined-step
statements, COMMIT/ROLLBACK) — a borrowing `Transaction` is never stored
beside its owner; any sqlx `Transaction` wrapper is confined to a single
borrow scope. Cooperative cancellation SHALL await
a bounded rollback; a forced drop (task abort/panic) at any point SHALL
detach the connection (DiscardOnDrop). Dropping the sqlx
transaction object alone is not confirmation — finalization is an explicitly
awaited rollback (or commit) issued on the borrowed connection, and only its
acknowledged outcome may be reported; finalization (rollback and commit) is
bounded by the same 5 s finalize grace.

#### Scenario: outbox two-write commit

- **GIVEN** a route whose `transacted` block contains an UPDATE and an INSERT (transactional outbox)
- **WHEN** both sql steps succeed
- **THEN** both writes are visible after the block completes

#### Scenario: rollback under injected failure

- **GIVEN** the same outbox route with the second sql step forced to fail
- **WHEN** the block body returns Err
- **THEN** neither write is persisted and the exchange fails

#### Scenario: clean Stop commits and propagates

- **GIVEN** a `transacted` block whose first sql step writes a row and whose body ends with the Stop EIP (`PipelineOutcome::Stopped`)
- **WHEN** the block finalizes
- **THEN** the transaction commits, the written row persists, the Stop propagates unchanged, and steps after the block do not run

### Requirement: Cancellation cutoff

The observable cutoff is handing COMMIT to the driver after the final
cancellation check. Cancellation before the cutoff SHALL trigger a bounded
rollback (5 s grace). After the cutoff, the body deadline SHALL NOT trigger
rollback and the block SHALL resolve within the bound: an acknowledged COMMIT
yields Committed; a lost COMMIT acknowledgment SHALL be reported as
`CommitUncertain` and never as a rollback; a COMMIT rejected by the database
SHALL fail with a typed `CommitFailed`, and the connection SHALL be cleaned
up within the bound or detached — never returned to the pool with an active
transaction. A forced drop (the pipeline future
being dropped, e.g. by the inline dispatcher) cannot await rollback — the
armed DiscardOnDrop guard SHALL detach the connection. `CommitFailed` is
reserved for a definitive database rejection of COMMIT; a connection-fatal
response without a definitive outcome (e.g. PostgreSQL `FATAL 57P01`) is
`CommitUncertain`, detaches the connection, and issues no rollback. Named
fixtures SHALL
pin all paths.

#### Scenario: cancellation before COMMIT rolls back

- **GIVEN** a block whose cancellation arrives before the cutoff (fixture `transacted_cancel_before_commit`)
- **WHEN** the block unwinds
- **THEN** a bounded rollback completes within the 5 s grace and no partial write persists

#### Scenario: cancellation after the cutoff with a deterministic ACK

- **GIVEN** a block whose cancellation arrives after COMMIT was handed to the driver and whose COMMIT is acknowledged (fixture `transacted_cancel_after_commit_ack`)
- **WHEN** the commit resolution completes within the bound
- **THEN** the outcome is Committed and no ROLLBACK is issued

#### Scenario: cancellation after the cutoff with a lost ACK

- **GIVEN** a block whose cancellation arrives after COMMIT was handed to the driver and whose COMMIT acknowledgment is lost (fixture `transacted_cancel_after_commit_lost`)
- **WHEN** the commit resolution completes within the bound
- **THEN** the outcome is `CommitUncertain` naming the datasource and no ROLLBACK is issued or claimed

#### Scenario: COMMIT rejected by the database fails typed and cleans up

- **GIVEN** a block whose COMMIT returns a database error (fixture `transacted_commit_rejected`, a PostgreSQL `DEFERRABLE INITIALLY DEFERRED` foreign-key violation returning `23503`)
- **WHEN** the block finalizes
- **THEN** the exchange fails with typed `CommitFailed`, no write persists, and the connection is cleaned up within the bound or detached — never returned to the pool with an active transaction

#### Scenario: definitive COMMIT rejection maps to CommitFailed

- **GIVEN** a block whose transaction holds a write violating a PostgreSQL `DEFERRABLE INITIALLY DEFERRED` foreign key, so the write succeeds before COMMIT and COMMIT itself returns `23503` (fixture `transacted_commit_deferred_fk`)
- **WHEN** the block finalizes
- **THEN** COMMIT's definitive rejection maps to `CommitFailed` naming the datasource and the constraint error, and no row persists

#### Scenario: forced drop detaches via the guard

- **GIVEN** a block whose pipeline future is dropped mid-body (fixture `transacted_forced_drop_inline`, inline-dispatcher drop path)
- **WHEN** the armed discard guard drops with the connection still armed
- **THEN** the connection is detached (DiscardOnDrop) and the pool stays usable

### Requirement: Handled joined DB errors are rollback-only

After any joined step returns a database execution error, the session SHALL
be marked rollback-only BEFORE the error is returned to the pipeline, and
further joined SQL SHALL be rejected at admission (`SessionAborted`) without
touching the connection — a statement must never run on a server-aborted
transaction (SQLite autocommits statements after a trigger-rolled-back
transaction). A `do_try` catch inside the block may make the body
return Ok, but the block SHALL then fail with a typed
`AbortedByHandledError` and roll back — the server may have aborted the
transaction, and a commit path must never report Committed for a
server-aborted transaction.

#### Scenario: further joined SQL after a caught DB error is rejected at admission

- **GIVEN** a block where a joined step's duplicate-key DB error is caught by a `do_try` and a later joined sql step attempts to run
- **WHEN** the later step tries to join
- **THEN** the join is rejected with `SessionAborted` before any connection I/O, and neither the pre-error write nor the later attempt's write persists

#### Scenario: caught prepare error cannot produce Committed

- **GIVEN** a block that first writes a marker row successfully, then a joined step with a parameterized statement (non-empty binds, so the planner cannot skip probing) whose PREPARE fails with a database error, and a `do_try` catch that handles the flattened `ProcessorError`
- **WHEN** the block body ends Ok
- **THEN** the statement execution never started, the session is rollback-only, the block fails with `AbortedByHandledError`, and the marker row is absent (the failure classification is preserved before error mapping)

#### Scenario: do_try catching a joined DB error fails the block

- **GIVEN** a `transacted` block that inserts a marker row and a `do_try` whose try body causes a duplicate-key DB error on a joined step and whose catch handles it
- **WHEN** the block body ends Ok
- **THEN** the block fails with the typed `AbortedByHandledError`, the session rolls back, and the marker row is absent

### Requirement: Join semantics for sql steps on the block datasource

The system SHALL join every sql step on the block's datasource inside the
block into the same transaction — membership is the lexically baked scope, not
task or exchange membership — so `max_connections=1` does not self-deadlock,
concurrent exchanges SHALL hold separate transactions, and a sequential split
inside the block joins like any other in-block step. A `direct:` callee's sql
steps SHALL NOT inherit the session (scope is the block, not the call chain).
When an in-block dynamic routing target cannot prove membership at load, the
load SHALL fail closed. Transaction-control statements (BEGIN/COMMIT/ROLLBACK/
SAVEPOINT and dialect equivalents) and other transaction-ending or
implicit-commit statements in a joined step SHALL be rejected at load;
runtime SQL sources (header/body-provided statements) SHALL be rejected for
joined steps at load — a joined step's SQL is static, route-configured text —
and dialects whose transaction-ending semantics the loader cannot prove SHALL
be rejected at load (`DialectUnprovable` naming the unresolved URL scheme;
dialect identity comes from the URL scheme, never from the provider, which
only selects the connection factory). The
rejection variants are named — `RuntimeSqlInJoinedStep` (carrying the step),
`TransactionControlSql` (carrying the step and the offending statement),
`JoinSessionMissing` (datasource), and `JoinScopeMismatch` (datasource,
expected and found scope ids) — and each rejection has a named unit-tier
fixture asserting the exact variant and offender fields. A baked join whose
session is missing, or whose scope id is mismatched, at runtime SHALL fail
with the typed join error naming the datasource; it SHALL never auto-commit
and no write SHALL occur; the missing-session and mismatched-scope cases have
separate named fixtures, each asserting the exact variant, the datasource,
and zero writes. The `DialectUnprovable` fixture registers a
TransactionManager for a datasource with an unrecognized URL scheme
(e.g. `opaque://host/db`), so resolution cannot stop at `ManagerMissing`,
and asserts `DialectUnprovable` naming the unresolved scheme — the dialect
comes from the URL scheme, not the provider, so the fixture uses a fake
registered manager and an unrecognized URL scheme (e.g. `opaque://host/db`);
resolved non-PostgreSQL dialects use `UnsupportedJoinDialect` (MySQL
`mysql://`, SQLite `sqlite://`). Joined steps in v1 are
POSTGRESQL-ONLY: a joined step resolving to SQLite, MySQL, or any other
dialect whose transactional behavior cannot be proven from configuration
(MyISAM tables survive ROLLBACK; an attached SQLite database's journal mode
is invisible to configuration and `journal_mode=OFF` acknowledges ROLLBACK
while its writes persist) SHALL be rejected at load with
`UnsupportedJoinDialect` naming the resolved dialect; standalone MySQL,
SQLite, and manual-transaction usage outside blocks is unchanged. Provider
selects the
connection factory while the dialect comes from the URL scheme (one factory,
sqlx, serves several dialects), so dialect rejections key on the resolved
URL, not the provider alone. The `file:` SQL source is rejected for joined steps at load
alongside runtime
sources, so an edit after load can never change the SQL that executes. The
discard guard is armed on connection checkout; admission hygiene runs ONCE
before the first `BEGIN`: the session SHALL issue a hygiene ROLLBACK on
the borrowed connection — a no-op warning on a clean connection, a terminator
of any abandoned transaction — and if the hygiene is not acknowledged the
connection SHALL be detached without sending `BEGIN`;
a prior borrower's uncommitted writes are discarded (documented named
behavior), and a `max_connections=1` regression SHALL prove a block cannot
commit a prior borrower's write. A named fixture SHALL inject an observable
transport failure on the hygiene ROLLBACK before either deadline
(`transacted_hygiene_unacknowledged`) and assert exactly one
ROLLBACK attempt, no BEGIN, detachment, and the `RollbackUncertain` +
`Detached` outcome surfaced to the caller with the pool left usable. Hygiene
is bounded by the datasource acquire deadline; if that deadline expires
while hygiene is pending (fixture `transacted_hygiene_deadline_expiry` holds
the hygiene ROLLBACK pending and advances the virtual clock through the
acquire-first, block-first, and tie cases), the deadline error is primary —
`PoolExhausted` for acquire-first, `Timeout` for block-first and the tie —
and in all cases exactly one ROLLBACK attempt is made, BEGIN is never sent,
the connection is detached, and the pool stays usable.

#### Scenario: second step observes the first step's uncommitted write

- **GIVEN** a `transacted` block with an INSERT followed by a SELECT on the same datasource
- **WHEN** the SELECT runs before commit
- **THEN** it observes the uncommitted row (join proof)

#### Scenario: max_connections=1 does not self-deadlock

- **GIVEN** the datasource pool is configured with `max_connections=1` (test `sql_transacted_join_no_deadlock_maxconn1`, bounded to complete within 10 s)
- **WHEN** a route with two joined sql steps executes inside the block
- **THEN** the route completes without deadlock and BOTH writes persist (the join reused one connection)

#### Scenario: concurrent exchanges get separate transactions

- **GIVEN** two exchanges processed concurrently through the same transacted route, both transactions held open at a barrier (test `sql_transacted_concurrent_exchanges_distinct_pids`)
- **WHEN** both blocks execute
- **THEN** each reports a DISTINCT PostgreSQL backend PID (`pg_backend_pid()`) and no connection is shared

#### Scenario: sequential split inside a block joins

- **GIVEN** a `transacted` block containing a sequential-mode split whose fragments are sql steps on the block datasource
- **WHEN** the block executes
- **THEN** all fragment steps join the block's transaction

#### Scenario: direct callee does not inherit the session

- **GIVEN** a `transacted` block containing a `direct:` call whose target route runs a sql step on the block datasource outside any block
- **WHEN** the callee sql step runs with `transactionMode=Auto`
- **THEN** it executes on its own auto-commit connection, not the block's transaction

#### Scenario: unprovable dynamic target inside a block is rejected

- **GIVEN** a `transacted` block containing a dynamic-routing step whose target is resolved at runtime
- **WHEN** the route is loaded
- **THEN** the load fails closed with a typed error (membership cannot be proven)

#### Scenario: transaction-control statement in a joined step is rejected

- **GIVEN** a joined sql step whose statement text contains COMMIT, ROLLBACK, BEGIN, or SAVEPOINT
- **WHEN** the route is loaded
- **THEN** the load fails with the typed `TransactionControlSql` error naming the step and the offending statement

#### Scenario: runtime SQL source in a joined step is rejected

- **GIVEN** a joined sql step configured to take its statement from a header or body expression
- **WHEN** the route is loaded
- **THEN** the load fails with the typed `RuntimeSqlInJoinedStep` error naming the step (joined SQL must be static route text)

#### Scenario: missing session fails, never auto-commits

- **GIVEN** a compiled joined sql step executing with no block session on its task
- **WHEN** the step runs
- **THEN** `JoinSessionMissing` naming the datasource is returned, no write occurs, and it never falls back to auto-commit

#### Scenario: mismatched scope fails, never auto-commits

- **GIVEN** a compiled joined sql step executing with a block session whose scope id differs from the step's baked scope
- **WHEN** the step runs
- **THEN** `JoinScopeMismatch` naming the datasource and both scope ids is returned, no write occurs, and it never falls back to auto-commit

#### Scenario: unprovable dialect in a joined step is rejected

- **GIVEN** a `transacted` block containing a joined sql step on a datasource whose URL scheme is unrecognized (`opaque://host/db`) with a registered TransactionManager
- **WHEN** the route is loaded
- **THEN** the load fails with the typed `DialectUnprovable` error naming the unresolved scheme

#### Scenario: MySQL join is rejected for v1

- **GIVEN** a `transacted` block containing a joined sql step on a datasource with `provider = "sqlx"` and a `mysql://` URL
- **WHEN** the route is loaded
- **THEN** the load fails with the typed `UnsupportedJoinDialect` error naming MySQL (MyISAM writes survive ROLLBACK), while standalone MySQL steps outside blocks still load

#### Scenario: SQLite join is rejected for v1

- **GIVEN** a `transacted` block containing a joined sql step on a SQLite datasource
- **WHEN** the route is loaded
- **THEN** the load fails with the typed `UnsupportedJoinDialect` error naming SQLite (attached-database journal state is not provable; `journal_mode=OFF` survives ROLLBACK with writes persisting), while standalone SQLite steps outside blocks still load

#### Scenario: block cannot commit a prior borrower's abandoned write

- **GIVEN** `max_connections=1` and a pooled connection returned with an abandoned open transaction holding an uncommitted INSERT (prior borrower)
- **WHEN** a transacted block acquires that connection, passes admission hygiene, inserts its own row, and commits
- **THEN** the block's row persists, the prior borrower's row is absent (discarded by the hygiene ROLLBACK, never committed), the route completes without deadlock, and the regression asserts the same PostgreSQL backend PID for both phases so a replacement connection cannot satisfy it

#### Scenario: unacknowledged hygiene detaches without BEGIN

- **GIVEN** a pooled connection whose hygiene ROLLBACK fails with an injected observable transport failure before either deadline (fixture `transacted_hygiene_unacknowledged`)
- **WHEN** the block attempts admission
- **THEN** exactly one ROLLBACK attempt is made, BEGIN is never sent, the connection is detached, the caller observes `RollbackUncertain` with the `Detached` outcome naming the datasource, and the pool stays usable

#### Scenario: deadline expiry during pending hygiene selects the deadline error

- **GIVEN** a pooled connection whose hygiene ROLLBACK is left pending while the virtual clock advances through the acquire-first, block-first, and tie cases (fixture `transacted_hygiene_deadline_expiry`)
- **WHEN** each case's deadline elapses
- **THEN** `PoolExhausted` is returned for acquire-first, `Timeout` for block-first and the tie, with exactly one ROLLBACK attempt, BEGIN never sent, the connection detached, and the pool left usable

#### Scenario: file-based SQL source in a joined step is rejected

- **GIVEN** a joined sql step whose statement resolves from a `file:` source
- **WHEN** the route is loaded
- **THEN** the load fails with the typed `RuntimeSqlInJoinedStep` error naming the step (joined SQL is static route text; an edit after load can never change what executes)

### Requirement: Busy block session

The system SHALL admit at most one concurrent join per transaction session via
`try_lock`, returning a typed Busy error naming the datasource.

#### Scenario: concurrent join attempt on a held session returns Busy

- **GIVEN** a transaction session whose single execution slot is held
- **WHEN** another join is attempted on the same session
- **THEN** the attempt returns the typed Busy error naming the datasource

### Requirement: Block timeout

The system SHALL bound block execution with a timeout (default 30 s,
per-block override), rolling back and failing the exchange with the typed
Timeout error naming the datasource and elapsed time when it elapses. When a
deadline race could apply (block timeout versus datasource acquire timeout),
the deadline that elapses first wins; on an exact tie the block-timeout error
is reported. Deadlines are compared as absolute instants — not by
outer-timeout polling order. Finalization — rollback or commit — after the
deadline is
bounded by a fixed 5 s finalize grace; finalization that stalls past the
grace leaves Timeout as the primary error with `RollbackUncertain` and
`Detached` as cleanup context. If the body already
failed, the body error is primary and finalization uncertainty is carried in
the outcome, not substituted for it. Named fixtures SHALL cover both deadline
orderings and the tie with a deterministic virtual clock (injected timer):
`transacted_block_deadline_first` (block deadline 30 s, acquire deadline 60 s
→ `Timeout { datasource, elapsed }`), `transacted_acquire_deadline_first`
(acquire deadline 5 s, block deadline 60 s → `PoolExhausted { datasource,
max_connections, waited }`), `transacted_deadline_tie` (both 30 s →
`Timeout` wins), and `transacted_stalled_rollback` (rollback stalls past the
5 s grace → `Timeout` primary, `RollbackUncertain`, `Detached`), and
`transacted_stalled_commit` (successful body, COMMIT stalls past the 5 s
grace under the virtual clock → `CommitUncertain`, connection discard, no
rollback claim — the grace bound, distinct from the backend-termination
fixture); the
`CommitUncertain` backend-termination fixture is exercised in the Docker
suite by
terminating the backend at COMMIT from a server-side trigger
(`pg_terminate_backend` in a deferred trigger), so the client's COMMIT
acknowledgment is lost — distinct from a failure before COMMIT.

#### Scenario: block exceeding the timeout rolls back

- **GIVEN** a transacted block whose body exceeds the block timeout (injected `pg_sleep`)
- **WHEN** the timeout elapses
- **THEN** the transaction rolls back, the exchange fails with the typed Timeout error naming datasource and elapsed time, and no partial write persists

### Requirement: Uncertain finalization detaches the connection

The system SHALL discard (DiscardOnDrop) a connection whose rollback outcome
is uncertain — reporting the typed RollbackUncertain error and the Detached
outcome when cleanup is observable — and SHALL treat an unacknowledged COMMIT
as a failure carrying an explicit uncertain-commit error that does not claim
rollback was confirmed; in both cases the pool SHALL remain usable for later
exchanges.

#### Scenario: rollback failure leaves the pool usable

- **GIVEN** a block whose rollback cannot be acknowledged
- **WHEN** the rollback fails or times out
- **THEN** the session reports RollbackUncertain with the Detached outcome, the connection is discarded, and subsequent exchanges on the same pool succeed

#### Scenario: unacknowledged commit fails without claiming rollback

- **GIVEN** a block whose COMMIT acknowledgment is lost because the backend is terminated at COMMIT by a server-side deferred trigger (`pg_terminate_backend`)
- **WHEN** the block finalizes
- **THEN** the exchange fails with the typed CommitUncertain error naming the datasource, without asserting the transaction was rolled back, and the pool serves a later exchange on a different backend connection

#### Scenario: discard proof under max_connections=1

- **GIVEN** `max_connections=1` and a block that aborts after its first write
- **WHEN** the aborted block's connection is discarded and a later exchange runs on the same pool
- **THEN** no write from the aborted block persists, the pool serves the later exchange on a different backend connection, and the route completes

### Requirement: Pool exhaustion is bounded and named

The system SHALL bound transaction begin and connection acquisition by the
datasource `acquire_timeout` (sqlx default 30 s), surfacing the typed
PoolExhausted error that names the datasource, `max_connections`, and the
time waited.

#### Scenario: exhausted pool produces a typed named error

- **GIVEN** `max_connections=1` and one exchange holding the single connection inside a block
- **WHEN** a second exchange's block waits for a connection
- **THEN** within the acquire bound it fails with the typed PoolExhausted error naming datasource, max_connections, and waited time

### Requirement: Bind path invariance inside joined steps

The system SHALL keep `plan_binds` as the only bind path for sql steps —
batch planning SHALL route through one shared planner entry point that
accepts all batch shapes together (not per-shape calls into the single-shape
planner) without changing `sql-binds` whole-batch rules — and bind-kind
probing SHALL work over a transaction-scoped connection.

#### Scenario: joined step executes a parameterized statement via plan_binds

- **GIVEN** a joined sql step with typed parameters inside a `transacted` block
- **WHEN** the step executes
- **THEN** binds are planned by `plan_binds` against the transaction connection and the statement applies

#### Scenario: joined mixed-shape batch rolls back fully on a later bind failure

- **GIVEN** a block that writes a marker row, then a joined batch step whose DDL pins a `NUMERIC` target (`v NUMERIC`) and whose statement uses a variable-length `IN` list so describe cannot pin one shape (forcing the whole-batch shape scan); item 0 binds the float `1.5` at parameter 0 and item 1 binds the integer `9007199254740993` at parameter 0
- **WHEN** the batch executes inside a `transacted` block
- **THEN** `CamelError::ProcessorError` is returned naming batch item 1 and parameter 0, and neither the marker row nor any batch write persists (the whole block rolls back)

### Requirement: idempotent consumer composition

The system SHALL place the transacted block inside the `idempotent_consumer`
body; a block rollback SHALL fail the exchange. Key disposition follows
ADR-0099 exactly: with the eager default `remove_on_failure=true` the key is
removed on the child failure, permitting redelivery; with an explicit
`remove_on_failure=false` the reservation is retained (at-most-once, with the
load-time warning ADR-0099 already requires); when removal itself fails the
key enters the pending-set single-owner recovery path; with
`on_missing_key: process` and an absent key no reservation exists, so the
failure path performs no repository add or remove. Each disposition SHALL be
tested separately. The window between DB commit and idempotent-key confirm is an
inherent at-least-once window: outbox rows SHALL carry a unique message id and
the relay/downstream SHALL deduplicate.

#### Scenario: rollback releases the idempotent key (eager default)

- **GIVEN** an `idempotent_consumer` with default `remove_on_failure=true` whose body wraps a transacted block that rolls back
- **WHEN** the block rolls back
- **THEN** the exchange fails, the idempotent key is removed, and redelivery is possible

#### Scenario: explicit retain-on-failure keeps the reservation

- **GIVEN** an `idempotent_consumer` with explicit `remove_on_failure=false` whose body wraps a transacted block that rolls back
- **WHEN** the block rolls back
- **THEN** the exchange fails and the reservation is retained (at-most-once), with the ADR-0099 load-time warning present

#### Scenario: failed removal enters pending recovery

- **GIVEN** an `idempotent_consumer` whose block rolled back and whose key removal fails
- **WHEN** the removal failure is recorded
- **THEN** the key enters the pending-set single-owner recovery path and a raced redelivery fails loudly per ADR-0099

#### Scenario: process mode with absent key performs no repository write

- **GIVEN** an `idempotent_consumer` with `on_missing_key: process`, an absent key, and a transacted block body that rolls back
- **WHEN** the child fails
- **THEN** the exchange fails, the key stays absent, and no repository add or remove occurs on the failure path

#### Scenario: at-least-once window is documented

- **GIVEN** the operational documentation for transacted blocks
- **WHEN** a DB commit succeeds but the idempotent key confirm is not yet recorded
- **THEN** the documentation specifies the unique outbox message id and relay/downstream deduplication as the required mitigation

