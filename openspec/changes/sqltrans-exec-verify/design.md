# Design: sqltrans-exec-verify (S2b — runtime semantics)

Parent ruling (e_opus 2026-11-15/2026-11-20, restated in the frozen
`openspec/changes/sqltrans/design.md`, do not re-litigate): surface (b),
explicit `transacted { datasource }` block, owned transaction per block,
lexical scope, PostgreSQL-only joins in v1, no 2PC. Per the 604c split ruling
this change is **S2b: runtime semantics** — reqs 2, 3, 4, 8, 9, 10, 11; req 5
RUNTIME (join channel, missing/mismatch, hygiene fixtures, maxconn1, PIDs,
sequential split, direct callee); req 12 runtime scenarios; req 14; the PG
suite; the req 15 remainder docs. The load-side half moved to the new
`sqltrans-exec-load` (S2a), which this change depends on. The camel-api
`Any` supertrait + `join` task-local module are a PORT DELTA owned here and
explicitly accepted by the 604c ruling (the port was declared frozen in S1).

## Design probes — OBSERVED BEHAVIOUR (run 2026-11-21, before any bless)

probe crate: `/home/shared/tmp/sqltrans-exec-verify/probe` (workspace-pinned
sqlx 0.9.0, testcontainers-modules 0.15, postgres:16-alpine). Full digest with
raw excerpts: `/home/shared/tmp/sqltrans-exec-verify/probe-digest.md`.
Environment note (recorded for the change docs): this machine's default
container port mapping is DEAD — docker-proxy accepts the TCP connection on
loopback but the data leg never answers (an SSLRequest gets no reply), so
mapped-port testcontainers connections hang. Local PG runs (probes and suite)
use `--network host` containers as the documented workaround; the
testcontainers mapped-port path remains the CI-owned path unchanged.

1. **`23503` at COMMIT** (`DEFERRABLE INITIALLY DEFERRED` FK): the INSERT
   succeeds inside the transaction; `commit()` returns
   `Err(Database(PgDatabaseError { severity: Error, code: "23503",
   constraint: Some("p1_child_pid_fkey") }))`; `is_foreign_key_violation()`
   true; 0 rows persist; pool immediately usable. → classified per the R7
   rule: **`CommitFailed`** requires affirmative evidence of definitive
   rejection — `sqlx::Error::Database` with PG severity exactly `ERROR` AND
   SQLSTATE class `23` (integrity violation; probe P1 proves `23503` only) or
   class `40` ("transaction rollback": the server affirms abort). Everything
   else — other severity-Error codes, FATAL severity, Io — classifies
   conservatively as **`CommitUncertain`** + detach. Bounded cleanup; never a
   silent rollback.
2. **Connection killed mid-COMMIT** (`pg_terminate_backend` inside a deferred
   constraint trigger): `commit()` returns
   `Err(Database(PgDatabaseError { severity: Fatal, code: "57P01" }))` — sqlx
   surfaces the kill as a **DatabaseError, not an Io error**, so classification
   keys on **severity (Fatal) / connection death**, not on the error type.
   Commit in flight aborted with the backend (0 rows); pool served a NEW
   backend pid afterwards. → **`CommitUncertain` + DiscardOnDrop**, never a
   rollback claim. This is the frozen spec's exact 57P01 case — observed.
3. **Cancellation cutoff**: probes establish the two ENDPOINT states. Cancel
   BEFORE the driver handoff → explicit bounded rollback completes in ~126 µs,
   no write persists. After an acknowledged COMMIT → a stray ROLLBACK on the
   same session is a server-side no-op (`WARNING: there is no transaction in
   progress`) and the committed row survives. The probes bracket the cutoff's
   two sides; they do NOT inject cancellation mid-block — the actual cutoff
   assertions (cancel-before-commit, cancel-after-ack, cancel-after-lost-ack)
   belong to the named fixtures (`transacted_cancel_before_commit`,
   `transacted_cancel_after_commit_ack`, `transacted_cancel_after_commit_lost`).
   Lost ACK mid-COMMIT = probe 2 → `CommitUncertain`.
