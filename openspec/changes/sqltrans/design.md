# Design: sqltrans

## Ruling (design input, e_opus 2026-11-15 — restated, do not re-litigate)

Surface (b): explicit transaction block `transacted { datasource: X }`, not
per-exchange. A transaction's scope and commit point are route semantics, so
they must be lexically visible in the route and must not be inferred from
exchange completion. Rejected (a) per-exchange: it holds a pooled connection
across every later step (including slow HTTP), clones share it invisibly, and
core completion would be coupled to a component resource. Rejected (c)
per-endpoint: insufficient for the outbox. Hooks: none — the block owns
begin/commit/rollback. Sequential split may join; parallel split/multicast/
wiretap/async/seda inside a block are REJECTED AT LOAD when they reach a joined
sql step; the handle uses `try_lock` and returns a typed error when busy.
Load errors also: StreamList inside a block, sql step on another datasource
(no 2PC). Scope is the block, not the route. Held: an owned sqlx
`Transaction`, begun lazily at the first sql step on X inside the block,
committed when the body returns Ok, rolled back on Err/cancel/timeout;
uncertain rollback detaches the connection (DiscardOnDrop). Every sql step on
X inside the block joins — this avoids self-deadlock at `max_connections=1`.
Managed becomes "join required": a Managed step outside a block is an error.
Pool exhaustion bounded by the datasource `acquire_timeout` (default 30 s),
typed error names datasource, `max_connections` and time waited; block timeout
default 30 s. `idempotent_consumer`: block goes INSIDE the idempotent body; a
rollback fails the exchange and `remove_on_failure` + the pending set
(`5301460da`) release the key. DB commit to key confirm is an inherent
at-least-once window: outbox rows need a unique message id and the relay/
downstream must dedupe. (ADR-0099 precision on the key-disposition claim:
with the eager default `remove_on_failure=true` a rollback removes the key;
an explicit `false` retains the reservation with the load-time warning; a
failed removal enters pending-set single-owner recovery; `on_missing_key:
process` with an absent key reserves nothing on the failure path.) Needs its
own ADR and a `TransactionManager` port in camel-api; refactors every
producer executor onto a borrowed connection.

## Approach

- **Port** (`crates/camel-api/src/transaction.rs`): object-safe
  `TransactionManager` (`datasource()`, `begin() -> Arc<dyn TransactionSession>`)
  and `TransactionSession` (`try_join() -> Result<TransactionJoin, TransactionError>`
  single-admission; `commit()`; `rollback() -> TransactionOutcome`
  {Committed, RolledBack, Detached}). Typed `#[non_exhaustive] TransactionError`
  and `#[non_exhaustive] TransactionOutcome`, full public rustdoc (ADR-0049):
  Busy, PoolExhausted{datasource, max_connections, waited}, Timeout, JoinRequired,
  ForeignDatasource, ParallelInsideBlock, StreamListInsideBlock, NestedBlock,
  UnnamedDatasource, ManagerMissing, RollbackUncertain, CommitUncertain,
  DynamicTargetUnprovable, TransactionControlSql — surfaced via a new
  `CamelError::Transaction` variant. Follows the ADR-0023 port pattern
  (Contract C1: failures are `Err(CamelError)`, never silent). Re-exported from
  `camel-api` `lib.rs`.
- **Discard-guard ownership and session state**: the discard guard is the
  SOLE owner of the pooled `PoolConnection` and lives in session state —
  NEVER a borrowing `Transaction` stored beside its owner (self-referential).
  The session tracks explicit transaction state
  (`TxState: Fresh → Active → Finalizing → Done | Detached`) and lends short
  `&mut` borrows of the guarded connection per operation: BEGIN is issued on
  the borrowed connection (transaction state lives server-side on the
  connection), each joined step executes on the borrowed connection
  (`plan_binds` takes `&mut AnyConnection`), and COMMIT/ROLLBACK are issued
  on the same borrow at finalization. A sqlx `Transaction` wrapper, if used,
  is confined to a single borrow scope and never stored. The guard is armed
  before BEGIN is sent and disarmed only after an acknowledged COMMIT or
  ROLLBACK; it stays armed through every await until acknowledgment.
  `Transaction::drop`'s implicit rollback is never treated as confirmation:
  finalization is an explicitly awaited rollback (or commit) on the borrowed
  connection. Cooperative cancellation awaits a bounded rollback; a forced
  drop (task abort/panic) at any point — begin, body, or finalization —
  detaches the connection (DiscardOnDrop). An unacknowledged rollback reports
  `RollbackUncertain` + `Detached`; an unacknowledged COMMIT reports
  `CommitUncertain` — never claiming a confirmed rollback. Both discard the
  connection; the pool stays usable. If the body already failed, the body
  error is primary and the rollback uncertainty is carried in the outcome,
  not substituted for it. Finalization — both rollback AND commit — is
  bounded by the same fixed 5 s finalize grace.
