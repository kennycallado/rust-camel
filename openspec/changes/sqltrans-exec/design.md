# Design: sqltrans-exec (code-first SPIKE)

This change is the code-first spike that replaces the prose spec loop. The
normative design already exists and is binding; this document carries it
verbatim (604c §A/§B, 604d) and adds only the spike step mapping, the probe
evidence, and `file:line` citations as the code lands.

Binding precedence: the frozen family spec
(`openspec/changes/sqltrans/specs/sql-transactions/spec.md`) wins over every
ruling below.

## Normative text — 604c §A/§B (verbatim)

Ruling: `.opencode/fleet/orders/604c-sqltrans-s2-ruling.md` (e_opus,
2026-11-21). Sections §A and §B are reproduced word-for-word below; they are
the finalization decision list and the `begin()` resolution that the
`SqlTxSession` and `TransactedSegment` implement.

### A. Final cooperative-finalization decision list

Key on a private `TxPhase`, never on `state()`. The frozen `TransactionState`
cannot express "BEGIN bytes may be on the wire" or "COMMIT handed off"; both
drive the outcome, so the gap is real and must live behind the enum.

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
`Detached` is terminal; `rollback_only` is the port's `AtomicBool`.

Body race: `biased select { sleep_until(block_deadline), CANCEL_TOKEN.cancelled(), body }`
in that order → `BodyEnd ∈ {Ok(Continue), Ok(Stopped), Err(e), Deadline, Cancelled}`.
Tie → deadline wins → `Timeout`.

Grace: ONE absolute finalize deadline, `finalize_start + 5 s`, covering every
finalization SQL in that run (COMMIT plus any cleanup ROLLBACK); `commit()` and
`rollback()` enforce it internally, so the session detaches itself.

**R0 — forced drop (guard `Drop`, any TxPhase except `Done`/`Detached`).**
Includes mid-body, pending BEGIN, open_session/hygiene, stalled COMMIT/ROLLBACK
await. Action: detach synchronously (connection closed/detached, never
returned), no SQL. This is the guard invariant, not a cooperative row.

### Cooperative rows — first match wins, evaluated once after the body race

**R1 — `TxPhase == Detached`** (a failed `begin()` or a joined transport failure
already detached). No SQL. Primary by precedence: Timeout > body Err >
cancellation > `AbortedByHandledError`. Cleanup context `RollbackUncertain` +
`Detached`. Connection already detached.

**R2 — `TxPhase == Clean`** (BEGIN never attempted). Disarm, return the
connection clean, no SQL. Outcome = body end unchanged (Ok stays Ok, Stop
propagates per ADR-0025, Err primary, Deadline → Timeout, Cancelled →
cancellation). `rollback_only` unreachable (`debug_assert`).