4. **ROLLBACK before BEGIN**: raw ROLLBACK on a clean pooled connection → Ok
   (server logs `WARNING:  there is no transaction in progress`), no error
   surfaced — hygiene ROLLBACK is safe and a block that never touches the DB
   needs no rollback (lazy begin). Poisoning hazard REPRODUCED: a connection
   returned to the pool with an abandoned open transaction, a naive `BEGIN`
   logs `WARNING:  there is already a transaction in progress` (tx CONTINUES)
   and the block's COMMIT **committed the prior borrower's uncommitted row**
   (3 rows persisted where the block wrote 2) — mandatory pre-BEGIN hygiene
   confirmed. Hygiene ROLLBACK on the poisoned connection discards the
   abandoned write (0 rows).
5. **`acquire_timeout`**: pool `max_connections=1`, acquire_timeout 800 ms;
   second acquire → `Err(sqlx::Error::PoolTimedOut)` after 801.2 ms (bound
   honored; not a DatabaseError). sqlx does NOT name the datasource → our
   typed `PoolExhausted { datasource, max_connections, waited_ms }` adds it.
6. **`try_lock`**: `tokio::sync::Mutex::try_lock` while held → Err in ~110 ns;
   200 contended rounds deterministic (second attempt never succeeded) —
   busy-session admission is deterministic, never a hang.

**Contradiction check**: none — every probe observation matches the frozen
spec's stated semantics (CommitFailed for definitive rejection, CommitUncertain
for connection-fatal 57P01, cutoff observability, safe hygiene, bounded named
exhaustion, deterministic Busy). No `blocked: design` condition.

## Approach

### Join channel (camel-api, ADDITIVE — disclosed port change)

S1's port doc promises "the implementing crate can downcast the trait object to
its concrete type for statement execution", but `TransactionSession` has no
`Any` supertrait, so downcast is unrealizable today. **Disclosure (boundary
exception, gate-visible): camel-api gains exactly two additive items** —
- the `Any` supertrait on `TransactionSession`
  (`pub trait TransactionSession: Any + Send + Sync + 'static` — the trait
  already requires `'static` and `Send + Sync`; the workspace MSRV is 1.96,
  trait-object upcasting is stable since 1.86, so `Arc<dyn
  TransactionSession>` upcasts to `Arc<dyn Any + Send + Sync>` and
  `Arc::downcast` reaches the concrete session). No method signatures change,
  no methods are added to implementor burden (`Any` is auto-implemented), and
  the `non_exhaustive` types are untouched, and
- a new `transaction::join` module: `task_local!` channel
  `BLOCK_SESSION` (per-block `{ datasource, scope: u64, session:
  Arc<dyn TransactionSession> }`) and `JOIN_SCOPE` (`u64` baked per joined
  sql step), plus `JoinScope` newtype helpers. No sqlx types in camel-api.
- a **docs-only contract delta** on the existing methods (ruling 2, 604d —
  no signature change, no new variants, bless must accept): `rollback()` is
  TOTAL over every lifecycle state (the implementation selects the action
  from its private `TxPhase` — see the phase map in §SqlTxSession), and
  `commit()` gains the precondition "Precondition `Active`. Otherwise no SQL
  is sent and the call returns `Err(SessionAborted)`. The owner SHALL call
  `commit` only when `state() == Active`."

Everything else in the port is final and unchanged (S1 promised slice 2 could
implement finalization without changing signatures — confirmed: `begin`,
`set_rollback_only`, `commit`, `rollback`, `state`, `TransactionOutcome` cover
the whole lifecycle; the delta above is rustdoc/contract text only).

### camel-sql: manager, session, executor

- **`SqlTransactionManager`** (`impl TransactionManager`): `datasource()`,
  `acquire_deadline()` = the datasource `acquire_timeout` (probe 5 bounds);
  `open_session()` = acquire one pooled connection from the datasource pool
  (bounded by that deadline, typed `PoolExhausted` naming datasource +
  `max_connections` + waited), construct the session with the **discard guard
  armed immediately on checkout**, run admission hygiene (one ROLLBACK; probe 4:
  safe no-op on clean connections, terminator of abandoned transactions;
  unacknowledged hygiene → detach, no BEGIN, `RollbackUncertain` + `Detached`
  surfaced from `open_session`), and return the session. The manager resolves
  the pool through the existing `DatasourceCatalog` (same pool the component
  uses) — no eager connect, no new pool.