- **Rollback-only on joined DB error**: after any joined step returns a
  database execution error — statement execution OR bind/prepare probing
  (`plan_binds` sends prepares whose failures are flattened into
  `ProcessorError`; the failure classification is preserved before mapping) —
  the session is marked rollback-only BEFORE the
  error is returned to the pipeline, and further joined SQL is rejected at
  admission (`SessionAborted`) without touching the connection — a later
  statement must never run on a server-aborted transaction (SQLite, which
  ships by default, autocommits statements after a trigger-rolled-back
  transaction; a caught prepare error must equally be unable to produce
  `Committed`). The block cannot end Committed even when a `do_try` catch
  makes the body return Ok, because the server may have aborted the
  transaction (PostgreSQL answers a later COMMIT with a ROLLBACK completion
  tag, which the commit path does not distinguish). A rollback-only session
  finalizing after an Ok body fails the block with a typed
  `AbortedByHandledError` and rolls back; neither the pre-error nor the
  post-error attempt's writes persist.
- **Outcome semantics**: a clean `PipelineOutcome::Stopped` from the body
  commits and propagates unchanged (ADR-0025: structural steps preserve
  Stop; steps after the block do not run); `Err` rolls back; a rollback-only
  session under Ok or Stop fails per `AbortedByHandledError`.
- **Cancellation cutoff**: the observable cutoff is handing COMMIT to the
  driver after the final cancellation check — not a driver-internal event.
  Before the cutoff, cancellation triggers a bounded rollback (5 s grace);
  after it, the body deadline can no longer trigger rollback and the block
  resolves within the bound: acknowledged COMMIT → Committed; lost
  acknowledgment → `CommitUncertain`, never a rollback claim; COMMIT rejected
  by the database (a PostgreSQL `DEFERRABLE INITIALLY DEFERRED`
  foreign-key violation returning `23503`) → a
  typed `CommitFailed` with bounded cleanup or detachment — the connection is
  never returned to the pool with an active transaction. A forced drop — e.g.
  the inline dispatcher dropping the pipeline future — cannot await rollback;
  the armed DiscardOnDrop guard detaches the connection. Named fixtures pin
  all paths.
- **Supported join backends and connection hygiene**: joined steps are
  restricted to backends with provably transactional, provably-fresh
  transactions. Provider selects the connection factory; the dialect comes
  from the URL scheme, and one factory (sqlx) serves several dialects. In v1
  only PostgreSQL joins are allowed: SQLite joins are rejected at load
  (`UnsupportedJoinDialect` naming SQLite) because connection state is not
  provable from configuration — an attached database's journal mode
  (`journal_mode=OFF`) survives ROLLBACK while its writes persist, and no
  statement-level check can exclude it; MySQL joins are rejected for the same
  class of reason (MyISAM). Standalone usage of any dialect outside blocks is
  unchanged.
