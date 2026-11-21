# Design: sqltrans-exec-verify

Parent ruling (e_opus 2026-11-15/2026-11-20, restated in the frozen
`openspec/changes/sqltrans/design.md`, do not re-litigate): surface (b),
explicit `transacted { datasource }` block, owned transaction per block,
lexical scope, PostgreSQL-only joins in v1, no 2PC. This slice owns Phases
C+D+E of that design (executor, join, finalization, docker suite, docs).

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
   true; 0 rows persist; pool immediately usable. → the observed definitive
   rejection (severity Error + `23503`) maps to **`CommitFailed`
   { datasource, reason }**; bounded best-effort cleanup; never a silent
   rollback. (Classification is stated for the observed pair — definitive
   rejection vs connection-fatal — not generalized to every severity-Error
   code.)
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

Everything else in the port is final and unchanged (S1 promised slice 2 could
implement finalization without changing signatures — confirmed: `begin`,
`set_rollback_only`, `commit`, `rollback`, `state`, `TransactionOutcome` cover
the whole lifecycle).

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
  `PoolConnection` (sole owner; never in exchange
  extensions), explicit `TxState` (`Fresh → Active → Finalizing → Done |
  Detached`) in a sync `Mutex` (`state()` is sync), `rollback_only: AtomicBool`,
  and a single admission slot (`tokio::sync::Mutex<()>`; `try_lock` → `Busy`,
  probe 6). BEGIN/COMMIT/ROLLBACK are issued as plain statements on the
  borrowed connection — **no sqlx `Transaction` wrapper is ever stored**
  (transaction state lives server-side on the connection); the borrow is
  confined to one await scope. `begin()` (lazy, once): hygiene already done at
  open; send `BEGIN`; state → Active; an unacknowledged `BEGIN` is never
  folded back to `Fresh` — acknowledged cleanup ROLLBACK or detach (see the
  finalization table). `commit()`: state → Finalizing; classify the driver
  result on exactly the probe-observed split — a DEFINITIVE database rejection
  (severity Error + SQLSTATE, observed `23503`: the transaction conclusively
  failed server-side) → `CommitFailed { datasource, reason }`, which the
  caller may handle as a normal constraint outcome; an UNRESOLVED outcome —
  severity Fatal `57P01`, connection death, Io error, or a timeout after the
  handoff — → `CommitUncertain` + DiscardOnDrop detach, never mapped to
  `CommitFailed` (the frozen spec reserves that variant for definitive
  rejections); acknowledged → disarm guard (connection returns to pool),
  state Done. `rollback()`: same bounded shape; acknowledged →
  `RolledBack`, disarm; unacknowledged → `RollbackUncertain` + Detach. Both
  finalizations bounded by the fixed **5 s finalize grace**; a finalization that
  stalls past the grace degrades to its uncertainty variant with the primary
  error preserved. `Drop` of the session while the guard is armed = DiscardOnDrop
  (detach: close the raw connection, never return to pool; pool stays usable —
  probe 2 backend replacement).
- **Joined execution**: a joined sql step's producer reads the channel:
  `JOIN_SCOPE` set but no `BLOCK_SESSION` → `JoinSessionMissing`; scope or
  datasource mismatch → `JoinScopeMismatch`/missing; then upcast
  (`Arc<dyn Any + Send + Sync>`) and `Arc::downcast` to `SqlTxSession`,
  `try_lock` the slot (Busy), auto-admit
  (hygiene at open; `begin()` on first join), execute the statement on the
  guarded connection via `plan_binds` (unchanged signature — still the only
  bind path), and on ANY joined database execution error call
  `set_rollback_only()` BEFORE the error is returned to the pipeline (probe
  note: never run statements on a server-aborted transaction). A later join on
  a rollback-only session is rejected at admission with `SessionAborted`
  without touching the connection.
- **Executor refactor (Phase A, no behaviour change)**: the four execute fns
  (`fetch_single_rows`, `execute_select`, `execute_modify`, `execute_batch`)
  move onto a borrowed-connection surface `DbHandle<'a>` over `&mut
  AnyConnection`; the producer call site acquires from the pool exactly as
  today and lends the handle. Joined steps lend the session's guarded
  connection instead — same downstream code. Preserved unchanged in Phase A:
  standalone (non-joined) batch execution keeps its own pool transaction for
  the statement batch, and StreamList keeps its deferred pooled acquisition
  (StreamList production inside a block is already rejected at load, so the
  streaming path never borrows a session connection).
- **Batch planner (req 12)**: `execute_batch` today plans per-shape via
  separate single-shape calls; batch planning moves to ONE shared entry point
  (`plan_batch_binds`) that accepts all batch shapes together, without
  changing `sql-binds` whole-batch rules or `plan_binds`' signature.

### camel-core: lowering, structural walk, block executor