- **`SqlTxSession`** (`impl TransactionSession`, the `Arc::downcast` target
  reached via the `Any`-supertrait trait-object upcast): owns the guarded
  `PoolConnection` (sole owner; never in exchange extensions), a single
  admission slot (`tokio::sync::Mutex<()>`; `try_lock` → `Busy`, probe 6), and
  `rollback_only` as the port's `AtomicBool`. BEGIN/COMMIT/ROLLBACK are issued
  as plain statements on the borrowed connection — **no sqlx `Transaction`
  wrapper is ever stored** (transaction state lives server-side on the
  connection); the borrow is confined to one await scope.

  Decision state keys on a private **`TxPhase`**, never on `state()`: the
  frozen `TransactionState` cannot express "BEGIN bytes may be on the wire" or
  "COMMIT handed off"; both drive the outcome, so the gap is real and lives
  behind the enum.

  | TxPhase | Meaning | `state()` |
  |---|---|---|
  | `Clean` | hygiene acknowledged, nothing sent | `Fresh` |
  | `BeginPending` | set before the BEGIN write; cleared only by the ACK | `Fresh` (lossy — nothing may decide on `state()`) |
  | `Open` | BEGIN acknowledged | `Active` |
  | `CommitInFlight` | handed-off marker, set immediately before the COMMIT write | `Finalizing` |
  | `RollbackInFlight` | rollback sent, not yet acknowledged | `Finalizing` |
  | `Done` | acknowledged finalization, guard disarmed | `Done` |
  | `Detached` | connection discarded | `Detached` |

  Guard invariant: armed from checkout until `Done` or the R2 clean return;
  `Detached` is terminal.

  **`rollback()` is TOTAL** (ruling 2, 604d — the SESSION owns the non-`Open`
  rows inside it; the segment never needs `TxPhase`). Phase→action:
  `Clean` → R2, `BeginPending` → R3, `Open` → R4/R5/R6 SQL,
  `Detached`/defensive → R1. Its rustdoc (the disclosed docs-only port
  delta):

  ```rust
  /// End the block without committing. Total over every lifecycle state; the
  /// implementation selects the action from its private phase:
  /// - nothing begun (`Fresh`, BEGIN never sent): no SQL, connection returned
  ///   clean, `Ok(RolledBack)` (nothing was begun, so nothing is durable);
  /// - BEGIN sent but unacknowledged (`Fresh`): no SQL, detach,
  ///   `Err(RollbackUncertain)`;
  /// - `Active`: ROLLBACK within the finalize grace; acknowledged →
  ///   `Ok(RolledBack)`, else detach + `Err(RollbackUncertain)`;
  /// - `Detached`, or a defensive `Finalizing`/`Done` entry: no SQL,
  ///   `Err(RollbackUncertain)` (`error!` log for the defensive entry).
  fn rollback(&self) -> FinalizeFuture<'_>;
  ```

  `begin()` resolution (lazy, once; hygiene already done at open — mandatory,
  probe P4):
  1. Precondition `Clean`; else `Ok(())` if `Open`, else `SessionAborted`.
  2. Set `BeginPending`, send `BEGIN` under
     `timeout_at(begin_start + acquire_deadline())` (req 11).
  3. ACK → `Open`.
  4. Own timeout → detach, `Detached`, return
     `PoolExhausted { datasource, max_connections, waited_ms }`.
  5. Any driver error → detach, `Detached`, return
     `RollbackUncertain { datasource }`.
  6. NO cleanup SQL in any case: protocol position unknown, nothing durable.
  7. Future dropped → phase stays `BeginPending` → R3 (cooperative) or R0
     (forced).
  8. The joined step calls `set_rollback_only()` before returning ANY
     `begin()` error.

  `commit()`/`rollback()`: enforce the grace INTERNALLY — ONE absolute
  finalize deadline, `finalize_start + 5 s`, covering every finalization SQL
  in that run (COMMIT plus any cleanup ROLLBACK) — so the session detaches
  itself; acknowledged → disarm guard (connection returns to pool), `Done`;
  not acknowledged / stalled past grace → the uncertainty variant
  (`CommitUncertain` / `RollbackUncertain`) + `Detached`. **`commit()` on a
  non-`Open` phase → `Err(SessionAborted)`, no SQL** (docs-only port delta,
  ruling 2). The SEGMENT only decides commit-vs-rollback from what the frozen
  port already exposes (`state()`, `is_rollback_only()`, `BodyEnd`, the
  cutoff); the session returns only the cleanup result, and a cleanup result
  never replaces an existing primary error. `Drop` of the
  session while the guard is armed = R0 DiscardOnDrop (detach: close the raw
  connection, never return to pool; pool stays usable — probe 2 backend
  replacement).