**R3 — `TxPhase == BeginPending`** (the race dropped `begin()`'s future).
Detach, no SQL: protocol position unknown, nothing of the block's acknowledged.
Primary: Timeout or cancellation. Cleanup context `RollbackUncertain` +
`Detached`.

**R4 — `Open` ∧ `rollback_only`.** The FLAG alone triggers this row (req 4),
whether or not the error was caught. `rollback()` within the grace; never
COMMIT. Primary: Timeout (req 9) > body Err > cancellation >
`AbortedByHandledError`. Acknowledged → `RolledBack`, disarm, return to pool;
not acknowledged / stalled past grace → `RollbackUncertain` + `Detached`, primary
unchanged.

**R5 — `Open` ∧ `BodyEnd ∈ {Err, Deadline, Cancelled}`.** As R4. Primary: body
Err, Timeout, or cancellation.

**R6 — `Open` ∧ body Ok/Stop ∧ cutoff FAILS.** The check runs immediately before
the handoff: `CANCEL_TOKEN.is_cancelled() || now >= block_deadline`. As R4;
COMMIT never sent. Primary: Timeout if the deadline elapsed (tie included), else
cancellation.

**R7 — `Open` ∧ body Ok/Stop ∧ cutoff PASSED.** Set `CommitInFlight`, hand
COMMIT to the driver; after this point deadline and cancellation are never
consulted.
- **Committed** (driver Ok): disarm, return to pool, Stop propagates unchanged.
- **CommitFailed** (affirmative evidence ONLY): `sqlx::Error::Database` with PG
  severity exactly `ERROR` AND SQLSTATE class `23` (integrity violation; 23503
  probed) or `40` ("transaction rollback": the server affirms abort). One cleanup
  ROLLBACK within the remaining grace (a no-op WARNING on PG). Acknowledged →
  return to pool; not acknowledged → detach with `RollbackUncertain` +
  `Detached`.
- **CommitUncertain** (everything else): FATAL/PANIC severity (57P01 per P2),
  Io, Protocol, PoolClosed, any other code, or grace elapsed. Detach
  immediately; no ROLLBACK, no rollback claim.

Totality: at finalization entry TxPhase ∈ {Clean, BeginPending, Open, Detached}
(only the segment enters `*InFlight`/`Done`). R1–R3 partition on TxPhase; R4–R7
partition `Open` by flag → BodyEnd → cutoff. Defensive fallback for an entry
phase of `*InFlight`/`Done`: `error!` log, treat as R1.

Admission: checkout and hygiene race `min(block_abs, acquire_abs)` as absolute
instants; the guard arms at checkout; expiry drops the open future so R0 detaches
after exactly one ROLLBACK attempt. Primary: `PoolExhausted` if
`acquire_abs < block_abs`, else `Timeout` (tie included). Unacknowledged hygiene
→ `open_session` returns `Err(RollbackUncertain)`, connection detached.

### B. `begin()` resolution

1. Precondition `Clean`; else `Ok(())` if `Open`, else `SessionAborted`.
2. Set `BeginPending`, send `BEGIN` under `timeout_at(begin_start + acquire_deadline())` (req 11).
3. ACK → `Open`.
4. Own timeout → detach, `Detached`, return
   `PoolExhausted { datasource, max_connections, waited_ms }`.
5. Any driver error → detach, `Detached`, return `RollbackUncertain { datasource }`.
6. NO cleanup SQL in any case: protocol position unknown, nothing durable.
7. Future dropped → phase stays `BeginPending` → R3 (cooperative) or R0 (forced).
8. The joined step calls `set_rollback_only()` before returning ANY `begin()` error.

## Normative text — 604d (verbatim)

Ruling: `.opencode/fleet/orders/604d-sqltrans-s2-ruling2.md` (e_opus,
2026-11-21). Option ii, binding.

### Decision (option ii)

The SEGMENT only decides commit-vs-rollback, from what the frozen port already
exposes (`state()`, `is_rollback_only()`, `BodyEnd`, the cutoff). The SESSION owns
the non-`Open` rows inside a total `rollback()`. No new method, no new variants,
no signature change.

**Port delta (docs-only, accepted by 604c).**
Replace the doc of `TransactionSession::rollback`:

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

Add to `commit`: "Precondition `Active`. Otherwise no SQL is sent and the call
returns `Err(SessionAborted)`. The owner SHALL call `commit` only when
`state() == Active`."

**Segment pseudocode (replaces R1–R7 selection in camel-core):**

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

Precedence (Timeout > body Err > cancellation > `AbortedByHandledError`) stays in
the segment. The session returns only the cleanup result; a cleanup result never
replaces an existing primary error.

### S2a registration scope (604d §S2a, corrected by mission 609)

This spike covers S2a scope AND S2b scope in one change (steps 1–9). Registration:
`RuntimeBus::register_transaction_manager` + the two bus tests
(`runtime_bus_registers_and_resolves_manager`,
`unregistered_datasource_still_fails_closed`) land in step 4 with
`bundle_boot_leaves_transaction_registry_empty` asserting the registry has ZERO
keys. Step 9 (last, separately revertible) replaces the empty guard with
`bundle_boot_registers_catalog_datasources` and registers one
`SqlTransactionManager` per catalog datasource in bundle + CLI.

## Probe evidence (observed PostgreSQL 16.10-alpine, sqlx 0.9.0)

Digest: `/home/shared/tmp/sqltrans-exec-verify/probe-digest.md`. Six probes, no
contradiction with the frozen spec:

- **P1**: deferred FK violation surfaces at COMMIT as
  `Database(severity=Error, code=23503)`; 0 rows persist; pool usable →
  `CommitFailed` (definitive).
- **P2**: backend killed mid-COMMIT (`pg_terminate_backend` in a deferred
  trigger) surfaces as `Database(severity=Fatal, 57P01)` — a DatabaseError, not
  Io; classification MUST key on severity/connection death → `CommitUncertain` +
  detach, never a rollback claim.
- **P3**: cancel before the driver handoff → bounded rollback ~126 µs; after an
  acknowledged COMMIT a stray ROLLBACK is a server-side no-op.
- **P4**: hygiene ROLLBACK on a clean connection is a safe no-op; POISONING
  HAZARD reproduced (naive BEGIN on an abandoned-tx connection + COMMIT commits
  the prior borrower's row) → hygiene is mandatory.
- **P5**: acquire_timeout honored (`PoolTimedOut` after exactly the bound) →
  typed `PoolExhausted { datasource, max_connections, waited_ms }`.
- **P6**: `try_lock` while held fails deterministically (~110 ns, 200 rounds) →
  Busy admission, never a hang.

Environment note: this host's mapped-container-port data leg is dead
(docker-proxy accepts TCP, SSLRequest never answered). Local PG runs use a
`--network host` container via the `SQLTRANS_PG_URL` override; the
testcontainers mapped-port path stays the CI-owned default and is not modified.
CI's docker lane is the acceptance authority.

## Spike step mapping

| Step | Deliverable | Primary files |
|---|---|---|
| 1 | Typed error propagation (fixes the fallback `Config` erasure) | `crates/camel-core/src/lifecycle/application/commands.rs` |
| 2 | Phase A: `DbHandle` + `plan_batch_binds` | `crates/components/camel-sql/src/producer.rs`, `bind_kinds.rs` |
| 3 | LOAD walk at both fan-ins + req-5 fixture table + `DEFERRED_VARIANTS` move + `Managed` regression | `crates/camel-core/src/transacted_walk.rs` (new), `crates/camel-dsl/src/compile.rs`, `crates/camel-dsl/src/transacted_validation.rs`, `crates/camel-api/src/transaction.rs` (`DynamicTargetUnprovable`) |
| 4 | Registry entry point + empty-registry guard | `crates/camel-core/src/lifecycle/application/runtime_bus.rs`, `crates/camel-bundles/src/lib.rs` |
| 5 | Port delta: `Any` supertrait, `join` module, docs-only contract | `crates/camel-api/src/transaction.rs`, `crates/camel-api/src/transaction/join.rs` |
| 6 | `SqlTxSession` + fixtures | `crates/components/camel-sql/src/session.rs` (new) |
| 7 | `TransactedSegment` + fake-session fixture | `crates/camel-core/src/transacted_segment.rs` (new) |
| 8 | PG suite (P1–P6 as frozen fixtures) | `camel-test/tests/sql_transacted_test.rs` + `SQLTRANS_PG_URL` override |
| 9 | Bundle/CLI registration (last, revertible) | `crates/camel-bundles/src/lib.rs`, `crates/camel-cli` |

### `file:line` citations (added as the code lands)

- Step 1 erasure site: `crates/camel-core/src/lifecycle/application/commands.rs`
  `canonical_to_route_definition` circuit-breaker fallback `map_err` (was line
  1029 at dispatch).
- Step 3 fan-ins: the ONE visitor lives in
  `crates/camel-core/src/transacted_walk.rs` (`walk_transacted_block` at :93,
  parse-seam entry `walk_transacted_block_deferred` at :116). Wiring:
  declarative lowering `crates/camel-dsl/src/compile.rs:1372`
  (`DeclarativeStep::Transacted` arm, deferred); canonical lowering
  `crates/camel-dsl/src/compile.rs:635` (`CanonicalStepSpec::Transacted` arm,
  deferred, depth-0 only) and
  `crates/camel-core/src/lifecycle/application/commands.rs:1337`
  (`CanonicalStepSpec::Transacted` arm, registry-backed manager resolver +
  always-absent URL, depth-0 only so nested blocks surface as the outer
  `NestedBlock`). `compile_step`'s `BuilderStep::Transacted` backstop
  (`crates/camel-core/src/lifecycle/adapters/step_compilers/mod.rs:300`)
  stays the route-add dark gate; step 9 re-armed it from `ManagerMissing` to
  the frozen `JoinSessionMissing` (error at `:302`) — boot registers one
  `SqlTransactionManager` per catalog datasource, so a block passes load and
  fails closed at lowering until the executor slice. `DynamicTargetUnprovable` added at
  `crates/camel-api/src/transaction.rs` (after `UnsupportedJoinDialect`);
  `"Transaction"` moved out of `DEFERRED_VARIANTS` into
  `supported_exception_kinds()` in `compile.rs` with a
  `exception_kind_matches` arm over `CamelError::Transaction(_)`.
- Step 4 entry point: `crates/camel-core/src/lifecycle/application/runtime_bus.rs`
  (registry field exists since S1).
- S1 baseline facts: `TransactionState` = Fresh/Active/Finalizing/Done/Detached
  (`crates/camel-api/src/transaction.rs:35`); `DEFERRED_VARIANTS` contains
  `"Transaction"` (`crates/camel-dsl/src/compile.rs:2910`); declarative
  validation precedence 1–5 lives in
  `crates/camel-dsl/src/transacted_validation.rs`; `DynamicTargetUnprovable`
  does not exist yet (added in step 3).
- Step 6 session: `crates/components/camel-sql/src/session.rs` — `TxPhase` at
  :62 with the `project` table at :83; `TxIo`/`TxTimer` unit seams at :165/:329
  (real backend `AnyConnIo`, default `TokioTimer`); checkout guard
  `CheckedOutBackend` at :241 (armed at `pool.acquire()` resolution, disarmed
  only at the `with_io` hand-off); `SqlTxSession` at :406,
  `with_io` at :432, admission (`open` + hygiene) at :462/:847; R7 classifier
  `commit_disposition` at :136; frozen fixtures
  `transacted_commit_classifier_table` at :1290 and
  `transacted_begin_pending_deadline` at :1696.

### Edge adjudications

- **CommitFailed vs cleanup-unacknowledged**: the frozen requirement
  "COMMIT rejected SHALL fail with typed `CommitFailed`" WINS over 604c
  R7's sub-row ("not acknowledged → detach with `RollbackUncertain` +
  `Detached`") — the frozen Cancellation-cutoff text is explicit: a COMMIT
  rejected by the database SHALL fail with the typed `CommitFailed`, and the
  connection SHALL be cleaned up within the bound or detached. The session
  therefore returns `Err(CommitFailed)` in BOTH cleanup outcomes; when the
  cleanup ROLLBACK is unacknowledged or stalls past the remaining grace, the
  cleanup uncertainty is observable through the `warn!` log and the
  `Detached` state — logged, never replacing the primary (fixture
  `commit_cleanup_unacknowledged_keeps_commit_failed`). Typed
  `RollbackUncertain` remains the session-level result only for
  rollback-uncertainty paths with NO primary (R1/R3/R4/R5 legs). 604c R7's
  sub-row is overridden by the frozen spec on this sub-edge.
- **Admission deadline order**: the frozen "Block timeout" requirement
  ("Deadlines are compared as absolute instants — not by outer-timeout
  polling order") governs the both-elapsed case: after a deadline race
  resolves with both instants already spent, the admission reports
  `PoolExhausted` iff `acquire_abs < block_abs` strictly, else `Timeout`
  (tie included). This holds at every race site — the session's open race,
  the admission-hygiene race, and the segment's open race (a resolved open
  is authoritative; the segment timer bounds only a still-pending open).
  Port-corner note (RESOLVED at the segment tier, rc-9awnc): the trait's
  `open_session()` carries no block instant, so a ready `PoolExhausted` that
  resolves with the real block deadline also spent is re-adjudicated by the
  segment through absolute instants — `acquire_abs = open_start +
  acquire_deadline()`; `block_abs <= acquire_abs` (tie included) → `Timeout`
  from entry, acquire strictly first → the `PoolExhausted` passes through
  (fixtures `transacted_open_pool_exhausted_adjudicated_by_absolute_instants`
  and the rewritten ready-open race fixture).
- **Walk precedence (per-arm BEFORE manager)**: manager resolution lives in
  the lowering walk's tail (`walk_transacted_block_deferred` takes the
  resolver), not in the S1 AST gate — a per-arm rejection
  (`TransactionControlSql`, `RuntimeSqlInJoinedStep`) is never masked by
  `ManagerMissing`, and a clean block with the always-absent production
  resolver still fails `ManagerMissing` at load.

## Architecture boundaries

Port stays provider-neutral; camel-core resolves via the registry only (no
camel-sql dependency); camel-sql implements. Statements execute only on lent
connections; `plan_binds` remains the only bind path. No XA/2PC, no
per-exchange transactions, no transaction EIP, no new route surface, no new
dependencies. The `camel-api` port delta (`Any` supertrait + `join` module +
docs-only contract) is explicitly accepted by 604c; `camel-api` is NOT
published in this change. S3 scope untouched (`Managed` join-required,
endpoint creation, Auto-fallback warnings, `txnreject`) — `Managed` stays
rejected at load.