- **Structural walk at the fan-in (closes rc-c3mjp)**: S1's
  `validate_transacted_blocks` walks the declarative AST only. S2 adds the
  equivalent walk over `BuilderStep` trees in camel-core — the SHARED
  validation point for ALL authoring paths is the `BuilderStep::Transacted`
  arm of `StepCompilerRegistry::compile_step` (every path — DSL-lowered,
  builder-authored, canonical-lowered, programmatic — compiles through it;
  it replaces the `ManagerMissing` backstop). The canonical arm in
  `commands.rs` (`canonical_step_to_builder_step`) runs the same walk as an
  early gate before aggregate-side effects continue. The walk enforces the
  FULL rejection set in two groups:
  (a) the S1 structural set: parallel split/multicast/wiretap/async
  handoff/seda escape reaching a joined sql step, StreamList inside a block,
  foreign/unnamed datasource, nested block, dynamic target unprovable;
  (b) the req-5 join-side SQL guards on every joined step's STATIC route SQL:
  reject header/body runtime sources and `file:` sources
  (`RuntimeSqlInJoinedStep`), reject transaction-ending/implicit-commit
  statement text — BEGIN/COMMIT/ROLLBACK/SAVEPOINT and dialect equivalents
  (`TransactionControlSql` naming the step and the offending statement),
  resolve the dialect from the datasource URL scheme with a registered
  manager (`DialectUnprovable` naming the unresolved scheme for unrecognized
  schemes; `UnsupportedJoinDialect` naming MySQL/SQLite for resolved
  non-PostgreSQL dialects). Registry resolution stays fail-closed
  (`ManagerMissing` names the datasource). Each rejection keeps its named
  unit-tier fixture asserting the exact variant and offender fields.
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
- **`TransactedSegment` runtime** (explicit lifecycle, in order): the block
  deadline is computed at BLOCK ENTRY — before admission — so session open
  (pool checkout) and hygiene are already raced against it. Per execution:
  (1) resolve nothing (the manager `Arc` was captured at lowering); (2) open
  the session: pool checkout AND the hygiene ROLLBACK raced against the two
  absolute deadlines (`block_deadline` and `now + manager.acquire_deadline()`
  as `Instant`s, compared absolutely — the one that elapses first wins; on an
  exact tie the block-timeout error is reported); unacknowledged hygiene →
  detach, no `BEGIN` (probe 4); (3) set `BLOCK_SESSION` for the body task and
  run the body under the deadline race (block timeout default 30 s, per-block
  override); lazy `BEGIN` at the first joined step is likewise bounded by the
  datasource acquire deadline; (4) the final cancellation check
  (the existing `CANCEL_TOKEN` task-local) happens immediately BEFORE handing
  COMMIT to the driver — that check is the cutoff: cancelled before it →
  bounded ROLLBACK (5 s grace); committed already → resolve within the grace,
  never roll back; (5) finalize per the table below. **Finalization is a pure
  function of the session's `TxState` at block exit; the table rows are
  DISJOINT — each state has exactly one rule, and the body outcome never
  widens a state's rule.**

  | `TxState` at block exit | finalization rule (exactly one) |
  |---|---|
  | `Fresh` (BEGIN never sent) | NO finalizing SQL, for ANY body outcome (Ok, clean `PipelineOutcome::Stopped`, Err, or cancellation): disarm the guard, return the CLEAN connection to the pool, and preserve the original outcome untouched (Ok stays Ok, Stop propagates unchanged, the body error stays primary). No ROLLBACK rule exists for this state — `Fresh` cannot be rollback-only (`set_rollback_only` is reachable only after a joined DB error, which requires an acknowledged `BEGIN`). |
  | `Active`, body Ok / clean Stop, not rollback-only | final cancellation check (the cutoff) → COMMIT bounded by the 5 s grace. Acknowledged → `Committed`, disarm, pool return (Stop propagates unchanged per ADR-0025; steps after the block do not run). DEFINITIVE rejection (severity Error + SQLSTATE, observed `23503`) → `CommitFailed` + bounded best-effort cleanup (acknowledged ROLLBACK or detach). UNRESOLVED outcome (Fatal `57P01`, connection death, Io, timeout after the handoff) → `CommitUncertain` + DiscardOnDrop. Never claim rollback for commit uncertainty. |
  | `Active`, body Err, or cancellation before the cutoff | bounded ROLLBACK (5 s grace): acknowledged → `RolledBack`, disarm, pool return; unacknowledged → `RollbackUncertain` + Detach. The body error stays primary when present; rollback uncertainty is carried in the outcome, never substituted. |
  | `Active`, `rollback_only` (any body outcome — Ok/Stop via a `do_try` catch included) | bounded ROLLBACK (5 s grace): acknowledged → the block fails typed `AbortedByHandledError` (a commit path must never report Committed for a server-aborted transaction); unacknowledged → `RollbackUncertain` + Detach. A pre-existing body error stays primary. |
  | COMMIT/ROLLBACK already handed off, resolution pending (cancellation after the cutoff, grace expiry) | resolve within the grace: acknowledged → `Committed`/`RolledBack`; unresolved → `CommitUncertain`/`RollbackUncertain` + Detach. No new SQL beyond the in-flight statement. |
  | `BEGIN` sent but NOT acknowledged (the `begin()` future itself failed or was lost) | NEVER treated as `Fresh`: require acknowledged cleanup — a ROLLBACK whose completion is observed, after which the connection may return to the pool — or detach (DiscardOnDrop). |
  | `Detached` (any origin: unacknowledged hygiene, uncertain finalization, forced drop) | terminal: no further SQL on this connection; the pool is already usable (replacement backend, probe 2). |
  | forced drop (pipeline future dropped mid-body, e.g. inline dispatcher) | cannot await anything, from ANY state: the armed guard's `Drop` detaches (DiscardOnDrop). |