- **Joined execution**: a joined sql step's producer reads the channel:
  `JOIN_SCOPE` set but no `BLOCK_SESSION` → `JoinSessionMissing`; scope or
  datasource mismatch → `JoinScopeMismatch`/missing; then upcast
  (`Arc<dyn Any + Send + Sync>`) and `Arc::downcast` to `SqlTxSession`,
  `try_lock` the slot (Busy), auto-admit
  (hygiene at open; `begin()` on first join), execute the statement on the
  guarded connection via `plan_binds` (unchanged signature — still the only
  bind path), and on ANY joined database execution error call
  `set_rollback_only()` BEFORE the error is returned to the pipeline (probe
  note: never run statements on a server-aborted transaction). Any error
  from `begin()` or statement execution sets `rollback_only` before return;
  Busy/SessionAborted/JoinSessionMissing/JoinScopeMismatch are admission
  errors and do not touch the connection. A later join on
  a rollback-only session is rejected at admission with `SessionAborted`
  without touching the connection.

### camel-core: block executor

- **Scope baking + decorators**: the `Transacted` compiler allocates a `u64`
  scope id, lowers the body with a join context, and wraps ONLY the compiled
  processors of lexically-in-block sql steps on the block datasource with a
  decorator that sets `JOIN_SCOPE` around the inner call. A `direct:` callee's
  processors are compiled outside the block and are never decorated — the
  callee never joins even though `BLOCK_SESSION` is visible (scope is the
  block, not the call chain). Sequential splits/`do_try`/filter bodies recurse,
  so their fragments' sql steps are decorated individually. Parallel split /
  multicast / wiretap / async / seda reaching joined sql are rejected at load
  before any decoration matters.