- **Fresh-transaction hygiene**: the shared pool can hand the block a
  connection with abandoned transaction state (standalone manual SQL may
  leave a transaction open; sqlx's release ping does not reset it, and
  PostgreSQL accepts a second BEGIN inside an open transaction — committing a
  prior borrower's writes). At join admission, BEFORE BEGIN, the session
  issues a hygiene ROLLBACK on the borrowed connection: a no-op warning on a
  clean connection, a terminator of any abandoned transaction. The opened
  transaction is therefore provably fresh; a prior borrower's uncommitted
  writes are DISCARDED (documented named behavior — silently committing them
  would be worse). Hygiene is a ONE-TIME acknowledged admission step: the
  discard guard is armed on connection checkout, hygiene runs once before the
  first `BEGIN`, and if the hygiene is not acknowledged the connection is
  detached WITHOUT sending `BEGIN`. A `max_connections=1` regression proves
  a block cannot commit a prior borrower's write, asserting the same
  PostgreSQL backend PID across both phases so a replacement connection
  cannot satisfy it.
- **Commit finalization classification**: `CommitFailed` is reserved for a
  DEFINITIVE database rejection of COMMIT — e.g. a PostgreSQL
  `DEFERRABLE INITIALLY DEFERRED` foreign-key violation where COMMIT returns
  `23503` (the transaction state is then known); a
  connection-fatal response without a definitive outcome (e.g. PostgreSQL
  `FATAL 57P01` from the termination trigger) is `CommitUncertain`, detaches
  the connection, and issues no rollback.
- **Deadline precedence**: block deadline and datasource acquire deadline are
  compared as absolute instants (not by outer-timeout polling order — the
  body is not simply polled first); the deadline that elapses first wins and
  the block deadline wins an exact tie. Finalization on timeout is bounded by
  the fixed 5 s finalize grace; a finalization that stalls past the grace
  degrades per its uncertainty variant.
- **Registry**: camel-core keeps a transaction-manager registry beside the
  idempotent-repository registry; the block compiler resolves by datasource
  name, fail-closed (`TransactionError::ManagerMissing` names the datasource).
  `camel-sql` registers a `SqlTransactionManager` per named datasource (catalog
  datasources at component registration; named endpoints at creation). The
  manager resolves its pool lazily through the existing `DatasourceCatalog`/
  `OnceCell` path — no eager connect.
- **Scope channel**: the block compiler bakes a `ScopeId` into each joined sql
  step processor (lexical membership — membership is the baked scope, not task
  or exchange membership) and delivers the per-exchange session via a
  `task_local!` (ADR-0043 precedent). A `direct:` callee's sql steps do not
  carry the `ScopeId` and therefore do not join — scope is the block, not the
  call chain. Clones never see the session (nothing in exchange extensions).
  A sequential split's fragments inside the block join like any other in-block
  step. An in-block dynamic-routing target that cannot prove membership at
  load fails closed (`DynamicTargetUnprovable`); transaction-control
  statements (BEGIN/COMMIT/ROLLBACK/SAVEPOINT) and other transaction-ending or
  implicit-commit statements in a joined step are rejected at load
  (`TransactionControlSql`) so user SQL cannot defeat the block's
  commit/rollback; runtime SQL sources (header/body-provided statements) are
  rejected for joined steps at load — a joined step's SQL is static,
  route-configured text — and dialects whose transaction-ending semantics the
  loader cannot prove are rejected at load. A baked join whose session is
  missing or mismatched at runtime fails with a typed error; it never
  auto-commits. Standalone manual transactions outside blocks are unchanged
  (runtime SQL, manual BEGIN/COMMIT all still allowed there).
  StreamList production is rejected unconditionally inside a block (separate
  from the reachability-based rejections).
- **Mode semantics (decided)**: inside a block, `Auto` **joins** — the block's
  lexical scope governs membership; silent auto-commit inside a transaction
  block would be exactly the degrade class `txnreject` removed. `Managed`
  inside a block joins (the explicit form). `Managed` outside any block —
  producer or consumer, including `from: sql:...transactionMode=Managed` — is
  a typed join-required load error naming the endpoint and datasource;
  endpoint creation no longer rejects it (validity is route-level); the
  `producer.rs`/`consumer.rs` Auto-fallback warnings go away; the `txnreject`
  tests are rewritten to the new contract (not deleted) and URI-secret
  redaction is retained in Managed messages. A sql
  step without a named datasource inside a block is a load error
  (no 2PC, nothing to join).
- **Executor refactor**: the four execute fns (`fetch_single_rows`,
  `execute_select`, `execute_modify`, `execute_batch`) move onto a borrowed
  connection surface (`DbHandle<'a>` over `&mut AnyConnection`); a joined step
  passes the session's transaction guard into `plan_binds` — signature
  unchanged, still the only bind path. `execute_batch` uses the joined
  transaction instead of `pool.begin()` inside a block, and batch statement
  planning routes through one shared planner entry point that accepts all
  batch shapes together (no per-shape calls into the single-shape `plan_binds`;
  no change to `sql-binds` whole-batch rules).
- **Block execution**: `tokio::time::timeout` (default 30 s, per-block
  `timeout_secs` override) around the body; finalization after the deadline is
  bounded by the 5 s finalize grace (rollback and commit alike); `Ok` →
  commit, `Err`/cancel → rollback; unacknowledged finalization → detach
  connection (DiscardOnDrop), pool stays usable; `try_join` while held →
  `Busy`.
- **Authoring surfaces**: YAML `transacted: { datasource, steps, timeout_secs? }`
  (`deny_unknown_fields`, snake_case per ADR-0017); builder
  `RouteBuilder::transacted("pg") … .end_transacted()` (SplitBuilder pattern);
  canonical form gains `CanonicalStepSpec::Transacted` in the camel-api
  versioned contract enum (ADR-0011/0016 strict, no silent drop) — the block
  must be canonically expressible (unlike `idempotent_consumer`, which
  canonical v2 rejects).

## Affected crates

- **camel-api**: `transaction.rs` port + typed errors + `CamelError` variant.
- **camel-sql**: `SqlTransactionManager`/session impl; borrowed-connection
  executor refactor; Managed message moves to load; docs.
- **camel-core**: `BuilderStep::Transacted`, block compiler + load validation,
  transaction-manager registry.
- **camel-dsl**: YAML step + lowering + strict keys.
- **camel-builder**: fluent `transacted` scope builder.
- **camel-test**: docker Postgres scenarios in `sql_test.rs`.
- **AGENTS.md**: COMPONENT LANES rows (`camel-component-sql`, `camel-api` local
  rows) per the gates order.

## Architecture boundaries

camel-api defines the port; camel-core resolves and orchestrates it through the
registry only (no camel-sql dependency); camel-sql implements the port
(component side). DSL/builder/canonical all lower to the same `BuilderStep`.
Docs prose follows ste-writing; commit messages follow caveman-commit.

## Phases

### Phase A: ADR + spec delta + port skeleton
- **Goal:** ADR 0101, this spec delta, and a compiling `TransactionManager`
  port with typed errors — no behavior change.
- **Dependencies:** none.
- **Externally-visible types/interfaces:** `TransactionManager`,
  `TransactionSession`, `TransactionJoin`, `TransactionOutcome`,
  `TransactionError`, `CamelError::Transaction`.
- **Deliverable:** committed ADR + port + re-exports; `cargo doc` clean.
- **Exit-criteria:** `cargo build -p camel-api` and clippy pass; ADR file
  numbered 0101 with Status header; no runtime behavior change.

### Phase B: route model + DSL + load validation
- **Goal:** `transacted` block authoring (model/DSL/builder/canonical) with all
  load-time rejections as named typed errors.
- **Dependencies:** Phase A types.
- **Externally-visible types/interfaces:** `BuilderStep::Transacted`, YAML
  `transacted` step, builder scope API, load rejection errors.
- **Deliverable:** load-rejection unit tests (each rejection named).
- **Exit-criteria:** every rejection scenario in the spec has a failing-input
  test; valid block route loads; unknown YAML key rejected.

### Phase C: borrowed-connection executor + join
- **Goal:** the four producer fns execute on a borrowed connection; joined
  steps run on the block's transaction via `plan_binds`.
- **Dependencies:** Phase A port, Phase B model.
- **Externally-visible types/interfaces:** none new (internal refactor).
- **Deliverable:** executor refactor + join wiring; existing sql suites green.
- **Exit-criteria:** `camel-component-sql` lib tests + `camel-test` lane
  command compile/pass non-docker; `plan_binds` still the only bind path.

### Phase D: commit/rollback/timeout/discard + pool bounds
- **Goal:** full execution semantics: lazy begin, commit/rollback, 30 s block
  timeout, DiscardOnDrop on uncertain rollback, `Busy`, typed pool-exhaustion
  error.
- **Dependencies:** Phase C join path.
- **Externally-visible types/interfaces:** block timeout config knob.
- **Deliverable:** runtime behavior + unit tests (non-docker where possible).
- **Exit-criteria:** typed errors observable; rollback/detach paths tested;
  cancel path tested.

### Phase E: docker tests, docs, lanes
- **Goal:** the docker Postgres suite (outbox, concurrency,
  `max_connections=1`, discard), docs page + operational note, lane rows.
- **Dependencies:** Phases A–D.
- **Externally-visible types/interfaces:** none new.
- **Deliverable:** `sql_test.rs` scenarios green locally under Docker; docs.
- **Exit-criteria:** `cargo test -p camel-test --features integration-tests
  --test sql_test` passes locally; docs build; lanes sweep clean.