- **Registry threading (obligation 2)**: `CommandDeps.transaction_registry`
  already reaches the loader; S2 adds the registration entry point on
  `RuntimeBus` (the registry field is private with no setter) and camel-bundles
  registers a `SqlTransactionManager` per catalog datasource at boot, before
  routes load (fail-closed `ManagerMissing` otherwise). camel-cli's assembly
  path registers through the same entry point.
- **Obligation 4 (documented constraint)**: `handle_register` persists the
  aggregate first (`repo.save`), then propagates lowering errors — including
  typed `ManagerMissing` from `canonical_to_route_definition` — via `?`
  BEFORE the runtime registration; only `execution.register_route` errors
  enter the pre-existing `runtime_err` compensation branch (Failed marking).
  We do not restructure the aggregate in this slice.

### Decisions carried in from S1 (blocking obligations)

1. **rc-c3mjp**: covered by the new `BuilderStep`-tree structural walk at both
   fan-ins (above); canonical/BuilderStep-authored blocks get exactly the DSL
   path's rejections. Named unit-tier fixtures assert each rejection.
2. **Registry threading**: bundle-boot registration via the new `RuntimeBus`
   entry point; resolution at the canonical lowering seam already lands in
   `commands.rs`.
3. **`DEFERRED_VARIANTS` decision**: `CamelError::Transaction` becomes a
   supported in-pipeline kind — `"Transaction"` enters
   `supported_exception_kinds()` with a match arm over `TransactionError`
   variants, and is removed from `DEFERRED_VARIANTS`. Rationale: block
   finalization failures must be catchable by `on_exceptions` (an operator can
   route transaction failures without flattening them into
   `ProcessorError`), while a `do_try` catch inside a block still cannot
   produce a commit (`AbortedByHandledError`). The vocabulary-guard test
   asserts the moved bucket.
4. **handle_register constraint**: documented above; no restructuring.
5. **Dynamic targets (req 5)**: REJECT at load. `RecipientList`/`RoutingSlip`
   (and other runtime-target steps) inside a block fail closed with
   `DynamicTargetUnprovable` — membership cannot be proven statically, and
   silent auto-commit inside a block is the degrade class this family exists
   to remove. One line in the ADR amendment, one named fixture.

### PG suite execution (rc-f3145)

`camel-test/tests/sql_test.rs` gains the runtime scenarios: outbox
commit/rollback, join proof, `max_connections=1` join, concurrent distinct
backend PIDs, prior-borrower discard + poisoning regression, 23503
`CommitFailed`, backend-kill `CommitUncertain` + pool replacement,
cancel-before/after cutoff, block timeout (`pg_sleep`), named pool exhaustion,
joined mixed-shape batch rollback, idempotent dispositions. New lane rows in
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

camel-api (additive join module + session-port `Any` supertrait), camel-sql,
camel-core, camel-dsl, camel-bundles, camel-test, docs, AGENTS.md.

## Architecture boundaries

Port stays provider-neutral; camel-core resolves via the registry only (no
camel-sql dependency); camel-sql implements and registers. Statements execute
only on lent connections; `plan_binds` remains the only bind path. No XA/2PC,
no per-exchange transactions, no transaction EIP, no new route surface, no new
dependencies. S3 scope untouched (`Managed` join-required, endpoint creation,
Auto-fallback warnings, `txnreject`).

## Phases

### Phase A: borrowed-connection executor (own commit, no behaviour change)
- **Goal**: the four execute fns run on a borrowed connection; one shared
  batch-planner entry point. Zero behaviour change; every existing suite green
  unmodified.
- **Dependencies**: none.
- **Deliverable**: `DbHandle<'a>` + refactored fns + `plan_batch_binds`;
  existing unit + lane suites pass untouched. Standalone batch keeps its own
  transaction; StreamList keeps deferred pooled acquisition.
- **Exit-criteria**: `cargo test -p camel-component-sql` green;
  `cargo test -p camel-test --features integration-tests --no-run` compiles;
  `plan_binds` signature unchanged. Bisectable: Phase A changes plumbing only;
  joining and block-owned finalization arrive with Phase B alone.

### Phase B: owned transaction + finalization + verification
- **Goal**: manager/session, join channel, lowering walk + block executor,
  registration, dynamic-target rejection, `DEFERRED_VARIANTS` resolution, PG
  suite, docs, lanes.
- **Dependencies**: Phase A.
- **Deliverable**: all S2 scenarios exercised (unit-tier where possible, PG
  suite for runtime capability scenarios).
- **Exit-criteria**: PG suite green locally; gates-local exit 0; openspec
  validate clean; `.review.json` committed.