- **`TransactedSegment` runtime** (explicit lifecycle, ONE sequence, in
  order): (1) resolve nothing — the manager `Arc` was captured at lowering;
  (2) compute `block_deadline` at BLOCK ENTRY (default 30 s, per-block
  override) and open the session: pool checkout AND the hygiene ROLLBACK race
  `min(block_abs, acquire_abs)` as absolute instants; the guard arms at
  checkout; expiry drops the open future so R0 detaches after exactly one
  ROLLBACK attempt; primary: `PoolExhausted` if `acquire_abs < block_abs`,
  else `Timeout` (tie included); unacknowledged hygiene → `open_session`
  returns `Err(RollbackUncertain)`, connection detached; (3) set
  `BLOCK_SESSION` for the body task and run the body race; (4) cooperative
  finalization via the segment pseudocode below — the SEGMENT only decides
  commit-vs-rollback from what the frozen port already exposes (`state()`,
  `is_rollback_only()`, `BodyEnd`, the cutoff); the SESSION owns the
  non-`Open` rows inside a total `rollback()` (ruling 2, 604d, option ii).

  Body race: `biased select { sleep_until(block_deadline),
  CANCEL_TOKEN.cancelled(), body }` in that order →
  `BodyEnd ∈ {Ok(Continue), Ok(Stopped), Err(e), Deadline, Cancelled}`.
  Tie → deadline wins → `Timeout`.

  Segment pseudocode (replaces row selection in camel-core; the R-rows below
  stay as the normative outcome table):

  ```text
  primary = match body_end {                       // precedence lives here, unchanged
    Deadline => Some(Timeout), Err(e) => Some(e), Cancelled => Some(Cancelled),
    Ok(_) if s.is_rollback_only() => Some(AbortedByHandledError), Ok(_) => None }
  if primary.is_none() && s.state() == Active {    // R7 gate
    if CANCEL_TOKEN.is_cancelled() || now >= block_deadline {      // R6
      primary = Some(if now >= block_deadline { Timeout } else { Cancelled }) }
    else { return map_commit(s.commit().await, body_end) } }       // R7, no re-consult
  let r = s.rollback().await;                      // R1–R6: session selects by TxPhase
  match (primary, r) {
    (None, Ok(_))          => body_end_ok_unchanged,   // R2 clean, Stop per ADR-0025
    (None, Err(u))         => Err(u),                  // e.g. step-local drop left BeginPending
    (Some(p), Ok(_))       => Err(p),
    (Some(p), Err(u))      => Err(p).with_cleanup(u),  // RollbackUncertain + Detached context
  }
  ```

  Precedence (Timeout > body Err > cancellation > `AbortedByHandledError`)
  stays in the segment. The session returns only the cleanup result; a
  cleanup result never replaces an existing primary error.

  **R0 — forced drop** (guard `Drop`, any `TxPhase` except `Done`/`Detached`).
  Includes mid-body, pending BEGIN, open_session/hygiene, stalled
  COMMIT/ROLLBACK await. Action: detach synchronously (connection
  closed/detached, never returned), no SQL. This is the guard invariant, not a
  cooperative row.

  Cooperative rows — first match wins, evaluated once after the body race.
  SELECTION SPLIT: R1–R3 are selected inside `SqlTxSession::rollback` (the
  session knows its private `TxPhase`); R4–R7 are selected by the segment on
  `state()`, the flag, `BodyEnd` and the cutoff — the rows remain the
  normative outcome table.

  **R1 — `TxPhase == Detached`** (a failed `begin()` or a joined transport
  failure already detached). No SQL. Primary by precedence: Timeout > body
  Err > cancellation > `AbortedByHandledError`. Cleanup context
  `RollbackUncertain` + `Detached`. Connection already detached.

  **R2 — `TxPhase == Clean`** (BEGIN never attempted). Disarm, return the
  connection clean, no SQL. Outcome = body end unchanged (Ok stays Ok, Stop
  propagates per ADR-0025, Err primary, Deadline → Timeout, Cancelled →
  cancellation). `rollback_only` unreachable (`debug_assert`).

  **R3 — `TxPhase == BeginPending`** (the race dropped `begin()`'s future).
  Detach, no SQL: protocol position unknown, nothing of the block's
  acknowledged. Primary: Timeout or cancellation. Cleanup context
  `RollbackUncertain` + `Detached`.

  **R4 — `Open` ∧ `rollback_only`.** The FLAG alone triggers this row (req 4),
  whether or not the error was caught. `rollback()` within the grace; never
  COMMIT. Primary: Timeout (req 9) > body Err > cancellation >
  `AbortedByHandledError`. Acknowledged → `RolledBack`, disarm, return to
  pool; not acknowledged / stalled past grace → `RollbackUncertain` +
  `Detached`, primary unchanged.

  **R5 — `Open` ∧ `BodyEnd ∈ {Err, Deadline, Cancelled}`.** As R4. Primary:
  body Err, Timeout, or cancellation.

  **R6 — `Open` ∧ body Ok/Stop ∧ cutoff FAILS.** The check runs immediately
  before the handoff: `CANCEL_TOKEN.is_cancelled() || now >= block_deadline`.
  As R4; COMMIT never sent. Primary: Timeout if the deadline elapsed (tie
  included), else cancellation.

  **R7 — `Open` ∧ body Ok/Stop ∧ cutoff PASSED.** Set `CommitInFlight`, hand
  COMMIT to the driver; after this point deadline and cancellation are never
  consulted.
  - **`Committed`** (driver Ok): disarm, return to pool, Stop propagates
    unchanged per ADR-0025 (steps after the block do not run).
  - **`CommitFailed`** (affirmative evidence ONLY): `sqlx::Error::Database`
    with PG severity exactly `ERROR` AND SQLSTATE class `23` (integrity
    violation; `23503` probed, probe P1) or `40` ("transaction rollback": the
    server affirms abort). One cleanup ROLLBACK within the remaining grace (a
    no-op WARNING on PG, probe P4). Acknowledged → return to pool; not
    acknowledged → detach with `RollbackUncertain` + `Detached`.
  - **`CommitUncertain`** (everything else): FATAL/PANIC severity (`57P01`
    per probe P2), Io, Protocol, PoolClosed, any other code, or grace elapsed.
    Detach immediately; no ROLLBACK, no rollback claim.

  Segment totality: commit iff `state()==Active ∧ !rollback_only ∧ body
  Ok/Stop ∧ cutoff passed`, else `rollback()`. Session totality: `rollback()`
  covers every `TxPhase`.

### PG suite execution (rc-f3145)

`camel-test/tests/sql_test.rs` gains the runtime scenarios: outbox
commit/rollback, join proof, `max_connections=1` join, concurrent distinct
backend PIDs, prior-borrower discard + poisoning regression, 23503
`CommitFailed`, backend-kill `CommitUncertain` + pool replacement,
cancel-before/after cutoff, block timeout (`pg_sleep`), named pool exhaustion,
joined mixed-shape batch rollback, idempotent dispositions. Unit-tier
virtual-clock fixtures (injected timer, no Docker) pin the decision-list
paths: `transacted_begin_pending_deadline` — a **camel-sql unit fixture**
(`rollback()` from `BeginPending` → detach, no SQL, `Err(RollbackUncertain)`,
R3 via the total rollback); `transacted_fresh_rollback_precedence` — a
**camel-core segment fixture** with a fake session returning Ok/Err from
`rollback()` under Deadline, Err and Ok bodies, asserting the primary error
and the cleanup context (the pseudocode's precedence table); and
`transacted_commit_classifier_table` (R7 classifier table: SQLSTATE class
`23` and `40` → `CommitFailed`; `57P01`, `57014`, `08006`, Io →
`CommitUncertain` + detach). All frozen fixture names are kept
(`transacted_cancel_before_commit`, `transacted_cancel_after_commit_ack`,
`transacted_cancel_after_commit_lost`, `transacted_commit_rejected`,
`transacted_commit_deferred_fk`, `transacted_forced_drop_inline`,
`transacted_hygiene_unacknowledged`, `transacted_hygiene_deadline_expiry`,
`transacted_block_deadline_first`, `transacted_acquire_deadline_first`,
`transacted_deadline_tie`, `transacted_stalled_rollback`,
`transacted_stalled_commit`). New lane rows in
`AGENTS.md ## COMPONENT LANES`. Local execution workaround (this host only):
the default mapped-port path is dead (TCP accepted, SSLRequest unanswered —
data leg blocked), so the suite runs against a `--network host` Postgres with
the same test bodies; the testcontainers mapped-port path stays the CI-owned
default and is not modified.

### Docs

ADR 0101 amendment section (Accepted): probe-verified finalization
classification, dynamic-target rejection line, executor join detail,
PgBouncer `DISCARD ALL` / restart-on-upgrade operational note pointer.
`docs/src/sql-transactions.md`: fill the S2-pending sections (operational
note, PostgreSQL-only v1, at-least-once window). `docs/src/components/
database.md`: the operational note. English, ste-writing.

## Affected crates

camel-api (additive join module + session-port `Any` supertrait — PORT DELTA,
explicitly accepted by the 604c ruling), camel-sql, camel-core (block
executor + scope baking only), camel-test, docs, AGENTS.md. The load-side
crates (camel-dsl, camel-bundles) belong to `sqltrans-exec-load`.

## Architecture boundaries

Port stays provider-neutral; camel-core resolves via the registry only (no
camel-sql dependency); camel-sql implements. Statements execute only on lent
connections; `plan_binds` remains the only bind path (its borrowed-connection
planner changes live in `sqltrans-exec-load`). No XA/2PC, no per-exchange
transactions, no transaction EIP, no new route surface, no new dependencies.
S3 scope untouched (`Managed` join-required, endpoint creation, Auto-fallback
warnings, `txnreject`).

## Phases

**Depends on: `sqltrans-exec-load`** (S2a: `DbHandle`/`plan_batch_binds`, req
5 LOAD rejections, the rc-c3mjp BuilderStep walk at both fan-ins, the
`RuntimeBus` registry entry point + bundle/CLI registration, the
`DEFERRED_VARIANTS` move). S2b lands on top of it.

### Phase B: owned transaction + finalization + verification
- **Goal**: manager/session (`TxPhase`, R0–R7), join channel + admission,
  scope baking + `TransactedSegment` runtime, PG suite, docs, lanes.
- **Dependencies**: `sqltrans-exec-load` landed.
- **Deliverable**: all S2b runtime scenarios exercised (unit-tier virtual
  clock where possible, PG suite for runtime capability scenarios).
- **Exit-criteria**: PG suite green locally; gates-local exit 0; openspec
  validate clean; `.review.json` committed.
